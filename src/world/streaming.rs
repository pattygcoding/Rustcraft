//! Generating chunks on background threads.
//!
//! Terrain generation and its sky lighting are *pure* functions of a chunk position and the
//! world seed: a chunk is built without reading anything outside itself. That is what makes this
//! the part of streaming that can leave the render thread — and it is the expensive part. A
//! chunk costs roughly 17 ms in a debug build, and the player crossing a chunk boundary asks for
//! a whole row of thirteen at once, which on the render thread is a quarter of a second of
//! frozen frames. Here it is instead a queue that the render thread only ever *collects* from:
//! [`ChunkPool::poll`] drains finished chunks without blocking, so a frame pays for nothing but
//! the chunks that happen to be ready.
//!
//! The workers are plain threads sharing a queue, for the usual reasons: no dependency to take
//! on, the work is CPU-bound and independent of everything else, and nearest-first ordering
//! falls out of pushing positions in order and popping them off the front. Idle workers sleep on
//! a [`Condvar`] rather than spinning, and [`ChunkPool`]'s `Drop` stops them and joins, so no
//! thread outlives the world it was generating for.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Instant;

use super::Chunk;
use super::ChunkPos;
use super::terrain::Terrain;

/// How many generator threads to run.
///
/// One core is left for the render thread — more threads than cores only adds context switches —
/// and the cap keeps a very wide machine from spending its whole memory on queued chunks.
fn worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|cores| cores.get().saturating_sub(1))
        .unwrap_or(1)
        .clamp(1, 4)
}

/// The waiting jobs, and the flag that tells the workers to stop.
#[derive(Default)]
struct Jobs {
    /// Positions waiting to be generated, nearest first.
    queue: VecDeque<ChunkPos>,
    /// Set when the pool is being dropped; wakes every worker out of its wait.
    stop: bool,
}

/// The job board: the queue, and the [`Condvar`] that wakes workers idle on it. An `Arc` because
/// every worker thread holds one.
#[derive(Default)]
struct Board {
    jobs: Mutex<Jobs>,
    wake: Condvar,
}

impl Board {
    /// The job queue, holding on through a poisoned lock: one panicking generator must not stop
    /// the others from shutting down cleanly.
    fn lock(&self) -> MutexGuard<'_, Jobs> {
        self.jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// What the generator threads have done, for the frame-timing log.
#[derive(Clone, Copy, Debug, Default)]
pub struct GenerationStats {
    /// Chunks generated since startup.
    pub chunks: u64,
    /// How long the slowest single chunk took, in milliseconds.
    pub worst_millis: f32,
    /// How long one chunk takes on average, in milliseconds.
    pub average_millis: f32,
}

/// Running generation totals, written by every worker and read by the render thread. Atomics
/// rather than a lock because this is written on the hot path and read once a second at most.
#[derive(Default)]
struct Counters {
    chunks: AtomicU64,
    micros: AtomicU64,
    worst_micros: AtomicU64,
}

impl Counters {
    /// Note one finished chunk and how long it took.
    fn record(&self, micros: u64) {
        self.chunks.fetch_add(1, Ordering::Relaxed);
        self.micros.fetch_add(micros, Ordering::Relaxed);
        self.worst_micros.fetch_max(micros, Ordering::Relaxed);
    }

    fn snapshot(&self) -> GenerationStats {
        let chunks = self.chunks.load(Ordering::Relaxed);
        let micros = self.micros.load(Ordering::Relaxed);
        GenerationStats {
            chunks,
            worst_millis: self.worst_micros.load(Ordering::Relaxed) as f32 / 1000.0,
            average_millis: micros as f32 / 1000.0 / chunks.max(1) as f32,
        }
    }
}

/// A small pool of threads that generate chunks and hand them back to the world.
///
/// The pool knows nothing about *which* chunks are wanted: the world decides that and keeps its
/// own book of what is in flight, which is also what lets it drop a chunk that arrives after the
/// player has moved on.
pub struct ChunkPool {
    board: Arc<Board>,
    /// Finished chunks, as `(position, chunk)`. A chunk does not know where it lives, so the
    /// position travels back with it.
    results: Receiver<(ChunkPos, Chunk)>,
    workers: Vec<JoinHandle<()>>,
    counters: Arc<Counters>,
}

impl ChunkPool {
    /// Start a pool of [`worker_count`] threads generating from `terrain`.
    ///
    /// The threads idle until [`ChunkPool::request`] is called.
    pub fn new(terrain: Arc<Terrain>) -> Self {
        let board = Arc::new(Board::default());
        let counters = Arc::new(Counters::default());
        let (sender, results) = mpsc::channel();
        let workers = (0..worker_count())
            .map(|i| {
                let board = Arc::clone(&board);
                let terrain = Arc::clone(&terrain);
                let counters = Arc::clone(&counters);
                let sender = sender.clone();
                std::thread::Builder::new()
                    .name(format!("generator-{i}"))
                    .spawn(move || work(&board, &terrain, &sender, &counters))
                    .expect("failed to spawn a generator thread")
            })
            .collect();
        // Only the workers' clones need to stay alive: once they have all exited, the channel
        // disconnects, so a blocking receive can never hang.
        drop(sender);
        Self {
            board,
            results,
            workers,
            counters,
        }
    }

    /// Queue `positions` for generation, in the order they are worth doing: the workers pop from
    /// the front, so passing them nearest-first is what makes the nearest chunks appear first.
    pub fn request(&self, positions: impl IntoIterator<Item = ChunkPos>) {
        self.board.lock().queue.extend(positions);
        self.board.wake.notify_all();
    }

    /// Forget queued work that `keep` rejects, so the threads spend their time on the patch the
    /// player is actually in.
    ///
    /// Only work still *queued* can be dropped; a chunk already being built finishes and is
    /// handed back, and the world discards it there if it is no longer wanted.
    pub fn discard(&self, keep: impl Fn(ChunkPos) -> bool) {
        self.board.lock().queue.retain(|&pos| keep(pos));
    }

    /// Every chunk finished since the last call, without blocking. The frame loop's way in.
    pub fn poll(&self) -> Vec<(ChunkPos, Chunk)> {
        let mut done = Vec::new();
        // Stops on `Empty` — and on `Disconnected`, when every worker has stopped. Either way
        // there is nothing more to collect.
        while let Ok(finished) = self.results.try_recv() {
            done.push(finished);
        }
        done
    }

    /// The next finished chunk, blocking until one arrives.
    ///
    /// `None` means the workers have all stopped, so a caller waiting for a chunk it asked for
    /// always finishes waiting.
    pub fn recv(&self) -> Option<(ChunkPos, Chunk)> {
        self.results.recv().ok()
    }

    /// What the threads have generated so far.
    pub fn stats(&self) -> GenerationStats {
        self.counters.snapshot()
    }
}

impl Drop for ChunkPool {
    /// Tell the workers to stop and wait for them, so no thread outlives the world.
    fn drop(&mut self) {
        self.board.lock().stop = true;
        self.board.wake.notify_all();
        for worker in self.workers.drain(..) {
            // A panicking generator is not a reason to panic on the way out.
            let _ = worker.join();
        }
    }
}

/// One generator thread: take a position, build its chunk, hand it back — until told to stop.
fn work(
    board: &Arc<Board>,
    terrain: &Arc<Terrain>,
    results: &Sender<(ChunkPos, Chunk)>,
    counters: &Counters,
) {
    while let Some(pos) = next_job(board) {
        let started = Instant::now();
        let mut chunk = terrain.generate_chunk(pos);
        // Lighting belongs with generation rather than with the render thread: it is a flood
        // fill over the whole chunk, and it is exactly what the mesher reads.
        chunk.relight();
        counters.record(started.elapsed().as_micros() as u64);
        // A send fails only once the world is gone, which is a reason to stop.
        if results.send((pos, chunk)).is_err() {
            return;
        }
    }
}

/// The next chunk to generate, sleeping while the queue is empty.
///
/// `None` means the pool is shutting down.
fn next_job(board: &Arc<Board>) -> Option<ChunkPos> {
    let mut jobs = board.lock();
    while jobs.queue.is_empty() && !jobs.stop {
        jobs = board
            .wake
            .wait(jobs)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    jobs.queue.pop_front()
}
