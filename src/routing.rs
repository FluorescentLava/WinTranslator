use crate::{model::*, storage};
use chrono::{DateTime, Datelike, FixedOffset, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Quota,
    RateLimited,
    Transient,
    Auth,
    InvalidRequest,
    Storage,
    Cancelled,
}

#[derive(Debug)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
    pub retry_after_secs: Option<u64>,
}
impl Failure {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after_secs: None,
        }
    }
    pub fn cancelled() -> Self {
        Self::new(FailureKind::Cancelled, "已取消")
    }
}

#[derive(Debug)]
pub struct OnlineResult {
    pub target: String,
    pub detected: String,
    pub provider: Provider,
    pub note: String,
}

#[derive(Debug)]
pub struct RouteFailure {
    pub cancelled: bool,
    failures: Vec<(Provider, Failure)>,
}
impl RouteFailure {
    pub fn message(&self) -> String {
        if self.cancelled {
            return "已取消".into();
        }
        let details = self
            .failures
            .iter()
            .map(|(provider, failure)| format!("{}：{}", provider.name(), failure.message))
            .collect::<Vec<_>>()
            .join("；");
        format!("免费在线服务暂不可用：{details}")
    }
}

pub fn has_credentials(settings: &Settings, provider: Provider) -> bool {
    if !provider.needs_key() {
        return true;
    }
    let Some(credential) = settings.credentials.get(provider.index()) else {
        return false;
    };
    !credential.key.trim().is_empty()
        && (provider == Provider::DeepL || !credential.id.trim().is_empty())
}

// Only explicitly free plans and verified, unexpired grants receive requests.
pub fn candidates(settings: &Settings) -> Vec<Provider> {
    candidates_at(settings, Utc::now())
}

fn candidates_at(settings: &Settings, now: DateTime<Utc>) -> Vec<Provider> {
    if settings.provider == Provider::Offline {
        return Vec::new();
    }
    let mut result = Vec::new();
    for provider in [
        settings.provider,
        Provider::Baidu,
        Provider::DeepL,
        Provider::Tencent,
        Provider::Youdao,
        Provider::MyMemory,
    ] {
        if allowance(settings, provider, now).is_ok()
            && has_credentials(settings, provider)
            && !result.contains(&provider)
        {
            result.push(provider);
        }
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllowancePeriod {
    Daily,
    Monthly,
    Lifetime,
    Grant,
}
impl AllowancePeriod {
    pub fn label(self) -> &'static str {
        match self {
            Self::Daily => "今日",
            Self::Monthly => "本月",
            Self::Lifetime => "总量",
            Self::Grant => "试用",
        }
    }
}

#[derive(Clone, Copy)]
struct Allowance {
    characters: u64,
    period: AllowancePeriod,
    utc_offset: i32,
    expires_at: Option<DateTime<Utc>>,
}
impl Allowance {
    fn disabled() -> Self {
        Self {
            characters: 0,
            period: AllowancePeriod::Lifetime,
            utc_offset: 0,
            expires_at: None,
        }
    }
}

fn allowance(
    settings: &Settings,
    provider: Provider,
    now: DateTime<Utc>,
) -> Result<Allowance, Failure> {
    let limits = configured_allowance(settings, provider)?;
    if limits
        .expires_at
        .is_some_and(|expires_at| expires_at <= now)
    {
        return Err(Failure::new(FailureKind::Quota, "免费试用额度已到期"));
    }
    Ok(limits)
}

fn configured_allowance(settings: &Settings, provider: Provider) -> Result<Allowance, Failure> {
    let disabled = |message| Failure::new(FailureKind::Quota, message);
    if matches!(provider, Provider::Tencent | Provider::Youdao) {
        let grant = settings
            .free_grant(provider)
            .ok_or_else(|| disabled("未确认免费试用额度"))?;
        if grant.grant_id.trim().is_empty() {
            return Err(disabled("免费试用编号未填写"));
        }
        if grant.quota_characters == 0 {
            return Err(disabled("已确认的免费试用额度为 0"));
        }
        let expires_at = DateTime::parse_from_rfc3339(grant.expires_at.trim())
            .map_err(|_| disabled("免费试用缺少有效到期时间（须包含时区）"))?
            .with_timezone(&Utc);
        return Ok(Allowance {
            characters: grant.quota_characters,
            period: AllowancePeriod::Grant,
            utc_offset: 0,
            expires_at: Some(expires_at),
        });
    }
    let (characters, period, utc_offset) = match provider {
        Provider::Baidu => (50_000, AllowancePeriod::Monthly, 8 * 3600),
        Provider::DeepL => match settings.effective_deepl_plan() {
            DeepLPlan::Developer => (1_000_000, AllowancePeriod::Lifetime, 0),
            DeepLPlan::LegacyFree => (500_000, AllowancePeriod::Monthly, 0),
            _ => return Err(disabled("付费 DeepL 计划不加入免费路由")),
        },
        Provider::MyMemory => (5_000, AllowancePeriod::Daily, 0),
        _ => return Err(disabled("此服务未启用免费在线路由")),
    };
    Ok(Allowance {
        characters,
        period,
        utc_offset,
        expires_at: None,
    })
}

#[derive(Default, Serialize, Deserialize)]
struct Ledger {
    #[serde(default)]
    accounts: HashMap<String, Usage>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Usage {
    period_start: i64,
    period_end: i64,
    used_characters: u64,
    #[serde(default)]
    blocked_until: i64,
    #[serde(default)]
    blocked_kind: Option<FailureKind>,
    #[serde(default)]
    next_request_ms: i64,
    #[serde(default)]
    grant_credential_fingerprint: Option<String>,
}

pub struct ProviderStatus {
    pub provider: Provider,
    pub label: &'static str,
    pub configured: bool,
    pub eligible: bool,
    pub allowance_characters: u64,
    pub allowance_period: AllowancePeriod,
    pub expires_at: Option<DateTime<Utc>>,
    pub remaining_characters: Option<u64>,
    pub cooldown_seconds: u64,
    pub message: String,
}

// This snapshot is an estimate of this program's reservations, not account-wide
// billing data. Reading it never resets or changes the persisted allowance.
pub fn status(settings: &Settings) -> Vec<ProviderStatus> {
    let path = storage::data_dir().join("free-api-usage.json");
    let ledger: Result<Ledger, String> = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "免费额度记录损坏".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Ledger::default()),
        Err(_) => Err("免费额度记录无法读取".into()),
    };
    status_from_ledger(settings, &ledger, Utc::now())
}

fn status_from_ledger(
    settings: &Settings,
    ledger: &Result<Ledger, String>,
    now: DateTime<Utc>,
) -> Vec<ProviderStatus> {
    [
        Provider::Tencent,
        Provider::Baidu,
        Provider::Youdao,
        Provider::DeepL,
        Provider::MyMemory,
    ]
    .into_iter()
    .map(|provider| {
        let configured = has_credentials(settings, provider);
        let deepl_plan = settings.effective_deepl_plan();
        let (limits, disabled_reason) = match allowance(settings, provider, now) {
            Ok(limits) => (limits, None),
            Err(error) => (Allowance::disabled(), Some(error.message)),
        };
        let mut result = ProviderStatus {
            provider,
            label: if provider == Provider::DeepL {
                deepl_plan.name()
            } else {
                provider.name()
            },
            configured,
            eligible: configured && disabled_reason.is_none(),
            allowance_characters: limits.characters,
            allowance_period: limits.period,
            expires_at: limits.expires_at,
            remaining_characters: None,
            cooldown_seconds: 0,
            message: String::new(),
        };
        if !configured {
            result.message = "尚未配置 API 凭据".into();
            return result;
        }
        if let Some(reason) = disabled_reason {
            result.message = reason;
            return result;
        }
        match ledger {
            Err(error) => result.message = format!("{error}，已跳过在线请求"),
            Ok(ledger) => {
                let account = QuotaStore::account(settings, provider);
                let mut usage = ledger
                    .accounts
                    .get(&account)
                    .cloned()
                    .unwrap_or_else(|| Usage::new(limits, now));
                usage.refresh(limits, now);
                if let Err(error) = usage.bind_grant(settings, provider) {
                    result.eligible = false;
                    result.message = error.message;
                    return result;
                }
                result.remaining_characters = Some(usage.remaining(limits));
                result.cooldown_seconds =
                    usage.blocked_until.saturating_sub(now.timestamp()).max(0) as u64;
                result.message = if let Err(error) = usage.available(limits, now) {
                    error.message
                } else {
                    "本程序估算；包含失败请求的保守预留".into()
                };
            }
        }
        result
    })
    .collect()
}
impl Usage {
    fn new(limits: Allowance, now: DateTime<Utc>) -> Self {
        if matches!(
            limits.period,
            AllowancePeriod::Lifetime | AllowancePeriod::Grant
        ) {
            return Self {
                period_start: 0,
                period_end: limits.expires_at.map_or(i64::MAX, |time| time.timestamp()),
                used_characters: 0,
                blocked_until: 0,
                blocked_kind: None,
                next_request_ms: 0,
                grant_credential_fingerprint: None,
            };
        }
        let offset = FixedOffset::east_opt(limits.utc_offset).unwrap();
        let date = now.with_timezone(&offset).date_naive();
        let start = if limits.period == AllowancePeriod::Monthly {
            date.with_day(1).unwrap()
        } else {
            date
        };
        let end = if limits.period == AllowancePeriod::Monthly {
            if start.month() == 12 {
                chrono::NaiveDate::from_ymd_opt(start.year() + 1, 1, 1).unwrap()
            } else {
                chrono::NaiveDate::from_ymd_opt(start.year(), start.month() + 1, 1).unwrap()
            }
        } else {
            start.succ_opt().unwrap()
        };
        Self {
            period_start: offset
                .from_local_datetime(&start.and_hms_opt(0, 0, 0).unwrap())
                .single()
                .unwrap()
                .timestamp(),
            period_end: offset
                .from_local_datetime(&end.and_hms_opt(0, 0, 0).unwrap())
                .single()
                .unwrap()
                .timestamp(),
            used_characters: 0,
            blocked_until: 0,
            blocked_kind: None,
            next_request_ms: 0,
            grant_credential_fingerprint: None,
        }
    }
    fn refresh(&mut self, limits: Allowance, now: DateTime<Utc>) {
        // Moving the system clock backwards must not grant another allowance.
        if matches!(
            limits.period,
            AllowancePeriod::Daily | AllowancePeriod::Monthly
        ) && now.timestamp() >= self.period_end
        {
            *self = Self::new(limits, now);
        } else if let Some(expires_at) = limits.expires_at {
            // Correcting a grant's quota or expiry never resets its consumption.
            self.period_end = expires_at.timestamp();
        }
    }
    fn bind_grant(&mut self, settings: &Settings, provider: Provider) -> Result<(), Failure> {
        if !matches!(provider, Provider::Tencent | Provider::Youdao) {
            return Ok(());
        }
        if !has_credentials(settings, provider) {
            return Err(Failure::new(FailureKind::Auth, "尚未配置 API 凭据"));
        }
        // Keep usage across key rotation, but never apply an existing grant to
        // another API identity. Grant IDs are unique within each provider.
        let fingerprint = hex::encode(Sha256::digest(
            settings.credentials[provider.index()].id.trim().as_bytes(),
        ));
        match &self.grant_credential_fingerprint {
            Some(bound) if *bound != fingerprint => Err(Failure::new(
                FailureKind::Auth,
                "免费试用额度属于其他 API 标识，请核对账户和试用编号",
            )),
            Some(_) => Ok(()),
            None if self.used_characters > 0 => Err(Failure::new(
                FailureKind::Storage,
                "免费试用的凭据记录缺失，已跳过在线请求",
            )),
            None => {
                self.grant_credential_fingerprint = Some(fingerprint);
                Ok(())
            }
        }
    }
    fn remaining(&self, limits: Allowance) -> u64 {
        limits.characters.saturating_sub(self.used_characters)
    }
    fn available(&self, limits: Allowance, now: DateTime<Utc>) -> Result<(), Failure> {
        if self.blocked_until <= now.timestamp() {
            return Ok(());
        }
        let kind = self.blocked_kind.unwrap_or(FailureKind::Transient);
        let message = match kind {
            FailureKind::Quota
                if matches!(
                    limits.period,
                    AllowancePeriod::Lifetime | AllowancePeriod::Grant
                ) =>
            {
                "总免费额度已用完，不会自动重置"
            }
            FailureKind::Quota => "服务额度已用完，等待下个免费周期",
            FailureKind::RateLimited => "服务正在限流冷却，稍后自动重试",
            FailureKind::Auth => "凭据鉴权失败，暂时跳过此服务，请检查设置",
            _ => "服务暂不可用，正在短暂冷却",
        };
        Err(Failure::new(kind, message))
    }
}

struct QuotaStore {
    directory: PathBuf,
    settings: Settings,
}

#[derive(Debug)]
struct Reservation {
    period_start: i64,
    remaining_characters: u64,
}
impl QuotaStore {
    fn new(directory: PathBuf, settings: &Settings) -> Self {
        Self {
            directory,
            settings: settings.clone(),
        }
    }
    fn allowance(&self, provider: Provider, now: DateTime<Utc>) -> Result<Allowance, Failure> {
        allowance(&self.settings, provider, now)
    }
    fn account(settings: &Settings, provider: Provider) -> String {
        if matches!(provider, Provider::Tencent | Provider::Youdao) {
            let grant_id = settings
                .free_grant(provider)
                .map(|grant| grant.grant_id.trim())
                .unwrap_or_default();
            // Quota, expiry and credentials are deliberately absent from this
            // key: editing them must not create another copy of a free grant.
            return format!(
                "{provider:?}FreeGrant:{}",
                hex::encode(Sha256::digest(grant_id.as_bytes()))
            );
        }
        // Hashes separate different accounts without writing API secrets to disk.
        let identity = settings
            .credentials
            .get(provider.index())
            .map(|credential| {
                if provider == Provider::Baidu {
                    credential.id.trim().to_owned()
                } else {
                    format!("{}:{}", credential.id.trim(), credential.key.trim())
                }
            })
            .unwrap_or_else(|| "keyless".into());
        let label = if provider == Provider::DeepL {
            match settings.effective_deepl_plan() {
                DeepLPlan::Developer => "DeepLDeveloper".to_owned(),
                DeepLPlan::LegacyFree => "DeepL".to_owned(),
                _ => "DeepLPaid".to_owned(),
            }
        } else {
            format!("{provider:?}")
        };
        format!(
            "{label}:{}",
            hex::encode(Sha256::digest(identity.as_bytes()))
        )
    }
    fn io_error(error: impl std::fmt::Display) -> Failure {
        Failure::new(
            FailureKind::Storage,
            format!("无法安全记录免费额度，已跳过在线请求：{error}"),
        )
    }
    fn locked<T>(
        &self,
        operation: impl FnOnce(&mut Ledger) -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        fs::create_dir_all(&self.directory).map_err(Self::io_error)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.directory.join("free-api-usage.lock"))
            .map_err(Self::io_error)?;
        // The separate lock file protects reservations across threads and processes.
        // Its lifetime ends before any network request or pacing wait.
        lock.lock().map_err(Self::io_error)?;
        let path = self.directory.join("free-api-usage.json");
        let mut ledger = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(Self::io_error)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ledger::default(),
            Err(error) => return Err(Self::io_error(error)),
        };
        let result = operation(&mut ledger)?;
        let temp = path.with_extension("tmp");
        let mut file = File::create(&temp).map_err(Self::io_error)?;
        file.write_all(&serde_json::to_vec_pretty(&ledger).map_err(Self::io_error)?)
            .map_err(Self::io_error)?;
        file.sync_all().map_err(Self::io_error)?;
        drop(file);
        fs::rename(temp, path).map_err(Self::io_error)?;
        Ok(result)
    }
    fn reserve(
        &self,
        account: &str,
        provider: Provider,
        text: &str,
        now: DateTime<Utc>,
    ) -> Result<Reservation, Failure> {
        let count = text.chars().count() as u64;
        let limits = self.allowance(provider, now)?;
        self.locked(|ledger| {
            let usage = ledger
                .accounts
                .entry(account.into())
                .or_insert_with(|| Usage::new(limits, now));
            usage.refresh(limits, now);
            usage.bind_grant(&self.settings, provider)?;
            usage.available(limits, now)?;
            let remaining = usage.remaining(limits);
            if count > remaining {
                return Err(Failure::new(
                    FailureKind::Quota,
                    format!("本地记录的免费额度剩余 {remaining} 字符，本次需要 {count} 字符"),
                ));
            }
            // A failed or cancelled HTTP request may still be billed upstream.
            // Keep reservations conservative rather than refunding uncertain usage.
            usage.used_characters += count;
            Ok(Reservation {
                period_start: usage.period_start,
                remaining_characters: count,
            })
        })
    }
    fn request_slot(
        &self,
        account: &str,
        provider: Provider,
        reservation: &mut Reservation,
        now: DateTime<Utc>,
    ) -> Result<u64, Failure> {
        // Revalidate before every chunk, including requests that already have a
        // reservation: a free grant may expire during pacing or translation.
        let limits = self.allowance(provider, now)?;
        self.locked(|ledger| {
            let usage = ledger
                .accounts
                .entry(account.into())
                .or_insert_with(|| Usage::new(limits, now));
            usage.refresh(limits, now);
            usage.bind_grant(&self.settings, provider)?;
            usage.available(limits, now)?;
            // A long translation can straddle a day or month boundary. Reserve
            // its unsent chunks again in the new period before sending them.
            if reservation.period_start != usage.period_start {
                let remaining = usage.remaining(limits);
                if reservation.remaining_characters > remaining {
                    return Err(Failure::new(FailureKind::Quota, "新免费周期的剩余额度不足"));
                }
                usage.used_characters += reservation.remaining_characters;
                reservation.period_start = usage.period_start;
            }
            if provider != Provider::Baidu {
                return Ok(0);
            }
            let wait_ms = usage.next_request_ms.saturating_sub(now.timestamp_millis());
            if wait_ms > 0 {
                Ok(wait_ms as u64)
            } else {
                usage.next_request_ms = now.timestamp_millis() + 1_000;
                Ok(0)
            }
        })
    }
    fn record_failure(
        &self,
        account: &str,
        provider: Provider,
        failure: &Failure,
        now: DateTime<Utc>,
    ) -> Result<(), Failure> {
        // A response can arrive after the grant expires. Still retain a quota
        // failure so extending the configured expiry cannot revive that grant.
        let limits = configured_allowance(&self.settings, provider)?;
        self.locked(|ledger| {
            let usage = ledger
                .accounts
                .entry(account.into())
                .or_insert_with(|| Usage::new(limits, now));
            usage.refresh(limits, now);
            usage.bind_grant(&self.settings, provider)?;
            let blocked_until = match failure.kind {
                FailureKind::Quota if limits.period == AllowancePeriod::Grant => i64::MAX,
                FailureKind::Quota => usage.period_end,
                FailureKind::RateLimited => now
                    .timestamp()
                    .saturating_add(failure.retry_after_secs.unwrap_or(60).clamp(1, 300) as i64),
                FailureKind::Auth => now.timestamp().saturating_add(300),
                FailureKind::Transient => now.timestamp().saturating_add(15),
                _ => now.timestamp(),
            };
            if blocked_until > now.timestamp() && blocked_until > usage.blocked_until {
                usage.blocked_until = blocked_until;
                usage.blocked_kind = Some(failure.kind);
            }
            Ok(())
        })
    }
}

fn chunks(text: &str, provider: Provider) -> Vec<&str> {
    let (limit, bytes) = match provider {
        Provider::MyMemory => (450, true),
        Provider::Baidu => (1_000, false),
        _ => return vec![text],
    };
    let mut remaining = text;
    let mut result = Vec::new();
    while !remaining.is_empty() {
        let mut end = remaining.len();
        let mut count = 0;
        for (index, character) in remaining.char_indices() {
            let next = count + if bytes { character.len_utf8() } else { 1 };
            if next > limit {
                end = index;
                break;
            }
            count = next;
        }
        if end < remaining.len() {
            // Prefer a word or sentence boundary without exceeding the API limit.
            if let Some((index, character)) =
                remaining[..end].char_indices().rev().find(|(index, c)| {
                    *index >= end / 2 && (c.is_whitespace() || "。！？；.!?;".contains(*c))
                })
            {
                end = index + character.len_utf8();
            }
        }
        result.push(&remaining[..end]);
        remaining = &remaining[end..];
    }
    result
}

fn join_parts(parts: &[String], source_chunks: &[&str], to: &str) -> String {
    let mut result = String::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            let previous = source_chunks[index - 1];
            if previous.ends_with('\n') {
                result.push('\n');
            } else if previous.ends_with(char::is_whitespace) || !matches!(to, "zh" | "ja") {
                result.push(' ');
            }
        }
        result.push_str(part.trim());
    }
    result
}

fn online_budget(text: &str) -> Duration {
    let additional_chunks = text.len().saturating_sub(1) / 450;
    Duration::from_secs((6 + additional_chunks as u64).min(30))
}

fn request_timeout(budget: Duration, elapsed: Duration) -> Result<Duration, Failure> {
    let remaining = budget.saturating_sub(elapsed);
    if remaining.is_zero() {
        Err(Failure::new(
            FailureKind::Transient,
            "在线服务等待时间已用完",
        ))
    } else {
        Ok(remaining.min(Duration::from_secs(3)))
    }
}

pub fn translate_online(
    text: &str,
    from: &str,
    to: &str,
    settings: &Settings,
    cancelled: &dyn Fn() -> bool,
    attempt: impl FnMut(Provider, &str, Duration) -> Result<(String, String), Failure>,
) -> Result<OnlineResult, RouteFailure> {
    route_with_store(
        text,
        from,
        to,
        settings,
        cancelled,
        attempt,
        &QuotaStore::new(storage::data_dir(), settings),
        &Utc::now,
        &|event| storage::diagnostic(event),
    )
}

#[allow(clippy::too_many_arguments)]
fn route_with_store(
    text: &str,
    from: &str,
    to: &str,
    settings: &Settings,
    cancelled: &dyn Fn() -> bool,
    mut attempt: impl FnMut(Provider, &str, Duration) -> Result<(String, String), Failure>,
    store: &QuotaStore,
    now: &dyn Fn() -> DateTime<Utc>,
    diagnostic: &dyn Fn(serde_json::Value),
) -> Result<OnlineResult, RouteFailure> {
    let providers = candidates_at(settings, now());
    let mut failures = Vec::new();
    let started = Instant::now();
    let budget = online_budget(text);
    if !providers.contains(&settings.provider) {
        diagnostic(serde_json::json!({
            "event":"online_route_skip", "provider":settings.provider,
            "reason":if !has_credentials(settings, settings.provider) {
                "credentials_missing"
            } else {
                "free_plan_or_valid_grant_missing"
            }
        }));
    }
    for provider in providers {
        if cancelled() {
            return Err(RouteFailure {
                cancelled: true,
                failures,
            });
        }
        if let Err(error) = request_timeout(budget, started.elapsed()) {
            failures.push((provider, error));
            break;
        }
        let account = QuotaStore::account(settings, provider);
        let mut provider_failed = false;
        let result = (|| {
            let mut reservation = store.reserve(&account, provider, text, now())?;
            let source_chunks = chunks(text, provider);
            let mut parts = Vec::new();
            let mut detected = from.to_owned();
            for chunk in &source_chunks {
                loop {
                    if cancelled() {
                        return Err(Failure::cancelled());
                    }
                    request_timeout(budget, started.elapsed())?;
                    let wait_ms =
                        store.request_slot(&account, provider, &mut reservation, now())?;
                    if wait_ms == 0 {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(wait_ms.min(100)));
                }
                if cancelled() {
                    return Err(Failure::cancelled());
                }
                let timeout = request_timeout(budget, started.elapsed())?;
                let (translated, language) =
                    attempt(provider, chunk, timeout).map_err(|failure| {
                        provider_failed = true;
                        failure
                    })?;
                if translated.trim().is_empty() {
                    provider_failed = true;
                    return Err(Failure::new(FailureKind::Transient, "服务没有返回译文"));
                }
                if parts.is_empty() {
                    detected = language;
                }
                reservation.remaining_characters = reservation
                    .remaining_characters
                    .saturating_sub(chunk.chars().count() as u64);
                parts.push(translated);
            }
            Ok((join_parts(&parts, &source_chunks, to), detected))
        })();
        match result {
            Ok((target, detected)) => {
                if cancelled() {
                    return Err(RouteFailure {
                        cancelled: true,
                        failures,
                    });
                }
                let note = if provider != settings.provider {
                    format!(
                        "{}已自动切换为{}（免费在线服务）",
                        settings.provider.name(),
                        provider.name()
                    )
                } else {
                    String::new()
                };
                diagnostic(serde_json::json!({
                    "event":"online_route_success", "selected_provider":settings.provider,
                    "actual_provider":provider, "previous_failures":failures.len()
                }));
                return Ok(OnlineResult {
                    target,
                    detected,
                    provider,
                    note,
                });
            }
            Err(failure) => {
                if failure.kind == FailureKind::Cancelled || cancelled() {
                    return Err(RouteFailure {
                        cancelled: true,
                        failures,
                    });
                }
                diagnostic(serde_json::json!({
                    "event":"online_route_failure", "provider":provider,
                    "category":failure.kind
                }));
                // Reservation failures already describe local state; do not consume
                // the remaining small allowance by blocking the whole period.
                if provider_failed && failure.kind != FailureKind::Storage {
                    if let Err(error) = store.record_failure(&account, provider, &failure, now()) {
                        diagnostic(serde_json::json!({
                            "event":"quota_record_failure", "provider":provider,
                            "category":error.kind
                        }));
                    }
                }
                failures.push((provider, failure));
            }
        }
    }
    Err(RouteFailure {
        cancelled: false,
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestStore(QuotaStore);
    impl TestStore {
        fn new() -> Self {
            Self::with_deepl_plan(DeepLPlan::Developer)
        }
        fn with_deepl_plan(deepl_plan: DeepLPlan) -> Self {
            Self::with_settings(&Settings {
                deepl_plan,
                ..Settings::default()
            })
        }
        fn with_settings(settings: &Settings) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "translation-routing-{}-{}-{}",
                std::process::id(),
                Utc::now().timestamp_nanos_opt().unwrap(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            Self(QuotaStore::new(directory, settings))
        }
    }
    impl Drop for TestStore {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0.directory);
        }
    }
    fn date(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }
    fn configured(selected: Provider) -> Settings {
        let mut settings = Settings {
            provider: selected,
            ..Settings::default()
        };
        settings.credentials[Provider::Baidu.index()] = Credential {
            id: "test-baidu-id".into(),
            key: "test-baidu-key".into(),
        };
        settings.credentials[Provider::DeepL.index()].key = "test-free-key".into();
        settings.credentials[Provider::Tencent.index()] = Credential {
            id: "test-tencent-id".into(),
            key: "test-tencent-key".into(),
        };
        settings.credentials[Provider::Youdao.index()] = Credential {
            id: "test-youdao-id".into(),
            key: "test-youdao-key".into(),
        };
        settings
    }
    fn set_grant(settings: &mut Settings, provider: Provider, grant: Option<FreeGrant>) {
        match provider {
            Provider::Tencent => settings.tencent_free_grant = grant,
            Provider::Youdao => settings.youdao_free_grant = grant,
            _ => unreachable!(),
        }
    }
    fn trial_settings(provider: Provider) -> Settings {
        let mut settings = configured(provider);
        set_grant(
            &mut settings,
            provider,
            Some(FreeGrant {
                grant_id: "confirmed-trial-001".into(),
                quota_characters: 6,
                expires_at: "2027-03-01T00:00:00+08:00".into(),
            }),
        );
        settings
    }
    #[test]
    fn trials_require_verified_quota_id_absolute_expiry_and_credentials() {
        let now = date("2026-11-08T12:00:00Z");
        for provider in [Provider::Tencent, Provider::Youdao] {
            let mut settings = trial_settings(provider);
            assert_eq!(candidates_at(&settings, now)[0], provider);
            let valid = settings.free_grant(provider).unwrap().clone();
            let mut invalid = vec![None, Some(FreeGrant::default())];
            for change in 0..6 {
                let mut grant = valid.clone();
                match change {
                    0 => grant.grant_id = " ".into(),
                    1 => grant.quota_characters = 0,
                    2 => grant.expires_at.clear(),
                    3 => grant.expires_at = "2027-03-01T00:00:00".into(),
                    4 => grant.expires_at = "2026-11-08T11:59:59Z".into(),
                    _ => grant.expires_at = "2026-11-08T12:00:00Z".into(),
                }
                invalid.push(Some(grant));
            }
            for grant in invalid {
                set_grant(&mut settings, provider, grant);
                assert!(!candidates_at(&settings, now).contains(&provider));
                let store = TestStore::with_settings(&settings);
                let account = QuotaStore::account(&settings, provider);
                assert!(store.0.reserve(&account, provider, "x", now).is_err());
                let status = status_from_ledger(&settings, &Ok(Ledger::default()), now);
                let status = status
                    .iter()
                    .find(|status| status.provider == provider)
                    .unwrap();
                assert!(!status.eligible);
                assert!(!status.message.is_empty());
            }
            set_grant(&mut settings, provider, Some(valid));
            settings.credentials[provider.index()].key.clear();
            assert!(!candidates_at(&settings, now).contains(&provider));
            let store = TestStore::with_settings(&settings);
            assert_eq!(
                store
                    .0
                    .reserve("unconfigured", provider, "x", now)
                    .unwrap_err()
                    .kind,
                FailureKind::Auth
            );
        }
    }
    #[test]
    fn trial_expiry_is_rechecked_after_reservation_before_sending() {
        let before = date("2026-11-08T12:00:00Z");
        let expires = before + chrono::Duration::seconds(1);
        for provider in [Provider::Tencent, Provider::Youdao] {
            let mut settings = trial_settings(provider);
            let mut grant = settings.free_grant(provider).unwrap().clone();
            grant.expires_at = expires.to_rfc3339();
            set_grant(&mut settings, provider, Some(grant));
            let store = TestStore::with_settings(&settings);
            let account = QuotaStore::account(&settings, provider);
            let mut reservation = store
                .0
                .reserve(&account, provider, "hello", before)
                .unwrap();
            assert_eq!(
                store
                    .0
                    .request_slot(&account, provider, &mut reservation, before)
                    .unwrap(),
                0
            );
            assert!(
                store
                    .0
                    .request_slot(&account, provider, &mut reservation, expires)
                    .unwrap_err()
                    .message
                    .contains("已到期")
            );
            assert!(store.0.reserve(&account, provider, "x", expires).is_err());
        }
    }
    #[test]
    fn grant_consumption_survives_settings_edits_reopen_and_credential_changes() {
        let now = date("2026-11-30T23:59:59Z");
        for provider in [Provider::Tencent, Provider::Youdao] {
            let mut settings = trial_settings(provider);
            let store = TestStore::with_settings(&settings);
            let account = QuotaStore::account(&settings, provider);
            store.0.reserve(&account, provider, "中🦀ab", now).unwrap();
            let mut grant = settings.free_grant(provider).unwrap().clone();
            grant.quota_characters = 8;
            grant.expires_at = "2028-01-01T00:00:00Z".into();
            set_grant(&mut settings, provider, Some(grant));
            settings.credentials[provider.index()].key = "rotated-secret-key".into();
            assert_eq!(QuotaStore::account(&settings, provider), account);
            let reopened = QuotaStore::new(store.0.directory.clone(), &settings);
            reopened
                .reserve(&account, provider, "abcd", date("2026-12-01T00:00:00Z"))
                .unwrap();
            for now in [date("2026-11-01T00:00:00Z"), date("2027-01-01T00:00:00Z")] {
                assert_eq!(
                    reopened
                        .reserve(&account, provider, "x", now)
                        .unwrap_err()
                        .kind,
                    FailureKind::Quota
                );
            }
            settings.credentials[provider.index()].id = "another-api-identity".into();
            assert_eq!(QuotaStore::account(&settings, provider), account);
            let other_credentials = QuotaStore::new(store.0.directory.clone(), &settings);
            assert_eq!(
                other_credentials
                    .reserve(&account, provider, "x", now)
                    .unwrap_err()
                    .kind,
                FailureKind::Auth
            );
            let bytes = fs::read(store.0.directory.join("free-api-usage.json")).unwrap();
            let ledger: Ledger = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(ledger.accounts[&account].used_characters, 8);
            let serialized = String::from_utf8(bytes).unwrap();
            assert!(!serialized.contains("rotated-secret-key"));
            assert!(!serialized.contains("test-tencent-id"));
            assert!(!serialized.contains("test-youdao-id"));
        }
    }
    #[test]
    fn different_grants_and_providers_have_separate_finite_balances() {
        let now = date("2026-11-08T12:00:00Z");
        let mut settings = trial_settings(Provider::Tencent);
        settings.youdao_free_grant = settings.tencent_free_grant.clone();
        let store = TestStore::with_settings(&settings);
        for provider in [Provider::Tencent, Provider::Youdao] {
            let account = QuotaStore::account(&settings, provider);
            store.0.reserve(&account, provider, "abcdef", now).unwrap();
            assert_eq!(
                store
                    .0
                    .reserve(&account, provider, "x", now)
                    .unwrap_err()
                    .kind,
                FailureKind::Quota
            );
        }
        let first = QuotaStore::account(&settings, Provider::Tencent);
        settings.tencent_free_grant.as_mut().unwrap().grant_id =
            "newly-verified-distinct-grant".into();
        settings.credentials[Provider::Tencent.index()].id = "new-grant-api-identity".into();
        let second = QuotaStore::account(&settings, Provider::Tencent);
        assert_ne!(first, second);
        let reopened = QuotaStore::new(store.0.directory.clone(), &settings);
        reopened
            .reserve(&second, Provider::Tencent, "abcdef", now)
            .unwrap();
        assert!(
            reopened
                .reserve(&second, Provider::Tencent, "x", now)
                .is_err()
        );
        let ledger: Ledger = serde_json::from_slice(
            &fs::read(store.0.directory.join("free-api-usage.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(ledger.accounts[&first].used_characters, 6);
    }
    #[test]
    fn upstream_trial_exhaustion_is_not_cleared_by_expiry_edits_or_month_changes() {
        let before = date("2026-11-30T23:59:59Z");
        for provider in [Provider::Tencent, Provider::Youdao] {
            let mut settings = trial_settings(provider);
            let mut grant = settings.free_grant(provider).unwrap().clone();
            grant.expires_at = (before + chrono::Duration::seconds(1)).to_rfc3339();
            set_grant(&mut settings, provider, Some(grant));
            let store = TestStore::with_settings(&settings);
            let account = QuotaStore::account(&settings, provider);
            store.0.reserve(&account, provider, "x", before).unwrap();
            store
                .0
                .record_failure(
                    &account,
                    provider,
                    &Failure::new(FailureKind::Quota, "trial depleted"),
                    before + chrono::Duration::seconds(2),
                )
                .unwrap();
            let mut grant = settings.free_grant(provider).unwrap().clone();
            grant.quota_characters = 1_000;
            grant.expires_at = "2028-01-01T00:00:00Z".into();
            set_grant(&mut settings, provider, Some(grant));
            let reopened = QuotaStore::new(store.0.directory.clone(), &settings);
            let failure = reopened
                .reserve(&account, provider, "x", date("2027-04-01T00:00:00Z"))
                .unwrap_err();
            assert_eq!(failure.kind, FailureKind::Quota);
            assert!(failure.message.contains("不会自动重置"));
        }
    }
    #[test]
    fn verified_trial_routes_then_switches_online_when_its_balance_is_insufficient() {
        let now = date("2026-11-08T12:00:00Z");
        for provider in [Provider::Tencent, Provider::Youdao] {
            let settings = trial_settings(provider);
            let store = TestStore::with_settings(&settings);
            for expected in [provider, Provider::Baidu] {
                let mut attempted = Vec::new();
                let result = route_with_store(
                    "hello",
                    "en",
                    "zh",
                    &settings,
                    &|| false,
                    |actual, _, _| {
                        attempted.push(actual);
                        Ok(("你好".into(), "en".into()))
                    },
                    &store.0,
                    &|| now,
                    &|_| {},
                )
                .unwrap();
                assert_eq!(attempted, vec![expected]);
                assert_eq!(result.provider, expected);
            }
            let bytes = fs::read(store.0.directory.join("free-api-usage.json")).unwrap();
            let ledger = Ok(serde_json::from_slice(&bytes).unwrap());
            let status = status_from_ledger(&settings, &ledger, now);
            let status = status
                .iter()
                .find(|status| status.provider == provider)
                .unwrap();
            assert!(status.eligible);
            assert_eq!(status.allowance_period, AllowancePeriod::Grant);
            assert_eq!(status.remaining_characters, Some(1));
            assert_eq!(status.expires_at, Some(date("2027-02-28T16:00:00Z")));
            assert!(status.message.contains("估算"));
            assert_eq!(
                fs::read(store.0.directory.join("free-api-usage.json")).unwrap(),
                bytes
            );
        }
    }
    #[test]
    fn preferred_free_provider_then_other_free_services_only() {
        assert_eq!(
            candidates(&configured(Provider::DeepL)),
            vec![Provider::DeepL, Provider::Baidu, Provider::MyMemory]
        );
        assert_eq!(
            candidates(&configured(Provider::MyMemory)),
            vec![Provider::MyMemory, Provider::Baidu, Provider::DeepL]
        );
        assert_eq!(
            candidates(&configured(Provider::Tencent)),
            vec![Provider::Baidu, Provider::DeepL, Provider::MyMemory]
        );
        assert!(candidates(&configured(Provider::Offline)).is_empty());
        let mut paid = configured(Provider::DeepL);
        paid.deepl_free = false;
        paid.deepl_plan = DeepLPlan::Paid;
        assert_eq!(candidates(&paid), vec![Provider::Baidu, Provider::MyMemory]);
        paid.deepl_free = true;
        assert_eq!(candidates(&paid), vec![Provider::Baidu, Provider::MyMemory]);
        paid.deepl_plan = DeepLPlan::Auto;
        paid.deepl_free = false;
        assert_eq!(candidates(&paid), vec![Provider::Baidu, Provider::MyMemory]);
        assert_eq!(
            candidates(&Settings {
                provider: Provider::Baidu,
                ..Settings::default()
            }),
            vec![Provider::MyMemory]
        );
    }
    #[test]
    fn short_text_has_six_seconds_total_and_each_request_uses_remaining_budget() {
        let budget = online_budget("hello");
        assert_eq!(budget, Duration::from_secs(6));
        assert_eq!(
            request_timeout(budget, Duration::ZERO).unwrap(),
            Duration::from_secs(3)
        );
        assert_eq!(
            request_timeout(budget, Duration::from_millis(5_750)).unwrap(),
            Duration::from_millis(250)
        );
        assert!(request_timeout(budget, Duration::from_secs(6)).is_err());
        assert_eq!(online_budget(&"中".repeat(5_000)), Duration::from_secs(30));
    }
    #[test]
    fn quota_persists_across_reopen_counts_unicode_and_resets_daily() {
        let store = TestStore::new();
        let now = date("2026-11-08T23:59:00Z");
        let settings = Settings::default();
        let account = QuotaStore::account(&settings, Provider::MyMemory);
        store
            .0
            .reserve(&account, Provider::MyMemory, &"中🦀".repeat(2_500), now)
            .unwrap();
        let reopened = QuotaStore::new(store.0.directory.clone(), &store.0.settings);
        assert_eq!(
            reopened
                .reserve(&account, Provider::MyMemory, "a", now)
                .unwrap_err()
                .kind,
            FailureKind::Quota
        );
        reopened
            .reserve(
                &account,
                Provider::MyMemory,
                "中文",
                date("2026-11-09T00:00:00Z"),
            )
            .unwrap();
        let ledger: Ledger = serde_json::from_slice(
            &fs::read(store.0.directory.join("free-api-usage.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(ledger.accounts[&account].used_characters, 2);
    }
    #[test]
    fn developer_allowance_persists_across_reopen_months_and_years() {
        let store = TestStore::new();
        let settings = configured(Provider::DeepL);
        let account = QuotaStore::account(&settings, Provider::DeepL);
        let mut reservation = store
            .0
            .reserve(
                &account,
                Provider::DeepL,
                &"中🦀".repeat(250_000),
                date("2026-11-30T23:59:59Z"),
            )
            .unwrap();
        reservation.remaining_characters = 3;
        let reopened = QuotaStore::new(store.0.directory.clone(), &store.0.settings);
        // A lifetime reservation must not be charged again at a month boundary.
        reopened
            .request_slot(
                &account,
                Provider::DeepL,
                &mut reservation,
                date("2026-12-01T00:00:00Z"),
            )
            .unwrap();
        reopened
            .reserve(
                &account,
                Provider::DeepL,
                &"a".repeat(499_999),
                date("2026-12-01T00:00:00Z"),
            )
            .unwrap();
        reopened
            .reserve(
                &account,
                Provider::DeepL,
                "文",
                date("2027-01-01T00:00:00Z"),
            )
            .unwrap();
        for now in [date("2026-11-01T00:00:00Z"), date("2030-01-01T00:00:00Z")] {
            assert_eq!(
                reopened
                    .reserve(&account, Provider::DeepL, "x", now)
                    .unwrap_err()
                    .kind,
                FailureKind::Quota
            );
        }
        let ledger: Ledger = serde_json::from_slice(
            &fs::read(store.0.directory.join("free-api-usage.json")).unwrap(),
        )
        .unwrap();
        let usage = &ledger.accounts[&account];
        assert_eq!(usage.used_characters, 1_000_000);
        assert_eq!(usage.period_start, 0);
        assert_eq!(usage.period_end, i64::MAX);
    }
    #[test]
    fn legacy_free_keeps_old_account_usage_and_resets_at_utc_month_boundary() {
        let store = TestStore::with_deepl_plan(DeepLPlan::LegacyFree);
        let mut settings = configured(Provider::DeepL);
        settings.deepl_plan = DeepLPlan::Auto;
        settings.deepl_free = true;
        let account = QuotaStore::account(&settings, Provider::DeepL);
        assert_eq!(
            account,
            format!("DeepL:{}", hex::encode(Sha256::digest(b":test-free-key")))
        );
        assert!(candidates(&settings).contains(&Provider::DeepL));
        settings.deepl_plan = DeepLPlan::LegacyFree;
        assert_eq!(QuotaStore::account(&settings, Provider::DeepL), account);
        settings.deepl_plan = DeepLPlan::Developer;
        assert_ne!(QuotaStore::account(&settings, Provider::DeepL), account);

        // This is the ledger schema and account label written before plan selection.
        let old_ledger = serde_json::json!({
            "accounts": {
                (account.clone()): {
                    "period_start": date("2026-11-01T00:00:00Z").timestamp(),
                    "period_end": date("2026-12-01T00:00:00Z").timestamp(),
                    "used_characters": 499_999
                }
            }
        });
        fs::create_dir_all(&store.0.directory).unwrap();
        fs::write(
            store.0.directory.join("free-api-usage.json"),
            serde_json::to_vec(&old_ledger).unwrap(),
        )
        .unwrap();
        store
            .0
            .reserve(
                &account,
                Provider::DeepL,
                "中",
                date("2026-11-30T15:59:59Z"),
            )
            .unwrap();
        assert_eq!(
            store
                .0
                .reserve(&account, Provider::DeepL, "x", date("2026-11-30T16:00:00Z"),)
                .unwrap_err()
                .kind,
            FailureKind::Quota
        );
        let reopened = QuotaStore::new(store.0.directory.clone(), &store.0.settings);
        reopened
            .reserve(
                &account,
                Provider::DeepL,
                "中文",
                date("2026-12-01T00:00:00Z"),
            )
            .unwrap();
        let ledger: Ledger = serde_json::from_slice(
            &fs::read(store.0.directory.join("free-api-usage.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(ledger.accounts[&account].used_characters, 2);
        assert_eq!(
            ledger.accounts[&account].period_end,
            date("2027-01-01T00:00:00Z").timestamp()
        );
    }
    #[test]
    fn deepl_quota_failure_cooldown_matches_the_selected_plan_period() {
        let before = date("2026-11-30T23:59:59Z");
        let after = date("2026-12-01T00:00:00Z");
        for plan in [DeepLPlan::Developer, DeepLPlan::LegacyFree] {
            let store = TestStore::with_deepl_plan(plan);
            let mut settings = configured(Provider::DeepL);
            settings.deepl_plan = plan;
            let account = QuotaStore::account(&settings, Provider::DeepL);
            store
                .0
                .reserve(&account, Provider::DeepL, "hello", before)
                .unwrap();
            store
                .0
                .record_failure(
                    &account,
                    Provider::DeepL,
                    &Failure::new(FailureKind::Quota, "server quota exhausted"),
                    before,
                )
                .unwrap();
            assert_eq!(
                store
                    .0
                    .reserve(&account, Provider::DeepL, "x", before)
                    .unwrap_err()
                    .kind,
                FailureKind::Quota
            );
            let reopened = QuotaStore::new(store.0.directory.clone(), &store.0.settings);
            if plan == DeepLPlan::Developer {
                let failure = reopened
                    .reserve(&account, Provider::DeepL, "x", after)
                    .unwrap_err();
                assert_eq!(failure.kind, FailureKind::Quota);
                assert!(failure.message.contains("不会自动重置"));
                assert!(
                    reopened
                        .reserve(&account, Provider::DeepL, "x", date("2030-01-01T00:00:00Z"),)
                        .is_err()
                );
            } else {
                reopened
                    .reserve(&account, Provider::DeepL, "x", after)
                    .unwrap();
            }
        }
    }
    #[test]
    fn baidu_month_changes_at_beijing_midnight_and_clock_rollback_does_not_reset() {
        let store = TestStore::new();
        let settings = configured(Provider::Baidu);
        let account = QuotaStore::account(&settings, Provider::Baidu);
        let now = date("2026-11-30T15:59:59Z");
        store
            .0
            .reserve(&account, Provider::Baidu, &"中".repeat(50_000), now)
            .unwrap();
        assert!(
            store
                .0
                .reserve(&account, Provider::Baidu, "a", date("2026-11-01T00:00:00Z"))
                .is_err()
        );
        store
            .0
            .reserve(&account, Provider::Baidu, "a", date("2026-11-30T16:00:00Z"))
            .unwrap();
    }
    #[test]
    fn concurrent_reservations_cannot_exceed_free_allowance() {
        let store = TestStore::new();
        let now = date("2026-11-08T12:00:00Z");
        let path = store.0.directory.clone();
        let handles = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    QuotaStore::new(path, &Settings::default())
                        .reserve(
                            "same-keyless-account",
                            Provider::MyMemory,
                            &"a".repeat(1_000),
                            now,
                        )
                        .is_ok()
                })
            })
            .collect::<Vec<_>>();
        let successful = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|ok| *ok)
            .count();
        assert_eq!(successful, 5);
    }
    #[test]
    fn exhausted_provider_switches_with_true_provider_provenance() {
        let store = TestStore::new();
        let settings = configured(Provider::Baidu);
        let now = date("2026-11-08T12:00:00Z");
        let account = QuotaStore::account(&settings, Provider::Baidu);
        store
            .0
            .reserve(&account, Provider::Baidu, &"a".repeat(50_000), now)
            .unwrap();
        let mut attempted = Vec::new();
        let result = route_with_store(
            "hello",
            "en",
            "zh",
            &settings,
            &|| false,
            |provider, _, _| {
                attempted.push(provider);
                Ok(("你好".into(), "en".into()))
            },
            &store.0,
            &|| now,
            &|_| {},
        )
        .unwrap();
        assert_eq!(attempted, vec![Provider::DeepL]);
        assert_eq!(result.provider, Provider::DeepL);
        assert_eq!(result.target, "你好");
        assert!(result.note.contains("DeepL"));
    }
    #[test]
    fn baidu_pacing_is_shared_and_waits_one_second_between_requests() {
        let store = TestStore::new();
        let now = date("2026-11-08T12:00:00Z");
        let mut reservation = store
            .0
            .reserve("baidu-test-account", Provider::Baidu, "hello", now)
            .unwrap();
        assert_eq!(
            store
                .0
                .request_slot("baidu-test-account", Provider::Baidu, &mut reservation, now)
                .unwrap(),
            0
        );
        let reopened = QuotaStore::new(store.0.directory.clone(), &store.0.settings);
        assert_eq!(
            reopened
                .request_slot(
                    "baidu-test-account",
                    Provider::Baidu,
                    &mut reservation,
                    now + chrono::Duration::milliseconds(100)
                )
                .unwrap(),
            900
        );
        assert_eq!(
            reopened
                .request_slot(
                    "baidu-test-account",
                    Provider::Baidu,
                    &mut reservation,
                    now + chrono::Duration::seconds(1)
                )
                .unwrap(),
            0
        );
    }
    #[test]
    fn unsent_chunks_reserve_their_characters_again_when_the_period_changes() {
        let store = TestStore::new();
        let before = date("2026-11-08T23:59:59Z");
        let after = date("2026-11-09T00:00:00Z");
        let mut reservation = store
            .0
            .reserve("keyless", Provider::MyMemory, "abcdef", before)
            .unwrap();
        reservation.remaining_characters = 3;
        store
            .0
            .request_slot("keyless", Provider::MyMemory, &mut reservation, after)
            .unwrap();
        let ledger: Ledger = serde_json::from_slice(
            &fs::read(store.0.directory.join("free-api-usage.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(ledger.accounts["keyless"].used_characters, 3);
    }
    #[test]
    fn insufficient_space_for_a_large_request_does_not_block_smaller_requests() {
        let store = TestStore::new();
        let now = date("2026-11-08T12:00:00Z");
        store
            .0
            .reserve("keyless", Provider::MyMemory, &"a".repeat(4_999), now)
            .unwrap();
        assert_eq!(
            store
                .0
                .reserve("keyless", Provider::MyMemory, "ab", now)
                .unwrap_err()
                .kind,
            FailureKind::Quota
        );
        store
            .0
            .reserve("keyless", Provider::MyMemory, "中", now)
            .unwrap();
    }
    #[test]
    fn invalid_ledger_prevents_network_requests_instead_of_resetting_quota() {
        let store = TestStore::new();
        fs::create_dir_all(&store.0.directory).unwrap();
        fs::write(store.0.directory.join("free-api-usage.json"), b"corrupted").unwrap();
        let settings = Settings::default();
        let now = date("2026-11-08T12:00:00Z");
        let result = route_with_store(
            "hello",
            "en",
            "zh",
            &settings,
            &|| false,
            |_, _, _| panic!("a corrupt quota ledger must not issue HTTP requests"),
            &store.0,
            &|| now,
            &|_| {},
        );
        assert!(result.err().unwrap().message().contains("免费额度"));
    }
    #[test]
    fn quota_rate_network_and_auth_errors_continue_to_another_provider() {
        for kind in [
            FailureKind::Quota,
            FailureKind::RateLimited,
            FailureKind::Transient,
            FailureKind::Auth,
        ] {
            let store = TestStore::new();
            let settings = configured(Provider::Baidu);
            let now = date("2026-11-08T12:00:00Z");
            let result = route_with_store(
                "hello",
                "en",
                "zh",
                &settings,
                &|| false,
                |provider, _, _| {
                    if provider == Provider::Baidu {
                        Err(Failure::new(kind, "mock provider failure"))
                    } else {
                        Ok(("你好".into(), "en".into()))
                    }
                },
                &store.0,
                &|| now,
                &|_| {},
            )
            .unwrap();
            assert_eq!(result.provider, Provider::DeepL);
            let account = QuotaStore::account(&settings, Provider::Baidu);
            assert_eq!(
                store
                    .0
                    .reserve(&account, Provider::Baidu, "x", now)
                    .unwrap_err()
                    .kind,
                kind
            );
        }
    }
    #[test]
    fn provider_request_limits_preserve_all_unicode_text() {
        let source = "中文🦀 hello world。".repeat(400);
        for provider in [Provider::MyMemory, Provider::Baidu] {
            let parts = chunks(&source, provider);
            assert_eq!(parts.concat(), source);
            assert!(parts.iter().all(|part| !part.is_empty()));
            assert!(parts.iter().all(|part| if provider == Provider::MyMemory {
                part.len() <= 450
            } else {
                part.chars().count() <= 1_000
            }));
        }
    }
    #[test]
    fn cancellation_stops_before_the_next_provider_request() {
        let store = TestStore::new();
        let settings = configured(Provider::Baidu);
        let now = date("2026-11-08T12:00:00Z");
        let stop = std::cell::Cell::new(false);
        let mut attempts = 0;
        let result = route_with_store(
            "hello",
            "en",
            "zh",
            &settings,
            &|| stop.get(),
            |_, _, _| {
                attempts += 1;
                stop.set(true);
                Err(Failure::new(FailureKind::Transient, "mock network failure"))
            },
            &store.0,
            &|| now,
            &|_| {},
        );
        assert!(result.unwrap_err().cancelled);
        assert_eq!(attempts, 1);
    }
}
