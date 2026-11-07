//! Offline ECDICT lookup. Definitions and part-of-speech labels come from the
//! downloaded dictionary fields, independently of translation model output.
use std::{collections::HashMap, ops::Range, sync::OnceLock};

#[derive(Clone, Debug)]
pub struct Entry {
    pub word: String,
    pub phonetic: String,
    pub parts_of_speech: Vec<String>,
    pub senses: Vec<Sense>,
}

#[derive(Clone, Debug)]
pub struct Sense {
    pub part_of_speech: String,
    pub definition: String,
}

struct Dictionary {
    csv: String,
    rows: HashMap<String, Range<usize>>,
    phonetic: usize,
    translation: usize,
    definition: usize,
    pos: usize,
}
static DICTIONARY: OnceLock<Option<Dictionary>> = OnceLock::new();

/// Load once on a background thread. Missing data stays unavailable until the
/// next application start; lookup never downloads or calls a translation API.
pub fn warm_up() {
    let _ = dictionary();
}

/// Exact, case-insensitive English headword lookup. Sentence punctuation at the
/// edges is ignored; phrases and sentences are not represented as word entries.
pub fn lookup(word: &str) -> Option<Entry> {
    let key = normalize(word)?;
    let dictionary = dictionary()?;
    let range = dictionary.rows.get(&key)?.clone();
    let fields = csv_fields(&dictionary.csv[range])?;
    let value = |index: usize| fields.get(index).map_or("", String::as_str);
    let mut entry = Entry {
        word: value(0).to_owned(),
        phonetic: value(dictionary.phonetic).to_owned(),
        parts_of_speech: Vec::new(),
        senses: Vec::new(),
    };
    // The base ECDICT release leaves its dedicated POS column empty. Other
    // releases use n:46/v:54; support that field and the explicit POS annotations
    // in translation/definition. An absent annotation remains absent.
    for part in value(dictionary.pos).split('/') {
        if let Some(pos) = canonical_pos(part.split(':').next().unwrap_or("")) {
            push_unique(&mut entry.parts_of_speech, pos);
        }
    }
    append_senses(&mut entry, value(dictionary.translation));
    append_senses(&mut entry, value(dictionary.definition));
    (!entry.senses.is_empty()).then_some(entry)
}

fn dictionary() -> Option<&'static Dictionary> {
    DICTIONARY.get_or_init(Dictionary::load).as_ref()
}
impl Dictionary {
    fn load() -> Option<Self> {
        let csv =
            std::fs::read_to_string(crate::offline::resource("dictionary/ecdict.csv")).ok()?;
        let mut records = Records {
            bytes: csv.as_bytes(),
            position: 0,
        };
        let header = csv_fields(&csv[records.next()?])?;
        if header.first()?.trim_start_matches('\u{feff}') != "word" {
            return None;
        }
        let column = |name: &str| header.iter().position(|field| field == name);
        let (phonetic, translation, definition, pos) = (
            column("phonetic")?,
            column("translation")?,
            column("definition")?,
            column("pos")?,
        );
        let mut rows = HashMap::new();
        for range in records {
            let record = &csv[range.clone()];
            let headword = first_field(record)?;
            if let Some(key) = normalize(&headword) {
                // Prefer the canonical lowercase headword when a source also
                // contains a differently capitalized proper noun.
                if headword == key || !rows.contains_key(&key) {
                    rows.insert(key, range);
                }
            }
        }
        if rows.is_empty() {
            return None;
        }
        Some(Self {
            csv,
            rows,
            phonetic,
            translation,
            definition,
            pos,
        })
    }
}

fn normalize(word: &str) -> Option<String> {
    let word = word.trim().trim_matches(|c: char| {
        matches!(
            c,
            '.' | ',' | ';' | ':' | '!' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '“' | '”'
        )
    });
    if word.is_empty() || word.len() > 100 {
        return None;
    }
    let word = word.replace('’', "'").to_ascii_lowercase();
    if !word
        .chars()
        .all(|c| c.is_ascii_alphabetic() || c == '-' || c == '\'')
        || !word.chars().any(|c| c.is_ascii_alphabetic())
    {
        return None;
    }
    Some(word)
}

fn push_unique(items: &mut Vec<String>, value: &str) {
    if !items.iter().any(|old| old == value) {
        items.push(value.to_owned());
    }
}
fn canonical_pos(value: &str) -> Option<&'static str> {
    Some(
        match value
            .trim()
            .trim_end_matches('.')
            .to_ascii_lowercase()
            .as_str()
        {
            "a" | "adj" | "j" | "s" => "adj.",
            "n" | "noun" => "n.",
            "v" | "verb" => "v.",
            "vt" | "v. t" => "vt.",
            "vi" | "v. i" => "vi.",
            "adv" | "ad" | "r" => "adv.",
            "prep" => "prep.",
            "pron" => "pron.",
            "conj" => "conj.",
            "interj" | "int" => "interj.",
            "art" | "definite article" | "indefinite article" => "art.",
            "det" => "det.",
            "num" => "num.",
            "aux" => "aux.",
            "abbr" => "abbr.",
            "modal" => "modal.",
            _ => return None,
        },
    )
}

fn prefix(line: &str) -> (Option<&'static str>, &str) {
    for label in ["definite article.", "indefinite article.", "v. t.", "v. i."] {
        if let Some(rest) = line.strip_prefix(label) {
            return (canonical_pos(label), rest.trim());
        }
    }
    if let Some((label, rest)) = line.split_once('.') {
        if label.len() <= 8
            && let Some(pos) = canonical_pos(label)
        {
            return (Some(pos), rest.trim());
        }
    }
    (None, line)
}

fn append_senses(entry: &mut Entry, field: &str) {
    let expanded = field.replace("\\r\\n", "\n").replace("\\n", "\n");
    for raw_line in expanded.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let (part, definition) = prefix(line);
        if definition.is_empty() {
            continue;
        }
        if let Some(pos) = part {
            push_unique(&mut entry.parts_of_speech, pos);
        }
        // Indented source lines continue the previous English definition.
        if part.is_none()
            && raw_line.starts_with(char::is_whitespace)
            && let Some(last) = entry.senses.last_mut()
        {
            last.definition.push(' ');
            last.definition.push_str(definition);
            continue;
        }
        let part_of_speech = part.unwrap_or("");
        if !entry
            .senses
            .iter()
            .any(|sense| sense.part_of_speech == part_of_speech && sense.definition == definition)
        {
            entry.senses.push(Sense {
                part_of_speech: part_of_speech.to_owned(),
                definition: definition.to_owned(),
            });
        }
    }
}

/// CSV record boundaries, including quoted fields containing actual newlines.
struct Records<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl Iterator for Records<'_> {
    type Item = Range<usize>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.position >= self.bytes.len() {
            return None;
        }
        let start = self.position;
        let mut quoted = false;
        while self.position < self.bytes.len() {
            let byte = self.bytes[self.position];
            if byte == b'"' {
                if quoted && self.bytes.get(self.position + 1) == Some(&b'"') {
                    self.position += 2;
                    continue;
                }
                quoted = !quoted;
            }
            self.position += 1;
            if byte == b'\n' && !quoted {
                let mut end = self.position - 1;
                if end > start && self.bytes[end - 1] == b'\r' {
                    end -= 1;
                }
                return Some(start..end);
            }
        }
        Some(start..self.position)
    }
}
fn first_field(record: &str) -> Option<String> {
    if !record.starts_with('"') {
        return Some(record.split(',').next()?.to_owned());
    }
    let end = record
        .as_bytes()
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(i, byte)| {
            (*byte == b'"' && record.as_bytes().get(i + 1) == Some(&b',')).then_some(i)
        })?;
    Some(record[1..end].replace("\"\"", "\""))
}
fn csv_fields(record: &str) -> Option<Vec<String>> {
    let bytes = record.as_bytes();
    let mut fields = Vec::new();
    let mut field = Vec::new();
    let mut quoted = false;
    let mut position = 0;
    while position < bytes.len() {
        match bytes[position] {
            b'"' if quoted && bytes.get(position + 1) == Some(&b'"') => {
                field.push(b'"');
                position += 1;
            }
            b'"' => quoted = !quoted,
            b',' if !quoted => {
                fields.push(String::from_utf8(std::mem::take(&mut field)).ok()?);
            }
            byte => field.push(byte),
        }
        position += 1;
    }
    if quoted {
        return None;
    }
    fields.push(String::from_utf8(field).ok()?);
    Some(fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn csv_quotes_newlines_and_literal_escapes_are_preserved() {
        let data = "word,translation\nentry,\"n. one, two\nthree \"\"quoted\"\"\"\n";
        let records: Vec<_> = Records {
            bytes: data.as_bytes(),
            position: 0,
        }
        .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(
            csv_fields(&data[records[1].clone()]).unwrap()[1],
            "n. one, two\nthree \"quoted\""
        );
        assert_eq!(
            prefix("a. original adjective annotation"),
            (Some("adj."), "original adjective annotation")
        );
        assert_eq!(prefix("unknown annotation"), (None, "unknown annotation"));
    }
    #[test]
    #[ignore = "requires scripts/download-dictionary.ps1"]
    fn real_downloaded_entries_have_source_parts_of_speech_and_multiple_senses() {
        let sad = lookup("SAD.").expect("downloaded ECDICT entry");
        assert!(sad.parts_of_speech.iter().any(|p| p == "adj."));
        assert_eq!(sad.phonetic, "sæd");
        assert!(sad.senses[0].definition.contains("悲哀"));
        let snake = lookup("snake").unwrap();
        assert!(snake.parts_of_speech.iter().any(|p| p == "n."));
        assert!(snake.senses[0].definition.contains('蛇'));
        assert!(snake.senses.len() > 1);
        assert!(lookup("this is a sentence").is_none());
        assert!(lookup("qzxnonexistentlexemeqzx").is_none());
    }
}
