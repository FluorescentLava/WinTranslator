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
    // Chinese lookups store the English candidate itself here. Source Chinese
    // glosses belong to dictionary matching, not to the list of translations.
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
    rank: (u8, bool, u32),
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

/// Resolve a candidate's POS from the matching Chinese sense. A polysemous
/// English word must not borrow the first POS of an unrelated meaning.
pub(crate) fn chinese_candidate_parts_of_speech(english: &str, chinese: &str) -> Vec<String> {
    let Some(dictionary) = dictionary() else {
        return Vec::new();
    };
    candidate_parts_of_speech(dictionary, english, chinese)
}

pub(crate) fn foreign_candidate(english: &str) -> bool {
    let Some(dictionary) = dictionary() else {
        return false;
    };
    let Some(fields) = normalize(english)
        .and_then(|word| dictionary.rows.get(&word))
        .and_then(|row| csv_fields(&dictionary.csv[row.clone()]))
    else {
        return false;
    };
    let mut source = Entry {
        word: english.to_owned(),
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
    !source.senses.is_empty()
        && source
            .senses
            .iter()
            .all(|sense| foreign_language_gloss(&sense.definition))
}

fn candidate_parts_of_speech(dictionary: &Dictionary, english: &str, chinese: &str) -> Vec<String> {
    let Some(key) = chinese_key(chinese) else {
        return Vec::new();
    };
    let Some(fields) = normalize(english)
        .and_then(|word| dictionary.rows.get(&word))
        .and_then(|row| csv_fields(&dictionary.csv[row.clone()]))
    else {
        return Vec::new();
    };
    let mut source = Entry {
        word: english.to_owned(),
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
    let mut parts = Vec::new();
    for sense in &source.senses {
        if sense.part_of_speech.is_empty() || foreign_language_gloss(&sense.definition) {
            continue;
        }
        let matched = sense
            .definition
            .split([',', '，', ';', '；', '、', '\n'])
            .any(|part| {
                let Some(candidate) = chinese_key(strip_annotation(part)) else {
                    return false;
                };
                candidate == key
                    || (sense.part_of_speech == "adj." && candidate.strip_suffix('的') == Some(key))
            });
        if matched {
            push_unique(&mut parts, &sense.part_of_speech);
        }
    }
    if parts.is_empty()
        && source.parts_of_speech.len() == 1
        && source
            .senses
            .iter()
            .any(|sense| !foreign_language_gloss(&sense.definition))
    {
        // A single annotated translation POS is unambiguous even if the local
        // dictionary uses another Chinese synonym for this same candidate.
        source.parts_of_speech
    } else {
        parts
    }
}

pub(crate) fn append_chinese_candidate(entry: &mut Entry, english: &str, parts: &[String]) {
    for part in parts {
        push_unique(&mut entry.parts_of_speech, part);
    }
    if let Some(sense) = entry
        .senses
        .iter_mut()
        .find(|sense| sense.definition.eq_ignore_ascii_case(english))
    {
        let mut combined = if sense.part_of_speech == "—" {
            Vec::new()
        } else {
            sense
                .part_of_speech
                .split(" / ")
                .map(str::to_owned)
                .collect()
        };
        for part in parts {
            push_unique(&mut combined, part);
        }
        if !combined.is_empty() {
            sense.part_of_speech = combined.join(" / ");
        }
        return;
    }
    entry.senses.push(Sense {
        part_of_speech: if parts.is_empty() {
            "—".into()
        } else {
            parts.join(" / ")
        },
        definition: english.to_owned(),
    });
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
        // Keep collecting POS for candidates already admitted at the limit.
        if entry.senses.len() < 16
            || entry
                .senses
                .iter()
                .any(|sense| sense.definition.eq_ignore_ascii_case(english))
        {
            let parts = if sense.part_of_speech.is_empty() {
                Vec::new()
            } else {
                vec![sense.part_of_speech.clone()]
            };
            append_chinese_candidate(&mut entry, english, &parts);
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
                if foreign_language_gloss(&sense.definition) {
                    continue;
                }
                let mut seen = HashSet::new();
                for part in sense.definition.split([',', '，', ';', '；', '、', '\n']) {
                    let part = strip_annotation(part);
                    let Some(key) = chinese_key(part) else {
                        continue;
                    };
                    if seen.insert(key) {
                        index.entry(key.to_owned()).or_default().push(ReverseMatch {
                            row: row.clone(),
                            sense: sense_index,
                            rank,
                        });
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

/// A parenthesized language tag identifies a foreign-language expression, not
/// an English translation. Square [法] is ECDICT's legal-domain label.
pub(crate) fn foreign_language_gloss(mut value: &str) -> bool {
    value = value.trim();
    while let Some(opening) = value.chars().next() {
        let closing = match opening {
            '(' => ')',
            '（' => '）',
            '[' => ']',
            '【' => '】',
            '<' => '>',
            '〈' => '〉',
            _ => break,
        };
        let Some((label, rest)) = value[opening.len_utf8()..].split_once(closing) else {
            break;
        };
        let language = |label: &str| {
            matches!(
                label.trim().to_ascii_lowercase().as_str(),
                "法" | "法语"
                    | "french"
                    | "意"
                    | "意大利"
                    | "意大利语"
                    | "italian"
                    | "德"
                    | "德语"
                    | "german"
                    | "西"
                    | "西语"
                    | "西班牙"
                    | "西班牙语"
                    | "spanish"
                    | "日"
                    | "日语"
                    | "japanese"
                    | "拉"
                    | "拉丁"
                    | "拉丁语"
                    | "latin"
                    | "俄"
                    | "俄语"
                    | "russian"
                    | "葡"
                    | "葡语"
                    | "葡萄牙语"
                    | "portuguese"
                    | "韩"
                    | "韩语"
                    | "朝鲜语"
                    | "korean"
                    | "阿拉伯"
                    | "阿拉伯语"
                    | "arabic"
                    | "梵"
                    | "梵语"
                    | "sanskrit"
            )
        };
        if !(matches!(opening, '[' | '【') && label.chars().count() == 1)
            && label.split(['、', ',', '，', '/']).all(language)
        {
            return true;
        }
        value = rest.trim();
    }
    false
}

pub(crate) fn strip_annotation(mut value: &str) -> &str {
    value = value.trim();
    while value.starts_with(['[', '【', '(', '（', '<', '〈']) {
        let closing = match value.chars().next() {
            Some('[') => ']',
            Some('【') => '】',
            Some('(') => ')',
            Some('<') => '>',
            Some('〈') => '〉',
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
        assert_eq!(snake.senses[0].definition, "snake");
        assert_eq!(snake.senses[1].definition, "serpent");
        assert_eq!(snake.parts_of_speech, ["n."]);
        assert!(
            snake
                .senses
                .iter()
                .all(|sense| sense.part_of_speech == "n.")
        );
        let sad = reverse_entry(&dictionary, "伤心的").unwrap();
        assert_eq!(sad.senses[0].part_of_speech, "adj.");
        assert_eq!(sad.senses[0].definition, "sad");
        assert!(reverse_entry(&dictionary, "伤心").is_none());
        let hello = reverse_entry(&dictionary, "你好").unwrap();
        assert_eq!(hello.senses.len(), 2);
        assert!(
            hello
                .senses
                .iter()
                .any(|sense| sense.definition == "how are you")
        );
        assert_eq!(
            reverse_entry(&dictionary, "银行").unwrap().senses[0].definition,
            "bank"
        );
        assert!(reverse_entry(&dictionary, "我今天去公园").is_none());
        assert_eq!(chinese_key(" 你好！ "), Some("你好"));
        assert!(chinese_key("snake").is_none());
        assert!(chinese_key("这是一个很长的中文句子而不是一个词语").is_none());
    }

    #[test]
    fn chinese_candidates_are_distinct_and_keep_only_matching_parts_of_speech() {
        let dictionary = Dictionary::from_csv(
            concat!(
                "word,phonetic,definition,translation,pos,collins,oxford,bnc,frq\n",
                "light,,,n. 光\\na. 轻的；明亮的\\nvt. 点燃,,3,1,500,500\n",
                "love,,,n. 爱\\nvt. 爱,,4,1,100,100\n",
                "affection,,,n. 爱；感情,,3,1,1000,1000\n",
                "lovely,,,a. 可爱的,,3,1,1000,1000\n",
                "hello,,n. a greeting,interj. 喂,,3,1,1000,1000\n",
            )
            .into(),
        )
        .unwrap();
        let love = reverse_entry(&dictionary, "爱").unwrap();
        assert_eq!(love.senses.len(), 2);
        assert_eq!(love.senses[0].definition, "love");
        assert_eq!(love.senses[0].part_of_speech, "n. / vt.");
        assert_eq!(love.senses[1].definition, "affection");
        assert_eq!(love.senses[1].part_of_speech, "n.");
        assert_eq!(
            candidate_parts_of_speech(&dictionary, "light", "明亮"),
            ["adj."]
        );
        assert_eq!(
            candidate_parts_of_speech(&dictionary, "light", "光"),
            ["n."]
        );
        assert!(candidate_parts_of_speech(&dictionary, "light", "轻松").is_empty());
        assert_eq!(
            candidate_parts_of_speech(&dictionary, "hello", "你好"),
            ["interj."]
        );
        assert!(reverse_entry(&dictionary, "光线").is_none());
        assert!(reverse_entry(&dictionary, "我想向你表达爱").is_none());
    }

    #[test]
    fn reverse_candidates_exclude_foreign_labels_and_do_not_shorten_chinese_glosses() {
        let dictionary = Dictionary::from_csv(
            concat!(
                "word,phonetic,definition,translation,pos,collins,oxford,bnc,frq\n",
                "hello,,,interj. 你好,,4,1,500,500\n",
                "hi,,,interj. 你好,,4,1,600,600\n",
                "Bonjour,,,n. （法）你好；问候,,0,0,40000,0\n",
                "ciao,,,int. （意）你好；再见,,0,0,34000,0\n",
                "snake,,,n. 蛇,,3,1,2000,2000\n",
                "serpentine,,,a. 蛇的,,0,0,24000,24000\n",
                "clause,,,n. [法] 条款,,3,1,4000,4000\n",
                "ballet,,,n. 芭蕾舞,,2,0,4400,4400\n",
            )
            .into(),
        )
        .unwrap();
        let greetings = reverse_entry(&dictionary, "你好").unwrap();
        assert_eq!(
            greetings
                .senses
                .iter()
                .map(|sense| sense.definition.as_str())
                .collect::<Vec<_>>(),
            ["hello", "hi"]
        );
        assert!(reverse_entry(&dictionary, "再见").is_none());
        let snake = reverse_entry(&dictionary, "蛇").unwrap();
        assert_eq!(snake.senses.len(), 1);
        assert_eq!(snake.senses[0].definition, "snake");
        assert_eq!(
            reverse_entry(&dictionary, "蛇的").unwrap().senses[0].definition,
            "serpentine"
        );
        assert_eq!(
            reverse_entry(&dictionary, "条款").unwrap().senses[0].definition,
            "clause"
        );
        assert_eq!(
            reverse_entry(&dictionary, "芭蕾舞").unwrap().senses[0].definition,
            "ballet"
        );
        for annotated in [
            "（法）你好",
            "(Italian) hello",
            "<日> 再见",
            "[法语] 你好",
            "（法、意）问候",
        ] {
            assert!(foreign_language_gloss(annotated), "{annotated}");
        }
        for ordinary in [
            "[法] 条款",
            "（英）你好",
            "n. a French dance",
            "（口语）你好",
        ] {
            assert!(!foreign_language_gloss(ordinary), "{ordinary}");
        }
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
            assert!(
                !entry.senses.is_empty(),
                "{query} needs a dictionary candidate"
            );
            assert!(entry.senses.iter().all(|sense| !matches!(
                sense.definition.as_str(),
                "Bonjour" | "ciao" | "serpentine" | "snaky"
            )));
            assert!(
                entry
                    .senses
                    .iter()
                    .any(|sense| sense.definition == expected),
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
