//! The language: what the game calls the things in it.
//!
//! Every block and every item has a **label**, and the labels live in a file rather than in the
//! code — `resources/assets/lang/en.json`, one flat JSON object of `"key": "value"` pairs, which
//! is the shape Minecraft's own language files have:
//!
//! ```json
//! { "block.grass_block": "Grass Block", "item.water_bucket": "Water Bucket" }
//! ```
//!
//! A key is a category and an id: `block.<id>` for a block, `item.<id>` for an item, where the id
//! is the snake-case word the block or item is known by ([`Block::key`], [`Item::key`]). Keeping
//! the labels in a file means a new block is a new *line* rather than a new arm in a `match`, and
//! it is what a second language would hang off — `en.json`, and then `fr.json` beside it.
//!
//! The labels are written the way Minecraft writes them — "Grass Block", not "GRASS BLOCK" — and
//! the bitmap font folds them to upper case as it draws, because that is the register five pixels
//! wide does well (see [`crate::font`]).
//!
//! **The JSON reader is hand-rolled, and deliberately.** A language file is the only JSON in the
//! project, and what it holds is one *shape*: an object whose values are strings. [`parse_labels`]
//! reads exactly that and refuses everything else — loudly, with the position — so a file that
//! grew a nested object or a bare number says so rather than being silently half-read. If the
//! format ever needs more than a flat map, that is the moment to reach for a real parser rather
//! than to grow this one.

use std::collections::HashMap;
use std::path::Path;

use crate::item::Item;
use crate::world::Block;

/// The labels, by key.
pub struct Lang {
    labels: HashMap<String, String>,
}

impl Lang {
    /// Read the labels from the JSON object at `path`.
    ///
    /// # Panics
    ///
    /// Panics if the file cannot be read or the JSON cannot be understood — a broken language file
    /// should fail loudly at startup, the way a missing texture does, rather than leave the game
    /// printing keys at the player.
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|err| panic!("cannot read language file {}: {err}", path.display()));
        let labels = parse_labels(&text)
            .unwrap_or_else(|err| panic!("cannot read language file {}: {err}", path.display()));
        log::info!("read {} labels from {}", labels.len(), path.display());
        Self { labels }
    }

    /// The label a key holds, if the file has one.
    pub fn label(&self, key: &str) -> Option<&str> {
        self.labels.get(key).map(String::as_str)
    }

    /// What the file calls `block`: `block.<id>`.
    pub fn label_of_block(&self, block: Block) -> Option<&str> {
        self.label(&format!("block.{}", block.key()))
    }

    /// What the file calls `item`: `block.<id>` for a block, `item.<id>` for a bucket.
    pub fn label_of_item(&self, item: Item) -> Option<&str> {
        match item {
            Item::Block(block) => self.label_of_block(block),
            bucket => self.label(&format!("item.{}", bucket.key())),
        }
    }
}

/// Read the *flat* JSON object of `"key": "value"` pairs that a language file is.
///
/// Errors are strings with the position in them, because the only thing a caller does with one is
/// put it in a panic message, and "expected a `:` at byte 41" is what a broken file needs to say.
fn parse_labels(text: &str) -> Result<HashMap<String, String>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0usize;

    /// Step over the whitespace JSON allows between everything.
    fn skip(chars: &[char], at: &mut usize) {
        while chars.get(*at).is_some_and(|c| c.is_whitespace()) {
            *at += 1;
        }
    }

    /// One JSON string, with the escapes a name could plausibly need.
    fn string(chars: &[char], at: &mut usize) -> Result<String, String> {
        if chars.get(*at) != Some(&'"') {
            return Err(format!("expected a string at character {at}"));
        }
        *at += 1;
        let mut out = String::new();
        loop {
            let c = *chars
                .get(*at)
                .ok_or_else(|| format!("a string is not closed (from character {at})"))?;
            *at += 1;
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let esc = *chars
                        .get(*at)
                        .ok_or_else(|| format!("an escape is unfinished at character {at}"))?;
                    *at += 1;
                    out.push(match esc {
                        '"' => '"',
                        '\\' => '\\',
                        '/' => '/',
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        'b' => '\u{8}',
                        'f' => '\u{c}',
                        'u' => {
                            let digits: String = chars
                                .get(*at..*at + 4)
                                .ok_or_else(|| format!("a \\u escape is short at character {at}"))?
                                .iter()
                                .collect();
                            *at += 4;
                            let code = u32::from_str_radix(&digits, 16).map_err(|_| {
                                format!("\\u{digits} is not four hex digits, at character {at}")
                            })?;
                            char::from_u32(code)
                                .ok_or_else(|| format!("\\u{digits} is not a character"))?
                        }
                        other => return Err(format!("unknown escape \\{other} at character {at}")),
                    });
                }
                other => out.push(other),
            }
        }
    }

    skip(&chars, &mut at);
    if chars.get(at) != Some(&'{') {
        return Err("a language file must be a JSON object".into());
    }
    at += 1;

    let mut labels = HashMap::new();
    loop {
        skip(&chars, &mut at);
        match chars.get(at) {
            // An empty object: the `}` where the first key would be.
            Some('}') if labels.is_empty() => {
                at += 1;
                break;
            }
            Some('"') => {}
            // A `}` here means a comma was followed by nothing, which JSON does not allow.
            _ => {
                return Err(format!("expected a key at character {at}"));
            }
        }
        let key = string(&chars, &mut at)?;
        skip(&chars, &mut at);
        if chars.get(at) != Some(&':') {
            return Err(format!("expected `:` after {key:?} at character {at}"));
        }
        at += 1;
        skip(&chars, &mut at);
        let value = string(&chars, &mut at)?;
        labels.insert(key, value);

        skip(&chars, &mut at);
        match chars.get(at) {
            Some(',') => at += 1,
            Some('}') => {
                at += 1;
                break;
            }
            _ => {
                return Err(format!(
                    "expected `,` or the end of the file at character {at}"
                ));
            }
        }
    }

    skip(&chars, &mut at);
    if at != chars.len() {
        return Err(format!("there is more after the object, at character {at}"));
    }
    Ok(labels)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parser reads the shape it is for, and puts up with the whitespace a hand-written file
    /// is full of.
    #[test]
    fn a_flat_object_reads_key_for_key() {
        let labels =
            parse_labels("{\n  \"block.stone\": \"Stone\",\n\n  \"item.bucket\" : \"Bucket\"\n}\n")
                .expect("a well-formed file");
        assert_eq!(labels.get("block.stone").map(String::as_str), Some("Stone"));
        assert_eq!(
            labels.get("item.bucket").map(String::as_str),
            Some("Bucket")
        );
        assert_eq!(labels.len(), 2);

        // An empty object is a file with no labels in it, not an error.
        assert!(parse_labels("{}").expect("an empty file").is_empty());
        // Escapes are understood, and are *not* what a language file usually holds.
        let labels = parse_labels(r#"{"a": "one \"two\" \\ three"}"#).expect("escapes");
        assert_eq!(
            labels.get("a").map(String::as_str),
            Some(r#"one "two" \ three"#)
        );
    }

    /// Everything the file could get wrong says so, with the position — a language file that
    /// cannot be read must fail loudly rather than leave the game printing keys.
    #[test]
    fn a_file_that_is_not_a_flat_object_is_refused() {
        for (text, why) in [
            ("", "nothing at all"),
            ("[]", "not an object"),
            ("{\"key\"}", "no value"),
            ("{\"key\": 3}", "a value that is not a string"),
            ("{\"key\": {\"nested\": \"no\"}}", "a nested object"),
            ("{\"key\": \"value\"", "an object that is not closed"),
            ("{\"key\": \"value\"} tail", "something after the object"),
            ("{\"a\": \"b\",}", "a trailing comma"),
        ] {
            assert!(
                parse_labels(text).is_err(),
                "this should be refused ({why}): {text:?}"
            );
        }
    }

    /// The file the game actually ships with is read by the same parser, and every block and item
    /// in the game has a label in it.
    ///
    /// This is the test that makes a missing label a *build* failure rather than something a
    /// player finds: add a block, add its line.
    #[test]
    fn every_block_and_item_has_a_label_in_the_real_file() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/assets/lang/en.json");
        let lang = Lang::load(path);

        for block in Block::ALL {
            let label = lang
                .label_of_block(block)
                .unwrap_or_else(|| panic!("{block:?} has no `block.{}` label", block.key()));
            assert!(!label.is_empty(), "{block:?} is labelled with nothing");
        }
        for item in [Item::Bucket, Item::WaterBucket, Item::LavaBucket] {
            let label = lang
                .label_of_item(item)
                .unwrap_or_else(|| panic!("{item:?} has no `item.{}` label", item.key()));
            assert!(!label.is_empty(), "{item:?} is labelled with nothing");
            // ...and a block item borrows its block's label, the way Minecraft's does.
            assert_eq!(
                lang.label_of_item(Item::Block(Block::Stone)),
                lang.label_of_block(Block::Stone)
            );
        }

        // A key nobody has is nothing, rather than a panic.
        assert_eq!(lang.label("block.not_a_block"), None);
    }

    /// Every label is something the bitmap font can draw once it has folded it to upper case —
    /// a stray hyphen or bracket would otherwise be a hole in the middle of a word on screen.
    #[test]
    fn every_label_is_something_the_font_can_draw() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/assets/lang/en.json");
        let lang = Lang::load(path);
        for block in Block::ALL {
            let label = lang.label_of_block(block).expect("covered above");
            for c in label.chars() {
                assert!(
                    crate::font::can_draw(c),
                    "{block:?} is labelled {label:?}, which the font cannot draw: {c:?}"
                );
            }
        }
    }
}
