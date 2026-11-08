//! Offline ECDICT lookup. Definitions and part-of-speech labels come from the
//! downloaded dictionary fields, independently of translation model output.
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::OnceLock,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupDirection {
    EnglishToChinese,
    ChineseToEnglish,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub word: String,
    pub direction: LookupDirection,
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
    reverse: OnceLock<HashMap<String, Vec<ReverseMatch>>>,
}

#[derive(Clone)]
struct ReverseMatch {
    row: Range<usize>,
    sense: usize,
    rank: (bool, u8, bool, u32),
}
static DICTIONARY: OnceLock<Option<Dictionary>> = OnceLock::new();

/// Load once on a background thread. Missing data stays unavailable until the
/// next application start; lookup never downloads or calls a translation API.
pub fn warm_up() {
    if let Some(dictionary) = dictionary() {
        dictionary
            .reverse
            .get_or_init(|| dictionary.reverse_index());
    }
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
        direction: LookupDirection::EnglishToChinese,
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

/// Reverse exact Chinese dictionary senses, including common English phrases.
/// This is independent of the selected translation API and works offline.
pub fn lookup_chinese(word: &str) -> Option<Entry> {
    let key = chinese_key(word)?;
    let dictionary = dictionary()?;
    reverse_entry(dictionary, key)
}

fn reverse_entry(dictionary: &Dictionary, key: &str) -> Option<Entry> {
    let index = dictionary
        .reverse
        .get_or_init(|| dictionary.reverse_index());
    let mut matches = index.get(key)?.iter().collect::<Vec<_>>();
    matches.sort_by(|a, b| {
        a.rank
            .cmp(&b.rank)
            .then_with(|| a.row.start.cmp(&b.row.start))
    });
    let mut entry = Entry {
        word: key.to_owned(),
        direction: LookupDirection::ChineseToEnglish,
        phonetic: String::new(),
        parts_of_speech: Vec::new(),
        senses: Vec::new(),
    };
    let mut seen = HashSet::new();
    for matched in matches {
        let Some(fields) = csv_fields(&dictionary.csv[matched.row.clone()]) else {
            continue;
        };
        let Some(english) = fields.first() else {
            continue;
        };
        let mut source = Entry {
            word: english.clone(),
            direction: LookupDirection::EnglishToChinese,
            phonetic: String::new(),
            parts_of_speech: Vec::new(),
            senses: Vec::new(),
        };
        append_senses(
            &mut source,
            fields
                .get(dictionary.translation)
                .map_or("", String::as_str),
        );
        let Some(sense) = source.senses.get(matched.sense) else {
            continue;
        };
        if !seen.insert((english.to_ascii_lowercase(), sense.part_of_speech.clone())) {
            continue;
        }
        if !sense.part_of_speech.is_empty() {
            push_unique(&mut entry.parts_of_speech, &sense.part_of_speech);
        }
        entry.senses.push(Sense {
            part_of_speech: if sense.part_of_speech.is_empty() {
                "—".into()
            } else {
                sense.part_of_speech.clone()
            },
            definition: format!("{english} · {}", sense.definition),
        });
        if entry.senses.len() == 16 {
            break;
        }
    }
    (!entry.senses.is_empty()).then_some(entry)
}

fn dictionary() -> Option<&'static Dictionary> {
    DICTIONARY.get_or_init(Dictionary::load).as_ref()
}
impl Dictionary {
    fn load() -> Option<Self> {
        let csv =
            std::fs::read_to_string(crate::offline::resource("dictionary/ecdict.csv")).ok()?;
        Self::from_csv(csv)
    }

    fn from_csv(csv: String) -> Option<Self> {
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
            reverse: OnceLock::new(),
        })
    }

    fn reverse_index(&self) -> HashMap<String, Vec<ReverseMatch>> {
        let header_range = Records {
            bytes: self.csv.as_bytes(),
            position: 0,
        }
        .next();
        let header = header_range
            .and_then(|range| csv_fields(&self.csv[range]))
            .unwrap_or_default();
        let column = |name: &str| header.iter().position(|field| field == name);
        let (collins, oxford, bnc, frq) = (
            column("collins"),
            column("oxford"),
            column("bnc"),
            column("frq"),
        );
        let mut index: HashMap<String, Vec<ReverseMatch>> = HashMap::new();
        for row in (Records {
            bytes: self.csv.as_bytes(),
            position: 0,
        })
        .skip(1)
        {
            let Some(fields) = csv_fields(&self.csv[row.clone()]) else {
                continue;
            };
            let word = fields.first().map_or("", String::as_str);
            if !english_candidate(word) {
                continue;
            }
            let number = |column: Option<usize>| {
                column
                    .and_then(|i| fields.get(i)?.parse::<u32>().ok())
                    .unwrap_or(0)
            };
            let frequency = [number(bnc), number(frq)]
                .into_iter()
                .filter(|n| *n > 0)
                .min()
                .unwrap_or(u32::MAX);
            let rank = (
                5 - number(collins).min(5) as u8,
                number(oxford) == 0,
                frequency,
            );
            let mut entry = Entry {
                word: String::new(),
                direction: LookupDirection::EnglishToChinese,
                phonetic: String::new(),
                parts_of_speech: Vec::new(),
                senses: Vec::new(),
            };
            append_senses(
                &mut entry,
                fields.get(self.translation).map_or("", String::as_str),
            );
            for (sense_index, sense) in entry.senses.iter().enumerate() {
                let adjective = sense.part_of_speech == "adj.";
                let mut seen = HashSet::new();
                for part in sense.definition.split([',', '，', ';', '；', '、', '\n']) {
                    let part = strip_annotation(part);
                    let Some(key) = chinese_key(part) else {
                        continue;
                    };
                    for (key, alias) in std::iter::once((key, false)).chain(
                        adjective
                            .then(|| key.strip_suffix('的'))
                            .flatten()
                            .and_then(chinese_key)
                            .map(|key| (key, true)),
                    ) {
                        if seen.insert(key) {
                            index.entry(key.to_owned()).or_default().push(ReverseMatch {
                                row: row.clone(),
                                sense: sense_index,
                                rank: (alias, rank.0, rank.1, rank.2),
                            });
                        }
                    }
                }
            }
        }
        index
    }
}

fn english_candidate(word: &str) -> bool {
    !word.is_empty()
        && word.len() <= 100
        && word
            .chars()
            .all(|c| c.is_ascii_alphabetic() || matches!(c, ' ' | '-' | '\''))
        && word.chars().any(|c| c.is_ascii_alphabetic())
}

fn chinese_key(word: &str) -> Option<&str> {
    let word = word.trim().trim_matches([
        '。', '，', '；', '：', '！', '？', '.', ',', ';', ':', '!', '?', '“', '”', '"',
    ]);
    (word.chars().count() <= 16
        && !word.is_empty()
        && word.chars().all(|c| ('\u{3400}'..='\u{9fff}').contains(&c)))
    .then_some(word)
}

fn strip_annotation(mut value: &str) -> &str {
    value = value.trim();
    while value.starts_with(['[', '【', '(', '（']) {
        let closing = match value.chars().next() {
            Some('[') => ']',
            Some('【') => '】',
            Some('(') => ')',
            _ => '）',
        };
        let Some((_, rest)) = value.split_once(closing) else {
            break;
        };
        value = rest.trim();
    }
    value.split(['(', '（']).next().unwrap_or(value).trim()
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
    fn chinese_reverse_lookup_returns_ranked_senses_without_substring_matches() {
        let dictionary = Dictionary::from_csv(
            concat!(
                "word,phonetic,definition,translation,pos,collins,oxford,bnc,frq\n",
                "serpent,,,n. 蛇；狡猾的人,,1,0,10000,10000\n",
                "snake,,,n. 蛇\\nv. 蜿蜒行进,,3,1,2000,2000\n",
                "snakeskin,,,n. 蛇皮,,0,0,20000,20000\n",
                "sad,,,a. 伤心的；悲伤的,,4,1,1000,1000\n",
                "hello,,,int. 你好！；喂,,4,1,500,500\n",
                "how are you,,,interj. 你好,,0,0,0,0\n",
                "bank,,,n. [经] 银行（金融机构）；河岸,,5,1,300,300\n",
            )
            .into(),
        )
        .unwrap();
        let snake = reverse_entry(&dictionary, "蛇").unwrap();
        assert_eq!(snake.direction, LookupDirection::ChineseToEnglish);
        assert_eq!(snake.senses.len(), 2);
        assert!(snake.senses[0].definition.starts_with("snake ·"));
        assert!(snake.senses[1].definition.starts_with("serpent ·"));
        assert_eq!(snake.parts_of_speech, ["n."]);
        assert!(
            snake
                .senses
                .iter()
                .all(|sense| sense.part_of_speech == "n.")
        );
        let sad = reverse_entry(&dictionary, "伤心").unwrap();
        assert_eq!(sad.senses[0].part_of_speech, "adj.");
        assert!(sad.senses[0].definition.starts_with("sad ·"));
        let hello = reverse_entry(&dictionary, "你好").unwrap();
        assert_eq!(hello.senses.len(), 2);
        assert!(
            hello
                .senses
                .iter()
                .any(|sense| sense.definition.starts_with("how are you ·"))
        );
        assert!(
            reverse_entry(&dictionary, "银行").unwrap().senses[0]
                .definition
                .starts_with("bank ·")
        );
        assert!(reverse_entry(&dictionary, "我今天去公园").is_none());
        assert_eq!(chinese_key(" 你好！ "), Some("你好"));
        assert!(chinese_key("snake").is_none());
        assert!(chinese_key("这是一个很长的中文句子而不是一个词语").is_none());
    }

    #[test]
    #[ignore = "requires scripts/download-dictionary.ps1"]
    fn real_chinese_queries_have_alternative_english_words_and_parts_of_speech() {
        let started = std::time::Instant::now();
        for (query, expected) in [
            ("蛇", "snake"),
            ("伤心", "grief"),
            ("银行", "bank"),
            ("你好", "hiya"),
        ] {
            let entry = lookup_chinese(query).expect(query);
            assert_eq!(entry.direction, LookupDirection::ChineseToEnglish);
            assert!(entry.senses.len() > 1, "{query} needs alternatives");
            assert!(
                entry
                    .senses
                    .iter()
                    .any(|sense| sense.definition.split(" · ").next() == Some(expected)),
                "{query} is missing {expected}"
            );
            assert!(
                !entry.parts_of_speech.is_empty(),
                "{query} needs part-of-speech metadata"
            );
            println!(
                "{query}: {}",
                entry
                    .senses
                    .iter()
                    .map(|sense| format!("{} {}", sense.part_of_speech, sense.definition))
                    .collect::<Vec<_>>()
                    .join(" | ")
            );
        }
        println!(
            "Chinese dictionary cold load and four lookups: {:?}",
            started.elapsed()
        );
    }
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
