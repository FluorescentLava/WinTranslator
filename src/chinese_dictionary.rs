//! Exact Chinese word entries from Youdao's Chinese-English dictionary. This
//! supplements the offline reverse index and caches successful results locally.
use crate::dictionary::{Entry, LookupDirection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

const MAX_RESPONSE_BYTES: usize = 512 * 1024;
const MAX_CACHE_BYTES: usize = 32 * 1024;
const MAX_SENSES: usize = 16;
const MAX_ENGLISH_CHARS: usize = 100;
const MAX_EXPLANATION_CHARS: usize = 100;
const MAX_RAW_FIELD_BYTES: usize = 8 * 1024;
static CLIENT: OnceLock<Option<reqwest::blocking::Client>> = OnceLock::new();
static CACHE_LOCK: Mutex<()> = Mutex::new(());
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

// Cache source text and only explicitly annotated, recognized source POS.
// Missing POS can be supplied by the installed English dictionary on each read.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CachedEntry {
    version: u8,
    query: String,
    senses: Vec<CachedSense>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CachedSense {
    english: String,
    // Keep reading existing cache files; Chinese glosses are no longer part of
    // the displayed English candidate and are optional in newer responses.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    explanation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    part_of_speech: Option<String>,
}

/// Supplement one Chinese dictionary word with exact Chinese-English senses.
/// Only at most sixteen Han characters are sent; punctuation, spaces inside a
/// query, mixed scripts and longer text are rejected before any network access.
/// Network failure is optional: a previously cached entry works offline.
pub fn lookup(text: &str, cancelled: &dyn Fn() -> bool) -> Option<Entry> {
    let query = chinese_word(text)?;
    if cancelled() {
        return None;
    }
    let path = cache_path(query);
    if let Some(cached) = read_cache(&path, query, cancelled) {
        return make_entry(&cached, cancelled);
    }
    let cached = fetch_entry(query, cancelled)?;
    let entry = make_entry(&cached, cancelled)?;
    save_cache(&path, &cached, cancelled);
    (!cancelled()).then_some(entry)
}

fn fetch_entry(query: &str, cancelled: &dyn Fn() -> bool) -> Option<CachedEntry> {
    chinese_word(query)?;
    let client = CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_millis(900))
                .timeout(Duration::from_secs(2))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("TranslationTool/0.1")
                .build()
                .ok()
        })
        .as_ref()?;
    if cancelled() {
        return None;
    }
    let mut response = client
        .get("https://dict.youdao.com/jsonapi")
        .query(&[("q", query)])
        .send()
        .ok()?;
    if cancelled()
        || !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return None;
    }
    let bytes = read_limited(&mut response, MAX_RESPONSE_BYTES, cancelled)?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    if cancelled() {
        return None;
    }
    parse_response(query, &value)
}

fn chinese_word(text: &str) -> Option<&str> {
    let text = text.trim();
    (!text.is_empty() && text.chars().count() <= 16 && text.chars().all(is_han)).then_some(text)
}

fn is_han(c: char) -> bool {
    ('\u{3400}'..='\u{9fff}').contains(&c)
}

fn read_limited(
    reader: &mut impl Read,
    limit: usize,
    cancelled: &dyn Fn() -> bool,
) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8 * 1024];
    loop {
        if cancelled() {
            return None;
        }
        let count = reader.read(&mut buffer).ok()?;
        if cancelled() || bytes.len().checked_add(count)? > limit {
            return None;
        }
        if count == 0 {
            return Some(bytes);
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

fn parse_response(query: &str, value: &Value) -> Option<CachedEntry> {
    chinese_word(query)?;
    let words = value.get("ce")?.get("word")?.as_array()?;
    if words.is_empty() || words.len() > 16 {
        return None;
    }
    let mut senses = Vec::new();
    let mut seen = HashSet::new();
    for word in words {
        // Other returned words may be spelling suggestions or compounds. They
        // are not senses of the user's query, even if a substring matches.
        if word.pointer("/return-phrase/l/i").and_then(Value::as_str) != Some(query) {
            continue;
        }
        let Some(groups) = word.get("trs").and_then(Value::as_array) else {
            continue;
        };
        if groups.len() > 128 {
            return None;
        }
        for group in groups {
            let Some(translations) = group.get("tr").and_then(Value::as_array) else {
                continue;
            };
            if translations.len() > 64 {
                return None;
            }
            for translation in translations {
                let Some(label) = translation.get("l") else {
                    continue;
                };
                if label
                    .get("#tran")
                    .and_then(Value::as_str)
                    .is_some_and(crate::dictionary::foreign_language_gloss)
                {
                    continue;
                }
                let Some(english) = label.get("i").and_then(english_fragments) else {
                    continue;
                };
                let explanation = label
                    .get("#tran")
                    .and_then(Value::as_str)
                    .and_then(short_explanation)
                    .unwrap_or_default();
                let part_of_speech = label
                    .get("pos")
                    .and_then(Value::as_str)
                    .and_then(canonical_pos)
                    .map(str::to_owned);
                if !seen.insert((english.to_ascii_lowercase(), part_of_speech.clone())) {
                    continue;
                }
                senses.push(CachedSense {
                    english,
                    explanation,
                    part_of_speech,
                });
                if senses.len() == MAX_SENSES {
                    return Some(CachedEntry {
                        version: 1,
                        query: query.to_owned(),
                        senses,
                    });
                }
            }
        }
    }
    (!senses.is_empty()).then(|| CachedEntry {
        version: 1,
        query: query.to_owned(),
        senses,
    })
}

fn english_fragments(value: &Value) -> Option<String> {
    let fragments = value.as_array()?;
    if fragments.is_empty() || fragments.len() > 64 {
        return None;
    }
    let mut joined = String::new();
    for fragment in fragments {
        let fragment = fragment
            .as_str()
            .or_else(|| fragment.get("#text").and_then(Value::as_str))?;
        if joined.len().checked_add(fragment.len())? > MAX_RAW_FIELD_BYTES {
            return None;
        }
        // Literal spaces between linked words are part of the source. Joining
        // only #text objects would silently turn "how do you do" into one word.
        joined.push_str(fragment);
    }
    if crate::dictionary::foreign_language_gloss(&joined) {
        return None;
    }
    let plain = plain_text(&joined)?.replace('’', "'");
    // CE may prefix its linked English candidate with a domain label such as
    // "[金融] bank". The domain label is metadata rather than part of the word.
    let english = crate::dictionary::strip_annotation(&plain).to_owned();
    valid_english(&english).then_some(english)
}

fn valid_english(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_ENGLISH_CHARS
        && text.trim() == text
        && !text.contains("  ")
        && text.bytes().any(|c| c.is_ascii_alphabetic())
        && text
            .bytes()
            .all(|c| c.is_ascii_alphabetic() || matches!(c, b' ' | b'-' | b'\''))
}

fn canonical_pos(value: &str) -> Option<&'static str> {
    Some(
        match value
            .trim()
            .trim_end_matches('.')
            .to_ascii_lowercase()
            .as_str()
        {
            "a" | "adj" => "adj.",
            "n" => "n.",
            "v" => "v.",
            "vt" => "vt.",
            "vi" => "vi.",
            "adv" | "ad" => "adv.",
            "prep" => "prep.",
            "pron" => "pron.",
            "conj" => "conj.",
            "interj" | "int" => "interj.",
            "art" => "art.",
            "det" => "det.",
            "num" => "num.",
            "aux" => "aux.",
            "abbr" => "abbr.",
            "modal" => "modal.",
            _ => return None,
        },
    )
}

fn short_explanation(raw: &str) -> Option<String> {
    let plain = plain_text(raw)?;
    if !plain.chars().any(is_han) {
        return None;
    }
    let mut explanation = plain
        .split([';', '；', '。'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join("；");
    if explanation.chars().count() > MAX_EXPLANATION_CHARS {
        explanation = explanation
            .chars()
            .take(MAX_EXPLANATION_CHARS - 1)
            .collect();
        explanation.push('…');
    }
    (!explanation.is_empty()).then_some(explanation)
}

fn plain_text(raw: &str) -> Option<String> {
    if raw.len() > MAX_RAW_FIELD_BYTES || raw.chars().any(|c| c.is_control() && !c.is_whitespace())
    {
        return None;
    }
    let decoded = html_escape::decode_html_entities(raw);
    let mut output = String::new();
    let mut in_tag = false;
    for c in decoded.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(c),
            _ => {}
        }
    }
    let output = output.split_whitespace().collect::<Vec<_>>().join(" ");
    (!output.is_empty()
        && !output
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>')))
    .then_some(output)
}

fn make_entry(cached: &CachedEntry, cancelled: &dyn Fn() -> bool) -> Option<Entry> {
    let mut entry = Entry {
        word: cached.query.clone(),
        direction: LookupDirection::ChineseToEnglish,
        phonetic: String::new(),
        parts_of_speech: Vec::new(),
        senses: Vec::with_capacity(cached.senses.len()),
    };
    for sense in &cached.senses {
        if cancelled() {
            return None;
        }
        if crate::dictionary::foreign_language_gloss(&sense.explanation)
            || crate::dictionary::foreign_candidate(&sense.english)
        {
            continue;
        }
        let parts = sense.part_of_speech.clone().map_or_else(
            || crate::dictionary::chinese_candidate_parts_of_speech(&sense.english, &cached.query),
            |part| vec![part],
        );
        crate::dictionary::append_chinese_candidate(&mut entry, &sense.english, &parts);
    }
    (!cancelled() && !entry.senses.is_empty()).then_some(entry)
}

fn cache_path(query: &str) -> PathBuf {
    crate::storage::data_dir()
        .join("dictionary-zh")
        .join(format!(
            "{}.json",
            hex::encode(Sha256::digest(query.as_bytes()))
        ))
}

fn valid_cache(entry: &CachedEntry, query: &str) -> bool {
    if entry.version != 1
        || entry.query != query
        || chinese_word(&entry.query).is_none()
        || entry.senses.is_empty()
        || entry.senses.len() > MAX_SENSES
    {
        return false;
    }
    let mut seen = HashSet::new();
    entry.senses.iter().all(|sense| {
        valid_english(&sense.english)
            && seen.insert((
                sense.english.to_ascii_lowercase(),
                sense.part_of_speech.clone(),
            ))
            && sense
                .part_of_speech
                .as_deref()
                .is_none_or(|part| canonical_pos(part) == Some(part))
            && (sense.explanation.is_empty()
                || (sense.explanation.chars().count() <= MAX_EXPLANATION_CHARS
                    && sense.explanation.chars().any(is_han)
                    && plain_text(&sense.explanation).as_deref()
                        == Some(sense.explanation.as_str())))
    })
}

fn read_cache(path: &Path, query: &str, cancelled: &dyn Fn() -> bool) -> Option<CachedEntry> {
    // The cache is optional, so a concurrent cache operation never makes a
    // dictionary request wait on a lock. Renames publish only complete files.
    let _guard = CACHE_LOCK.try_lock().ok()?;
    let metadata = fs::symlink_metadata(path).ok()?;
    if cancelled() || !metadata.is_file() || metadata.len() > MAX_CACHE_BYTES as u64 {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let bytes = read_limited(&mut file, MAX_CACHE_BYTES, cancelled)?;
    let cached: CachedEntry = serde_json::from_slice(&bytes).ok()?;
    (valid_cache(&cached, query) && !cancelled()).then_some(cached)
}

fn save_cache(path: &Path, entry: &CachedEntry, cancelled: &dyn Fn() -> bool) {
    if cancelled() || !valid_cache(entry, &entry.query) {
        return;
    }
    let Ok(bytes) = serde_json::to_vec(entry) else {
        return;
    };
    if bytes.len() > MAX_CACHE_BYTES {
        return;
    }
    let Ok(_guard) = CACHE_LOCK.try_lock() else {
        return;
    };
    let Some(directory) = path.parent() else {
        return;
    };
    if cancelled() || fs::create_dir_all(directory).is_err() {
        return;
    }
    let temporary = path.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        for chunk in bytes.chunks(8 * 1024) {
            if cancelled() {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            file.write_all(chunk)?;
        }
        drop(file);
        if cancelled() {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        // A unique temporary name also prevents colliding writes from separate
        // application processes. No partially written JSON is ever published.
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{cell::Cell, io::Cursor};

    // Same mixed string/object schema returned by the live jsonapi ce endpoint.
    fn greeting_fixture() -> Value {
        json!({"ce": {"word": [{
            "return-phrase": {"l": {"i": "你好"}},
            "trs": [
                {"tr": [{"l": {"i": ["", {"#text": "hello", "@href": "app:ds:hello"}],
                    "#tran": "喂，你好（用于问候或打招呼）；喂，你好（打电话时的招呼语）；喂，你好（引起别人注意的招呼语）；<非正式>喂，嘿；招呼，问候；"}}]},
                {"tr": [{"l": {"i": ["", {"#text": "hi", "@href": "app:ds:hi"}],
                    "#tran": "嗨！（表示问候或用以唤起注意）；（口语）一声“嗨”，问候；"}}]},
                {"tr": [{"l": {"i": ["", {"#text": "how"}, " ", {"#text": "do"}, " ", {"#text": "you"}, " ", {"#text": "do"}],
                    "#tran": "你好；"}}]}
            ]
        }]}})
    }

    #[test]
    fn real_schema_preserves_phrase_spaces_and_shortens_source_explanations() {
        let parsed = parse_response("你好", &greeting_fixture()).unwrap();
        assert_eq!(parsed.senses.len(), 3);
        assert_eq!(parsed.senses[0].english, "hello");
        assert_eq!(parsed.senses[1].english, "hi");
        assert_eq!(parsed.senses[2].english, "how do you do");
        assert_eq!(parsed.senses[2].explanation, "你好");
        assert_eq!(parsed.senses[0].explanation.split('；').count(), 3);
        assert!(valid_cache(&parsed, "你好"));
        assert_eq!(
            short_explanation("<b>你好</b> &amp; 问候").unwrap(),
            "你好 & 问候"
        );
        assert!(
            short_explanation(&"汉".repeat(300))
                .unwrap()
                .chars()
                .count()
                <= 100
        );
    }

    #[test]
    fn real_sad_schema_uses_explicit_source_pos_and_rejects_unrecognized_pos() {
        let mut fixture = json!({"ce": {"word": [{
            "return-phrase": {"l": {"i": "伤心"}},
            "trs": [
                {"tr": [{"l": {"pos": "adj.", "i": ["", {"#text": "sad", "@action": "link", "@href": "app:ds:sad"}],
                    "#tran": "伤心的，难过的；令人伤心的，让人难过的；糟糕的，不能令人接受的；不幸的，可怜的；"}}]},
                {"tr": [{"l": {"pos": "adj.", "i": ["", {"#text": "heart-broken", "@action": "link", "@href": "app:ds:heart-broken"}],
                    "#tran": "极其伤心的，心碎的；"}}]},
                {"tr": [{"l": {"pos": "adj.", "i": ["", {"#text": "grieved", "@action": "link", "@href": "app:ds:grieved"}],
                    "#tran": "伤心的，悲痛的；感到悲痛，哀悼（grieve 的过去式和过去分词）；使悲伤，使痛心；"}}]}
            ]
        }]}});
        let parsed = parse_response("伤心", &fixture).unwrap();
        assert_eq!(parsed.senses[0].english, "sad");
        assert_eq!(parsed.senses[0].part_of_speech.as_deref(), Some("adj."));
        let entry = make_entry(&parsed, &|| false).unwrap();
        assert_eq!(entry.parts_of_speech, ["adj."]);
        assert!(
            entry
                .senses
                .iter()
                .all(|sense| sense.part_of_speech == "adj.")
        );
        fixture["ce"]["word"][0]["trs"][0]["tr"][0]["l"]["pos"] = json!("invented");
        assert!(
            parse_response("伤心", &fixture).unwrap().senses[0]
                .part_of_speech
                .is_none()
        );
    }

    #[test]
    fn suggestions_empty_results_and_oversized_fields_are_rejected() {
        assert!(parse_response("您好", &greeting_fixture()).is_none());
        assert!(parse_response("你好", &json!({"ce": {"word": []}})).is_none());
        let mut fixture = greeting_fixture();
        fixture["ce"]["word"][0]["trs"] = json!([]);
        assert!(parse_response("你好", &fixture).is_none());
        assert!(english_fragments(&json!([{"#text": "a".repeat(101)}])).is_none());
        assert!(short_explanation(&"汉".repeat(MAX_RAW_FIELD_BYTES)).is_none());
        for text in [
            "",
            "hello",
            "你好 hello",
            "你好，你好吗",
            "今天去公园。",
            "这是一个超过十六个汉字的完整中文句子",
        ] {
            assert!(chinese_word(text).is_none(), "{text}");
        }
    }

    #[test]
    fn repeated_english_headwords_are_deduplicated() {
        let mut fixture = greeting_fixture();
        let mut duplicate = fixture["ce"]["word"][0]["trs"][0].clone();
        duplicate["tr"][0]["l"]["i"] = json!(["", {"#text": "HELLO"}]);
        fixture["ce"]["word"][0]["trs"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert_eq!(parse_response("你好", &fixture).unwrap().senses.len(), 3);
    }

    #[test]
    fn candidate_labels_keep_english_words_and_discard_foreign_language_metadata() {
        assert_eq!(
            english_fragments(&json!(["[金融] ", {"#text": "bank"}])).as_deref(),
            Some("bank")
        );
        assert_eq!(english_fragments(&json!(["", {"#text": "how"}, " ", {"#text": "do"}, " ", {"#text": "you"}, " ", {"#text": "do"}])).as_deref(), Some("how do you do"));
        assert!(english_fragments(&json!(["（法）bonjour"])).is_none());
        assert!(english_fragments(&json!(["<日> sayonara"])).is_none());
        let fixture = json!({"ce": {"word": [{
            "return-phrase": {"l": {"i": "你好"}},
            "trs": [{"tr": [
                {"l": {"pos": "interj.", "i": ["hello"], "#tran": "你好"}},
                {"l": {"i": ["bonjour"], "#tran": "（法）你好"}},
                {"l": {"i": ["ciao"], "#tran": "（意）你好；再见"}}
            ]}]
        }]}});
        let entry = make_entry(&parse_response("你好", &fixture).unwrap(), &|| false).unwrap();
        assert_eq!(entry.senses.len(), 1);
        assert_eq!(entry.senses[0].definition, "hello");
    }

    #[test]
    fn english_alternatives_do_not_require_chinese_glosses_and_merge_source_pos() {
        let fixture = json!({"ce": {"word": [
            {
                "return-phrase": {"l": {"i": "爱"}},
                "trs": [{"tr": [
                    {"l": {"pos": "n.", "i": [{"#text": "love"}]}},
                    {"l": {"pos": "vt.", "i": ["love"]}},
                    {"l": {"pos": "n.", "i": ["LOVE"]}},
                    {"l": {"pos": "n.", "i": ["affection"], "#tran": "感情；喜爱；影响；疾病；"}}
                ]}]
            },
            {
                "return-phrase": {"l": {"i": "可爱的"}},
                "trs": [{"tr": [{"l": {"pos": "adj.", "i": ["lovely"], "#tran": "可爱的"}}]}]
            }
        ]}});
        let parsed = parse_response("爱", &fixture).unwrap();
        assert!(valid_cache(&parsed, "爱"));
        let entry = make_entry(&parsed, &|| false).unwrap();
        assert_eq!(entry.word, "爱");
        assert_eq!(entry.direction, LookupDirection::ChineseToEnglish);
        assert_eq!(entry.senses.len(), 2);
        assert_eq!(entry.senses[0].definition, "love");
        assert_eq!(entry.senses[0].part_of_speech, "n. / vt.");
        assert_eq!(entry.senses[1].definition, "affection");
        assert_eq!(entry.senses[1].part_of_speech, "n.");
        assert_eq!(entry.parts_of_speech, ["n.", "vt."]);
        assert!(parse_response("爱情", &fixture).is_none());
    }

    #[test]
    fn existing_cache_glosses_do_not_turn_into_displayed_chinese_definitions() {
        let cached: CachedEntry = serde_json::from_value(json!({
            "version": 1,
            "query": "蛇",
            "senses": [
                {"english": "snake", "explanation": "蛇；阴险的人；蛇形运动", "part_of_speech": "n."},
                {"english": "serpent", "part_of_speech": "n."}
            ]
        })).unwrap();
        assert!(valid_cache(&cached, "蛇"));
        let entry = make_entry(&cached, &|| false).unwrap();
        assert_eq!(
            entry
                .senses
                .iter()
                .map(|sense| sense.definition.as_str())
                .collect::<Vec<_>>(),
            ["snake", "serpent"]
        );
        assert!(
            entry
                .senses
                .iter()
                .all(|sense| sense.part_of_speech == "n.")
        );
    }

    #[test]
    fn cache_checks_query_sense_count_and_display_fields() {
        let mut cached = parse_response("你好", &greeting_fixture()).unwrap();
        assert!(!valid_cache(&cached, "您好"));
        cached.senses[0].explanation = "<b>你好</b>".into();
        assert!(!valid_cache(&cached, "你好"));
        cached.senses[0].explanation = "汉".repeat(MAX_EXPLANATION_CHARS + 1);
        assert!(!valid_cache(&cached, "你好"));
        cached.senses = (0..MAX_SENSES + 1)
            .map(|_| CachedSense {
                english: "hello".into(),
                explanation: "你好".into(),
                part_of_speech: None,
            })
            .collect();
        assert!(!valid_cache(&cached, "你好"));
        cached.senses.clear();
        assert!(!valid_cache(&cached, "你好"));
    }

    #[test]
    fn chunk_reader_limits_size_and_checks_cancellation_after_read() {
        assert!(read_limited(&mut Cursor::new(vec![0; 9]), 8, &|| false).is_none());
        assert_eq!(
            read_limited(&mut Cursor::new(b"ok"), 8, &|| false).unwrap(),
            b"ok"
        );
        let calls = Cell::new(0);
        let cancelled = || {
            calls.set(calls.get() + 1);
            calls.get() >= 2
        };
        assert!(read_limited(&mut Cursor::new(b"ok"), 8, &cancelled).is_none());
    }

    #[test]
    fn successful_entry_is_reusable_without_a_network_request() {
        let path = std::env::temp_dir().join(format!(
            "translation-dictionary-zh-test-{}-{}.json",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let cached = parse_response("你好", &greeting_fixture()).unwrap();
        save_cache(&path, &cached, &|| false);
        let loaded = read_cache(&path, "你好", &|| false).unwrap();
        assert_eq!(loaded.senses[2].english, "how do you do");
        assert!(read_cache(&path, "您好", &|| false).is_none());
        assert!(read_cache(&path, "你好", &|| true).is_none());
        fs::remove_file(path).unwrap();
    }

    #[test]
    #[ignore = "requires live Youdao dictionary network access"]
    fn live_network_dictionary_returns_real_greetings_and_sad_and_reuses_temporary_cache() {
        for (query, expected) in [
            ("你好", &["hello", "hi", "how do you do"][..]),
            ("伤心", &["sad", "heart-broken", "grieved"][..]),
            ("蛇", &["snake", "serpent"][..]),
            ("银行", &["bank", "banking house"][..]),
        ] {
            let fetched = fetch_entry(query, &|| false).expect(query);
            for english in expected {
                assert!(
                    fetched.senses.iter().any(|sense| sense.english == *english),
                    "{query} did not return {english}"
                );
            }
            let path = std::env::temp_dir().join(format!(
                "translation-dictionary-zh-live-test-{}-{}.json",
                std::process::id(),
                TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            save_cache(&path, &fetched, &|| false);
            let cached = read_cache(&path, query, &|| false).expect("temporary offline cache");
            assert_eq!(cached.senses.len(), fetched.senses.len());
            assert_eq!(cached.senses[0].english, fetched.senses[0].english);
            let entry = make_entry(&cached, &|| false).expect("Chinese dictionary entry");
            assert_eq!(entry.word, query);
            assert_eq!(entry.direction, LookupDirection::ChineseToEnglish);
            assert_eq!(entry.senses.len(), fetched.senses.len());
            assert!(
                entry.senses.iter().all(|sense| !matches!(
                    sense.definition.as_str(),
                    "Bonjour" | "bonjour" | "ciao"
                ))
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
            fs::remove_file(path).unwrap();
        }
    }
}
