use crate::{model::*, offline, storage};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

#[derive(Debug)]
enum Failure {
    Network(String),
    Api(String),
}
impl Failure {
    fn message(&self) -> &str {
        match self {
            Self::Network(v) | Self::Api(v) => v,
        }
    }
}
static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
fn client() -> &'static reqwest::blocking::Client {
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_millis(1500))
            .timeout(Duration::from_secs(6))
            .pool_idle_timeout(Duration::from_secs(90))
            .user_agent("TranslationTool/0.1")
            .build()
            .expect("HTTP client initialization failed")
    })
}
fn send(request: reqwest::blocking::RequestBuilder) -> Result<Value, Failure> {
    let response = request.send().map_err(|_| {
        storage::diagnostic(json!({"event":"http_error", "reason":"connection_or_timeout"}));
        Failure::Network("网络连接失败或请求超时".into())
    })?;
    let status = response.status();
    storage::diagnostic(
        json!({"event":"http_response", "host":response.url().host_str(), "status":status.as_u16()}),
    );
    let body: Value = response
        .json()
        .map_err(|_| Failure::Api(format!("服务返回了无效数据（HTTP {status}）")))?;
    if !status.is_success() {
        return Err(Failure::Api(format!(
            "服务请求失败（HTTP {status}），请检查密钥和配额"
        )));
    }
    Ok(body)
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
    text: &str,
    from: &str,
    to: &str,
    settings: &Settings,
) -> Result<(String, String), Failure> {
    let p = settings.provider;
    let cred = settings.credential();
    if p.needs_key()
        && (cred.key.trim().is_empty() || (p != Provider::DeepL && cred.id.trim().is_empty()))
    {
        return Err(Failure::Api(format!(
            "请先在设置中填写{}的 API 凭据",
            p.name()
        )));
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
            )?;
            let r = &value["Response"];
            if let Some(code) = r["Error"]["Code"].as_str() {
                storage::diagnostic(json!({"event":"provider_error", "provider":p, "code":code}));
                return Err(Failure::Api(format!(
                    "腾讯翻译：{code} · {}",
                    r["Error"]["Message"].as_str().unwrap_or("请求失败")
                )));
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
            )?;
            if let Some(code) = value["error_code"].as_str() {
                return Err(Failure::Api(format!(
                    "百度翻译：{code} · {}",
                    value["error_msg"].as_str().unwrap_or("请求失败")
                )));
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
                from.into(),
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
            let value = send(client().post("https://openapi.youdao.com/api").form(&[
                ("q", text),
                ("from", mapped(p, from)),
                ("to", mapped(p, to)),
                ("appKey", cred.id.trim()),
                ("salt", &salt),
                ("sign", &sign),
                ("signType", "v3"),
                ("curtime", &curtime),
            ]))?;
            if value["errorCode"].as_str() != Some("0") {
                return Err(Failure::Api(format!("有道翻译：{}", value["errorCode"])));
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
                return Err(Failure::Api(
                    "免密钥试用单次限 450 个 UTF-8 字节，请缩短文本或选择其他服务".into(),
                ));
            }
            let actual = if from == "auto" { detect(text) } else { from };
            let value = send(
                client()
                    .get("https://api.mymemory.translated.net/get")
                    .query(&[("q", text), ("langpair", &format!("{actual}|{to}"))]),
            )?;
            if value["responseStatus"].as_i64() != Some(200) {
                return Err(Failure::Api(
                    value["responseDetails"]
                        .as_str()
                        .unwrap_or("试用服务额度已用完")
                        .into(),
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
        return Err(Failure::Api("服务没有返回译文".into()));
    }
    Ok((translated, detected))
}
static CACHE: OnceLock<Mutex<HashMap<String, Translation>>> = OnceLock::new();
#[cfg(test)]
pub fn translate(
    text: String,
    from: String,
    to: String,
    settings: Settings,
) -> Result<Translation, String> {
    translate_cancellable(text, from, to, settings, &|| false)
}
pub fn translate_cancellable(
    text: String,
    from: String,
    to: String,
    settings: Settings,
    cancelled: &dyn Fn() -> bool,
) -> Result<Translation, String> {
    if cancelled() {
        return Err("已取消".into());
    }
    let text = text.trim();
    if text.is_empty() {
        return Err("请输入要翻译的文本".into());
    }
    if text.chars().count() > 5000 {
        return Err("单次最多支持 5000 个字符".into());
    }
    if from == to {
        return Err("请选择不同的源语言和目标语言".into());
    }
    let credential = settings.credential();
    let credentials_present = !settings.provider.needs_key()
        || (!credential.key.trim().is_empty()
            && (settings.provider == Provider::DeepL || !credential.id.trim().is_empty()));
    let key = format!(
        "{:?}|{}|{}|{}|{}|{from}|{to}|{text}",
        settings.provider,
        settings.region,
        settings.deepl_free,
        settings.auto_offline,
        digest(format!("{}:{}", credential.id, credential.key))
    );
    storage::diagnostic(
        json!({"event":"translation_start", "selected_provider":settings.provider, "credentials_present":credentials_present, "from":from, "to":to, "characters":text.chars().count()}),
    );
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(mut cached) = cache.lock().unwrap().get(&key).cloned() {
        cached.note = "来自本次会话缓存".into();
        cached.elapsed_ms = 0;
        storage::diagnostic(
            json!({"event":"translation_result", "selected_provider":settings.provider, "actual_provider":cached.provider, "cache_hit":true}),
        );
        return Ok(cached);
    }
    let start = Instant::now();
    let actual = if from == "auto" { detect(text) } else { &from };
    let mut provider = settings.provider;
    let mut note = String::new();
    let (target, detected) = if provider == Provider::Offline {
        (
            offline::translate_cancellable(text, actual, &to, cancelled)
                .map_err(|e| format!("离线翻译失败：{e}"))?,
            actual.to_owned(),
        )
    } else {
        let cred = settings.credential();
        let no_credentials = provider.needs_key()
            && (cred.key.trim().is_empty()
                || (provider != Provider::DeepL && cred.id.trim().is_empty()));
        let result = if no_credentials && settings.auto_offline {
            Err(Failure::Network("尚未配置 API，使用本地离线翻译".into()))
        } else {
            online(text, &from, &to, &settings)
        };
        match result {
            Ok(v) => v,
            Err(Failure::Network(reason)) if settings.auto_offline => {
                storage::diagnostic(
                    json!({"event":"offline_fallback", "selected_provider":settings.provider, "reason":if no_credentials { "credentials_missing" } else { "network_unavailable" }, "online_attempted":!no_credentials}),
                );
                let translated = offline::translate_cancellable(text, actual, &to, cancelled)
                    .map_err(|e| format!("{reason}；离线翻译不可用：{e}"))?;
                provider = Provider::Offline;
                note = reason;
                (translated, actual.to_owned())
            }
            Err(e) => return Err(e.message().to_owned()),
        }
    };
    if cancelled() {
        return Err("已取消".into());
    }
    let translation = Translation {
        source: text.into(),
        target,
        from: detected,
        to,
        provider,
        elapsed_ms: start.elapsed().as_millis() as u64,
        created: chrono::Local::now().format("%m-%d %H:%M").to_string(),
        starred: false,
        note,
    };
    storage::diagnostic(
        json!({"event":"translation_result", "selected_provider":settings.provider, "actual_provider":provider, "cache_hit":false, "elapsed_ms":translation.elapsed_ms, "target_characters":translation.target.chars().count()}),
    );
    // Only cache the selected engine's results so offline fallbacks don't mask later network recovery.
    if provider == settings.provider {
        let mut c = cache.lock().unwrap();
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
    fn missing_credentials_without_fallback_reports_the_selected_service() {
        let settings = Settings {
            provider: Provider::Tencent,
            auto_offline: false,
            ..Settings::default()
        };
        let result = translate("sad".into(), "en".into(), "zh".into(), settings);
        assert!(result.err().unwrap().contains("腾讯翻译"));
    }
    #[test]
    #[ignore = "requires the downloaded local ONNX runtime and models"]
    fn missing_tencent_credentials_returns_true_offline_provenance() {
        let result = translate(
            "sad".into(),
            "en".into(),
            "zh".into(),
            Settings {
                provider: Provider::Tencent,
                ..Settings::default()
            },
        )
        .unwrap();
        println!(
            "selected=Tencent; actual={:?}; note={}; target={}",
            result.provider, result.note, result.target
        );
        assert_eq!(result.provider, Provider::Offline);
        assert!(result.note.contains("尚未配置 API"));
        assert!(
            ["悲伤", "难过", "伤心", "哀伤", "忧愁", "悲哀"]
                .iter()
                .any(|word| result.target.contains(word))
        );
    }
}
