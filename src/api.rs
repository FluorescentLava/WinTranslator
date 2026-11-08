use crate::{
    model::*,
    offline,
    routing::{self, Failure, FailureKind},
    storage,
};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

pub const ONLINE_TEXT_LIMIT: usize = 5_000;
const LONG_LOCAL_NOTICE: &str = "长文本已自动使用本地翻译";
const LONG_FALLBACK_NOTICE: &str = "在线不可用，已使用本地翻译全文";
const PARTIAL_ONLINE_NOTICE: &str = "在线仅翻译前5000字符，后续内容未翻译";

static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
fn client() -> &'static reqwest::blocking::Client {
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_millis(900))
            .timeout(Duration::from_secs(3))
            .pool_idle_timeout(Duration::from_secs(90))
            .user_agent("TranslationTool/0.1")
            .build()
            .expect("HTTP client initialization failed")
    })
}
fn send(request: reqwest::blocking::RequestBuilder, timeout: Duration) -> Result<Value, Failure> {
    let response = request.timeout(timeout).send().map_err(|_| {
        storage::diagnostic(json!({"event":"http_error", "reason":"connection_or_timeout"}));
        Failure::new(FailureKind::Transient, "网络连接失败或请求超时")
    })?;
    let status = response.status();
    storage::diagnostic(
        json!({"event":"http_response", "host":response.url().host_str(), "status":status.as_u16()}),
    );
    let retry_after_secs = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok());
    let body = response.json::<Value>();
    if !status.is_success() {
        let mut failure = http_failure(status.as_u16());
        failure.retry_after_secs = retry_after_secs;
        return Err(failure);
    }
    body.map_err(|_| {
        Failure::new(
            FailureKind::Transient,
            format!("服务返回了无效数据（HTTP {status}）"),
        )
    })
}

fn http_failure(status: u16) -> Failure {
    let (kind, message) = match status {
        402 | 456 => (FailureKind::Quota, "服务免费额度已用完"),
        429 => (FailureKind::RateLimited, "服务请求过于频繁"),
        401 | 403 => (FailureKind::Auth, "服务鉴权失败，请检查 API 凭据"),
        408 | 500..=599 => (FailureKind::Transient, "服务暂不可用或请求超时"),
        _ => (FailureKind::InvalidRequest, "服务不支持本次请求"),
    };
    Failure::new(kind, format!("{message}（HTTP {status}）"))
}

fn provider_failure(provider: Provider, code: &str, message: &str) -> Failure {
    let lower = format!("{code} {message}").to_ascii_lowercase();
    let kind = match provider {
        Provider::Baidu => match code {
            "54004" => FailureKind::Quota,
            "54003" | "54005" => FailureKind::RateLimited,
            "52003" | "54001" | "58000" | "58002" | "90107" => FailureKind::Auth,
            "52001" | "52002" => FailureKind::Transient,
            _ => FailureKind::InvalidRequest,
        },
        Provider::MyMemory
            if lower.contains("daily")
                || lower.contains("quota")
                || lower.contains("used all")
                || lower.contains("next available") =>
        {
            FailureKind::Quota
        }
        Provider::MyMemory if code == "429" || lower.contains("too many") => {
            FailureKind::RateLimited
        }
        Provider::MyMemory if code.starts_with('5') => FailureKind::Transient,
        Provider::MyMemory => FailureKind::InvalidRequest,
        Provider::Tencent if lower.contains("requestlimit") => FailureKind::RateLimited,
        Provider::Tencent
            if lower.contains("limitexceeded")
                || lower.contains("balance")
                || lower.contains("resourceunavailable") =>
        {
            FailureKind::Quota
        }
        Provider::Tencent if lower.contains("authfailure") => FailureKind::Auth,
        Provider::Youdao if code == "401" => FailureKind::Quota,
        Provider::Youdao if matches!(code, "411" | "412") => FailureKind::RateLimited,
        Provider::Youdao if matches!(code, "108" | "110" | "111" | "202" | "203") => {
            FailureKind::Auth
        }
        _ => FailureKind::Transient,
    };
    storage::diagnostic(json!({
        "event":"provider_error", "provider":provider, "code":code, "category":kind
    }));
    Failure::new(kind, format!("{code} · {message}"))
}
fn digest(data: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(data))
}
fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).unwrap();
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}
pub fn tencent_authorization(
    secret_id: &str,
    secret_key: &str,
    payload: &str,
    timestamp: i64,
) -> String {
    let date = chrono::DateTime::from_timestamp(timestamp, 0)
        .unwrap()
        .format("%Y-%m-%d")
        .to_string();
    let canonical = format!(
        "POST\n/\n\ncontent-type:application/json; charset=utf-8\nhost:tmt.tencentcloudapi.com\n\ncontent-type;host\n{}",
        digest(payload)
    );
    let scope = format!("{date}/tmt/tc3_request");
    let to_sign = format!(
        "TC3-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
        digest(canonical)
    );
    let date_key = hmac(format!("TC3{secret_key}").as_bytes(), &date);
    let service_key = hmac(&date_key, "tmt");
    let signing_key = hmac(&service_key, "tc3_request");
    format!(
        "TC3-HMAC-SHA256 Credential={secret_id}/{scope}, SignedHeaders=content-type;host, Signature={}",
        hex::encode(hmac(&signing_key, &to_sign))
    )
}
fn mapped<'a>(provider: Provider, code: &'a str) -> &'a str {
    match (provider, code) {
        (Provider::Baidu, "ja") => "jp",
        (Provider::Baidu, "ko") => "kor",
        (Provider::Baidu, "fr") => "fra",
        (Provider::Baidu, "es") => "spa",
        (Provider::Baidu, "ar") => "ara",
        (Provider::Youdao, "zh") => "zh-CHS",
        _ => code,
    }
}
pub fn youdao_input(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= 20 {
        text.to_owned()
    } else {
        format!(
            "{}{}{}",
            chars[..10].iter().collect::<String>(),
            chars.len(),
            chars[chars.len() - 10..].iter().collect::<String>()
        )
    }
}
fn online(
    p: Provider,
    text: &str,
    from: &str,
    to: &str,
    settings: &Settings,
    timeout: Duration,
) -> Result<(String, String), Failure> {
    let cred = settings
        .credentials
        .get(p.index())
        .cloned()
        .unwrap_or_default();
    if p.needs_key()
        && (cred.key.trim().is_empty() || (p != Provider::DeepL && cred.id.trim().is_empty()))
    {
        return Err(Failure::new(
            FailureKind::Auth,
            format!("请先在设置中填写{}的 API 凭据", p.name()),
        ));
    }
    let timestamp = chrono::Utc::now().timestamp();
    storage::diagnostic(
        json!({"event":"online_attempt", "provider":p, "from":from, "to":to, "characters":text.chars().count()}),
    );
    let (translated, detected) = match p {
        Provider::Tencent => {
            let payload = json!({"SourceText": text, "Source": from, "Target": to, "ProjectId": 0})
                .to_string();
            let auth = tencent_authorization(cred.id.trim(), cred.key.trim(), &payload, timestamp);
            let value = send(
                client()
                    .post("https://tmt.tencentcloudapi.com")
                    .header("Content-Type", "application/json; charset=utf-8")
                    .header("Authorization", auth)
                    .header("X-TC-Action", "TextTranslate")
                    .header("X-TC-Version", "2018-03-21")
                    .header("X-TC-Timestamp", timestamp.to_string())
                    .header("X-TC-Region", &settings.region)
                    .body(payload),
                timeout,
            )?;
            let r = &value["Response"];
            if let Some(code) = r["Error"]["Code"].as_str() {
                return Err(provider_failure(
                    p,
                    code,
                    r["Error"]["Message"].as_str().unwrap_or("请求失败"),
                ));
            }
            storage::diagnostic(
                json!({"event":"provider_response", "provider":p, "parsed_field":"Response.TargetText", "target_characters":r["TargetText"].as_str().unwrap_or("").chars().count(), "request_id":r["RequestId"].as_str()}),
            );
            (
                r["TargetText"].as_str().unwrap_or("").to_owned(),
                r["Source"].as_str().unwrap_or(from).to_owned(),
            )
        }
        Provider::Baidu => {
            let salt = chrono::Utc::now()
                .timestamp_nanos_opt()
                .unwrap_or(timestamp)
                .to_string();
            let sign = format!(
                "{:x}",
                md5::compute(format!("{}{text}{salt}{}", cred.id.trim(), cred.key.trim()))
            );
            let value = send(
                client()
                    .post("https://fanyi-api.baidu.com/api/trans/vip/translate")
                    .form(&[
                        ("q", text),
                        ("from", mapped(p, from)),
                        ("to", mapped(p, to)),
                        ("appid", cred.id.trim()),
                        ("salt", &salt),
                        ("sign", &sign),
                    ]),
                timeout,
            )?;
            if let Some(code) = value["error_code"].as_str() {
                return Err(provider_failure(
                    p,
                    code,
                    value["error_msg"].as_str().unwrap_or("请求失败"),
                ));
            }
            (
                value["trans_result"]
                    .as_array()
                    .map(|v| {
                        v.iter()
                            .filter_map(|i| i["dst"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default(),
                value["from"].as_str().unwrap_or(from).to_owned(),
            )
        }
        Provider::Youdao => {
            let salt = chrono::Utc::now()
                .timestamp_nanos_opt()
                .unwrap_or(timestamp)
                .to_string();
            let curtime = timestamp.to_string();
            let sign = digest(format!(
                "{}{}{salt}{curtime}{}",
                cred.id.trim(),
                youdao_input(text),
                cred.key.trim()
            ));
            let value = send(
                client().post("https://openapi.youdao.com/api").form(&[
                    ("q", text),
                    ("from", mapped(p, from)),
                    ("to", mapped(p, to)),
                    ("appKey", cred.id.trim()),
                    ("salt", &salt),
                    ("sign", &sign),
                    ("signType", "v3"),
                    ("curtime", &curtime),
                ]),
                timeout,
            )?;
            if value["errorCode"].as_str() != Some("0") {
                return Err(provider_failure(
                    p,
                    value["errorCode"].as_str().unwrap_or("unknown"),
                    "请求失败",
                ));
            }
            (
                value["translation"]
                    .as_array()
                    .map(|v| {
                        v.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default(),
                from.into(),
            )
        }
        Provider::DeepL => {
            let url = if settings.deepl_free {
                "https://api-free.deepl.com/v2/translate"
            } else {
                "https://api.deepl.com/v2/translate"
            };
            let mut payload = json!({"text": [text], "target_lang": to.to_uppercase()});
            if from != "auto" {
                payload["source_lang"] = json!(from.to_uppercase());
            }
            let value = send(
                client()
                    .post(url)
                    .header(
                        "Authorization",
                        format!("DeepL-Auth-Key {}", cred.key.trim()),
                    )
                    .json(&payload),
                timeout,
            )?;
            (
                value["translations"][0]["text"]
                    .as_str()
                    .unwrap_or("")
                    .into(),
                value["translations"][0]["detected_source_language"]
                    .as_str()
                    .unwrap_or(from)
                    .to_lowercase(),
            )
        }
        Provider::MyMemory => {
            if text.len() > 450 {
                return Err(Failure::new(
                    FailureKind::InvalidRequest,
                    "免密钥试用单次限 450 个 UTF-8 字节",
                ));
            }
            let actual = if from == "auto" { detect(text) } else { from };
            let value = send(
                client()
                    .get("https://api.mymemory.translated.net/get")
                    .query(&[("q", text), ("langpair", &format!("{actual}|{to}"))]),
                timeout,
            )?;
            if value["responseStatus"].as_i64() != Some(200) {
                return Err(provider_failure(
                    p,
                    &value["responseStatus"].to_string(),
                    value["responseDetails"].as_str().unwrap_or("服务请求失败"),
                ));
            }
            (
                html_escape::decode_html_entities(
                    value["responseData"]["translatedText"]
                        .as_str()
                        .unwrap_or(""),
                )
                .into_owned(),
                actual.into(),
            )
        }
        Provider::Offline => unreachable!(),
    };
    if translated.trim().is_empty() {
        return Err(Failure::new(FailureKind::Transient, "服务没有返回译文"));
    }
    Ok((translated, detected))
}
static CACHE: OnceLock<Mutex<HashMap<String, Translation>>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextRoute {
    Standard,
    FullLocal,
    OnlinePrefix,
}

#[derive(Debug)]
struct TextPolicy<'a> {
    source: &'a str,
    input: &'a str,
    route: TextRoute,
}
impl TextPolicy<'_> {
    fn cache_allowed(&self) -> bool {
        self.route == TextRoute::Standard
    }
    fn fallback_input(&self) -> &str {
        // A failed online attempt must not limit a capable local engine to the
        // prefix that was eligible for the online request.
        self.source
    }
    fn result_note(&self, actual_provider: Provider, note: String) -> String {
        match self.route {
            TextRoute::FullLocal => LONG_LOCAL_NOTICE.into(),
            TextRoute::OnlinePrefix if actual_provider == Provider::Offline => {
                LONG_FALLBACK_NOTICE.into()
            }
            TextRoute::OnlinePrefix if note.is_empty() => PARTIAL_ONLINE_NOTICE.into(),
            TextRoute::OnlinePrefix => format!("{note}；{PARTIAL_ONLINE_NOTICE}"),
            TextRoute::Standard => note,
        }
    }
}

fn choose_text_policy(
    text: &str,
    selected_provider: Provider,
    force_online_for_long_text: bool,
) -> TextPolicy<'_> {
    // The first character beyond the limit gives a safe UTF-8 boundary without
    // allocating a truncated document or imposing a new local document limit.
    let Some((online_end, _)) = text.char_indices().nth(ONLINE_TEXT_LIMIT) else {
        let text = text.trim();
        return TextPolicy {
            source: text,
            input: text,
            route: TextRoute::Standard,
        };
    };
    if !force_online_for_long_text || selected_provider == Provider::Offline {
        TextPolicy {
            source: text,
            input: text,
            route: TextRoute::FullLocal,
        }
    } else {
        TextPolicy {
            source: text,
            input: &text[..online_end],
            route: TextRoute::OnlinePrefix,
        }
    }
}

pub fn translate_cancellable(
    text: String,
    from: String,
    to: String,
    settings: Settings,
    cancelled: &dyn Fn() -> bool,
) -> Result<Translation, String> {
    translate_with_policy(text, from, to, settings, false, cancelled)
}

pub fn translate_with_policy(
    text: String,
    from: String,
    to: String,
    settings: Settings,
    force_online_for_long_text: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<Translation, String> {
    if cancelled() {
        return Err("已取消".into());
    }
    if text.trim().is_empty() {
        return Err("请输入要翻译的文本".into());
    }
    if from == to {
        return Err("请选择不同的源语言和目标语言".into());
    }
    let policy = choose_text_policy(&text, settings.provider, force_online_for_long_text);
    let input = policy.input;
    let credentials_present = routing::has_credentials(&settings, settings.provider);
    let key = policy.cache_allowed().then(|| {
        format!(
            "{:?}|{}|{}|{}|{}|{from}|{to}|{input}",
            settings.provider,
            settings.region,
            settings.deepl_free,
            settings.auto_offline,
            digest(serde_json::to_vec(&settings.credentials).unwrap_or_default())
        )
    });
    storage::diagnostic(
        json!({"event":"translation_start", "selected_provider":settings.provider, "credentials_present":credentials_present, "from":from, "to":to, "characters":policy.source.chars().count(), "input_characters":input.chars().count(), "long_text_route":match policy.route { TextRoute::Standard => "standard", TextRoute::FullLocal => "local_full_document", TextRoute::OnlinePrefix => "online_prefix" }}),
    );
    let cached = key.as_ref().and_then(|key| {
        CACHE
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap()
            .get(key)
            .cloned()
    });
    if let Some(mut cached) = cached {
        if cancelled() {
            return Err("已取消".into());
        }
        cached.note = "来自本次会话缓存".into();
        cached.elapsed_ms = 0;
        storage::diagnostic(
            json!({"event":"translation_result", "selected_provider":settings.provider, "actual_provider":cached.provider, "cache_hit":true}),
        );
        return Ok(cached);
    }
    let start = Instant::now();
    let actual = if from == "auto" { detect(input) } else { &from };
    let mut provider = if policy.route == TextRoute::FullLocal {
        Provider::Offline
    } else {
        settings.provider
    };
    let mut note = String::new();
    let (target, detected) = if provider == Provider::Offline {
        (
            offline::translate_cancellable(input, actual, &to, cancelled)
                .map_err(|e| format!("离线翻译失败：{e}"))?,
            actual.to_owned(),
        )
    } else {
        let result = routing::translate_online(
            input,
            &from,
            &to,
            &settings,
            cancelled,
            |candidate, chunk, timeout| {
                let source = if candidate == Provider::MyMemory && from == "auto" {
                    actual
                } else {
                    &from
                };
                online(candidate, chunk, source, &to, &settings, timeout)
            },
        );
        match result {
            Ok(result) => {
                provider = result.provider;
                note = result.note;
                (result.target, result.detected)
            }
            Err(error) if error.cancelled => return Err("已取消".into()),
            Err(error) if settings.auto_offline => {
                if cancelled() {
                    return Err("已取消".into());
                }
                let reason = error.message();
                storage::diagnostic(
                    json!({"event":"offline_fallback", "selected_provider":settings.provider, "reason":"all_free_online_unavailable"}),
                );
                let fallback_input = policy.fallback_input();
                let fallback_actual = if from == "auto" {
                    detect(fallback_input)
                } else {
                    &from
                };
                let translated =
                    offline::translate_cancellable(fallback_input, fallback_actual, &to, cancelled)
                        .map_err(|e| format!("{reason}；离线翻译不可用：{e}"))?;
                provider = Provider::Offline;
                note = reason;
                (translated, fallback_actual.to_owned())
            }
            Err(error) => return Err(error.message()),
        }
    };
    if cancelled() {
        return Err("已取消".into());
    }
    let translation = Translation {
        source: policy.source.into(),
        target,
        from: detected,
        to,
        provider,
        elapsed_ms: start.elapsed().as_millis() as u64,
        created: chrono::Local::now().format("%m-%d %H:%M").to_string(),
        starred: false,
        note: policy.result_note(provider, note),
    };
    storage::diagnostic(
        json!({"event":"translation_result", "selected_provider":settings.provider, "actual_provider":provider, "cache_hit":false, "elapsed_ms":translation.elapsed_ms, "target_characters":translation.target.chars().count()}),
    );
    // Alternative online engines and offline fallbacks must not hide recovery of
    // the preferred provider on the next translation.
    if let Some(key) = key.filter(|_| provider == settings.provider) {
        let mut c = CACHE
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap();
        if c.len() >= 128 {
            c.clear();
        }
        c.insert(key, translation.clone());
    }
    Ok(translation)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn youdao_v3_truncates_unicode_by_characters() {
        let input = "一二三四五六七八九十一二三四五六七八九十一二三";
        assert_eq!(
            youdao_input(input),
            "一二三四五六七八九十23四五六七八九十一二三"
        );
        assert_eq!(youdao_input("hello"), "hello");
    }
    #[test]
    fn tencent_signature_is_stable_and_payload_sensitive() {
        let a = tencent_authorization("id", "key", "{}", 1551113065);
        assert!(a.starts_with("TC3-HMAC-SHA256 Credential=id/2019-02-25/tmt/tc3_request"));
        assert_eq!(a, tencent_authorization("id", "key", "{}", 1551113065));
        assert_ne!(
            a,
            tencent_authorization("id", "key", "{\"x\":1}", 1551113065)
        );
    }
    #[test]
    fn http_errors_keep_quota_rate_and_auth_distinct() {
        assert_eq!(http_failure(456).kind, FailureKind::Quota);
        assert_eq!(http_failure(429).kind, FailureKind::RateLimited);
        assert_eq!(http_failure(403).kind, FailureKind::Auth);
        assert_eq!(http_failure(503).kind, FailureKind::Transient);
    }
    #[test]
    fn cancelled_translation_makes_no_request() {
        let result = translate_cancellable(
            "sad".into(),
            "en".into(),
            "zh".into(),
            Settings::default(),
            &|| true,
        );
        assert_eq!(result.err().unwrap(), "已取消");
    }
    #[test]
    fn exactly_five_thousand_unicode_characters_keep_the_standard_route() {
        let text = "中🦀".repeat(ONLINE_TEXT_LIMIT / 2);
        for force in [false, true] {
            let policy = choose_text_policy(&text, Provider::MyMemory, force);
            assert_eq!(policy.route, TextRoute::Standard);
            assert_eq!(policy.input, text);
            assert!(policy.cache_allowed());
            assert_eq!(policy.result_note(Provider::MyMemory, String::new()), "");
        }
    }
    #[test]
    fn long_documents_default_to_complete_local_input_without_an_extra_limit() {
        let text = format!("\n  {}\nend\n", "文".repeat(100_000));
        let policy = choose_text_policy(&text, Provider::Baidu, false);
        assert_eq!(policy.route, TextRoute::FullLocal);
        assert_eq!(policy.input, text);
        assert_eq!(policy.source, text);
        assert_eq!(policy.fallback_input(), text);
        assert!(!policy.cache_allowed());
        assert_eq!(
            policy.result_note(Provider::Offline, String::new()),
            LONG_LOCAL_NOTICE
        );
    }
    #[test]
    fn forced_online_uses_a_unicode_safe_prefix_and_preserves_the_document() {
        let prefix = "中🦀".repeat(ONLINE_TEXT_LIMIT / 2);
        let text = format!("{prefix}后续文档\n");
        let policy = choose_text_policy(&text, Provider::Baidu, true);
        assert_eq!(policy.route, TextRoute::OnlinePrefix);
        assert_eq!(policy.input, prefix);
        assert_eq!(policy.input.chars().count(), ONLINE_TEXT_LIMIT);
        assert_eq!(policy.source, text);
        assert!(!policy.cache_allowed());
        assert_eq!(
            policy.result_note(Provider::Baidu, String::new()),
            PARTIAL_ONLINE_NOTICE
        );
        let switched = policy.result_note(Provider::MyMemory, "已切换服务".into());
        assert!(switched.contains("已切换服务"));
        assert!(switched.contains(PARTIAL_ONLINE_NOTICE));
    }
    #[test]
    fn forced_online_failure_falls_back_to_the_whole_document_with_a_truthful_note() {
        let text = format!("{}这是后续内容", "a".repeat(ONLINE_TEXT_LIMIT));
        let policy = choose_text_policy(&text, Provider::MyMemory, true);
        assert_eq!(policy.input.chars().count(), ONLINE_TEXT_LIMIT);
        assert_eq!(policy.fallback_input(), text);
        let note = policy.result_note(Provider::Offline, "在线服务失败".into());
        assert_eq!(note, LONG_FALLBACK_NOTICE);
        assert!(!note.contains("后续内容未翻译"));
    }
    #[test]
    fn explicit_local_selection_keeps_full_document_even_with_online_force() {
        let text = "x".repeat(ONLINE_TEXT_LIMIT + 1);
        let policy = choose_text_policy(&text, Provider::Offline, true);
        assert_eq!(policy.route, TextRoute::FullLocal);
        assert_eq!(policy.input, text);
        assert_eq!(policy.source, text);
    }
}
