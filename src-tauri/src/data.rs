//! Per-account data layer: account discovery, credentials, OAuth usage API,
//! subscription detection, and local JSONL breakdown. Direct port of the
//! Python `Account` class, one struct instance per Claude config dir.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use walkdir::WalkDir;

const MIN_FETCH_INTERVAL: f64 = 55.0;
const RETENTION_DAYS: i64 = 14;
const USAGE_API_URL: &str = "https://api.anthropic.com/api/oauth/usage";

fn now_unix() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

// ─── Serializable output (sent to the frontend) ──────────────────────

#[derive(Serialize, Clone, Default)]
pub struct Window {
    pub utilization: f64,
    pub resets_at: Option<String>,
    pub resets_in: String,
}

#[derive(Serialize, Clone, Default)]
pub struct Subscription {
    pub plan: String,
    pub display: String,
    pub has_sonnet: bool,
    pub models: Vec<String>,
}

#[derive(Serialize, Clone, Default)]
pub struct Usage {
    pub session: Window,
    pub weekly_all: Window,
    pub weekly_sonnet: Window,
    pub weekly_opus: Window,
    pub subscription: Subscription,
    pub updated_ago: String,
    pub stale: bool,
}

#[derive(Serialize, Clone, Default)]
pub struct DayCount {
    pub day: String,
    pub date: String,
    pub total: u64,
    pub opus: u64,
    pub sonnet: u64,
    pub haiku: u64,
    pub other: u64,
}

#[derive(Serialize, Clone, Default)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub requests: u64,
}

#[derive(Serialize, Clone, Default)]
pub struct Local {
    pub by_model: HashMap<String, u64>,
    pub daily: Vec<DayCount>,
    pub weekly_tokens: Tokens,
}

#[derive(Serialize, Clone, Default)]
pub struct AccountData {
    pub name: String,
    pub session_pct: f64,
    pub plan_display: String,
    pub usage: Option<Usage>,
    pub local: Local,
}

// ─── Pure helpers ────────────────────────────────────────────────────

fn normalize_plan(val: Option<&str>) -> Option<String> {
    let raw = val?.trim();
    if raw.is_empty() {
        return None;
    }
    let v = raw.to_lowercase().replace('-', "_").replace(' ', "_");
    let has = |s: &str| v.contains(s);
    Some(
        if has("max") && has("20") {
            "max_20".into()
        } else if has("max") && has("5") {
            "max_5".into()
        } else if has("max") {
            "max".into()
        } else if has("pro") {
            "pro".into()
        } else if has("free") {
            "free".into()
        } else if has("team") {
            "team".into()
        } else {
            v
        },
    )
}

fn find_plan_in_dict(v: &Value, depth: u32) -> Option<String> {
    if depth > 3 {
        return None;
    }
    let obj = v.as_object()?;
    for key in [
        "membershipTier",
        "membership_tier",
        "tier",
        "plan",
        "plan_type",
        "subscription_type",
    ] {
        if let Some(s) = obj.get(key).and_then(|x| x.as_str()) {
            if !s.is_empty() {
                return normalize_plan(Some(s));
            }
        }
    }
    for (_, val) in obj {
        if val.is_object() {
            if let Some(r) = find_plan_in_dict(val, depth + 1) {
                return Some(r);
            }
        }
    }
    None
}

fn title_case(s: &str) -> String {
    s.replace('_', " ")
        .split(' ')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn plan_display(plan: &str) -> String {
    match plan {
        "pro" => "Pro".into(),
        "max_5" => "Max (5x)".into(),
        "max_20" => "Max (20x)".into(),
        "free" => "Free".into(),
        "team" => "Team".into(),
        "max" => "Max".into(),
        other => title_case(other),
    }
}

fn plan_models(plan: &str) -> Vec<String> {
    let m: &[&str] = match plan {
        "pro" => &["Opus", "Haiku"],
        "max_5" | "max_20" | "max" | "team" => &["Opus", "Sonnet", "Haiku"],
        "free" => &["Haiku"],
        _ => &["Opus", "Haiku"],
    };
    m.iter().map(|s| s.to_string()).collect()
}

/// model index: 0 opus, 1 sonnet, 2 haiku, 3 other
fn classify_model(s: &str) -> Option<u8> {
    if s.is_empty() || s == "<synthetic>" {
        return None;
    }
    let m = s.to_lowercase();
    Some(if m.contains("opus") {
        0
    } else if m.contains("sonnet") {
        1
    } else if m.contains("haiku") {
        2
    } else {
        3
    })
}

fn model_key(i: u8) -> &'static str {
    match i {
        0 => "opus",
        1 => "sonnet",
        2 => "haiku",
        _ => "other",
    }
}

fn parse_dt(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn time_until(dt: Option<DateTime<Utc>>) -> String {
    let dt = match dt {
        Some(d) => d,
        None => return String::new(),
    };
    let delta = dt - Utc::now();
    let secs = delta.num_seconds();
    if secs <= 0 {
        return "now".into();
    }
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

fn fmt_ago(secs: f64) -> String {
    let s = secs.max(0.0) as i64;
    if s < 60 {
        format!("{s}s ago")
    } else {
        format!("{}m ago", s / 60)
    }
}

fn append_tmp(p: &Path) -> PathBuf {
    let mut name = p
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("cache")
        .to_string();
    name.push_str(".tmp");
    p.with_file_name(name)
}

// ─── JSONL entry parsing + per-file cache ────────────────────────────

#[derive(Clone)]
struct Entry {
    ts: DateTime<Utc>,
    model: u8,
    in_tok: u64,
    out_tok: u64,
}

struct FileCache {
    mtime: u64,
    size: u64,
    entries: Vec<Entry>,
}

fn parse_jsonl_entry(line: &str) -> Option<Entry> {
    let d: Value = serde_json::from_str(line).ok()?;
    if d.get("type").and_then(|x| x.as_str())? != "assistant" {
        return None;
    }
    let msg = d.get("message")?;
    let model = classify_model(msg.get("model").and_then(|x| x.as_str()).unwrap_or(""))?;
    let ts = parse_dt(d.get("timestamp").and_then(|x| x.as_str())?)?;
    let usage = msg.get("usage");
    let getu = |k: &str| {
        usage
            .and_then(|u| u.get(k))
            .and_then(|x| x.as_u64())
            .unwrap_or(0)
    };
    let in_tok = getu("input_tokens") + getu("cache_creation_input_tokens") + getu("cache_read_input_tokens");
    let out_tok = getu("output_tokens");
    Some(Entry {
        ts,
        model,
        in_tok,
        out_tok,
    })
}

// ─── Account ─────────────────────────────────────────────────────────

pub struct Account {
    pub name: String,
    pub key: String,
    credentials_path: PathBuf,
    projects_dir: PathBuf,
    cache_path: PathBuf,

    last_usage: Option<Value>,
    last_usage_time: f64,
    backoff_until: f64,
    consecutive_429s: u32,
    account_info_fetched: bool,
    account_info_plan: Option<String>,
    jsonl_cache: HashMap<PathBuf, FileCache>,
}

enum ApiErr {
    Status(u16),
    Other,
}

fn oauth_get(url: &str, token: &str) -> ureq::Request {
    ureq::get(url)
        .timeout(std::time::Duration::from_secs(15))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json")
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("User-Agent", "claude-code/2.1")
}

impl Account {
    fn new(name: String, key: String, dir: PathBuf, cache_dir: &Path) -> Account {
        // cache file is keyed on the stable folder name, not the display name,
        // so renaming an account doesn't orphan its cache.
        let safe: String = key
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect();
        let safe = if safe.is_empty() { "default".into() } else { safe };
        Account {
            credentials_path: dir.join(".credentials.json"),
            projects_dir: dir.join("projects"),
            cache_path: cache_dir.join(format!(".usage_cache_{safe}.json")),
            name,
            key,
            last_usage: None,
            last_usage_time: 0.0,
            backoff_until: 0.0,
            consecutive_429s: 0,
            account_info_fetched: false,
            account_info_plan: None,
            jsonl_cache: HashMap::new(),
        }
    }

    fn read_oauth(&self) -> Option<Value> {
        let s = fs::read_to_string(&self.credentials_path).ok()?;
        let v: Value = serde_json::from_str(&s).ok()?;
        v.get("claudeAiOauth").cloned()
    }

    fn get_token(&self) -> Option<String> {
        self.read_oauth()?
            .get("accessToken")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
    }

    fn refresh_token(&mut self) -> Option<String> {
        let s = fs::read_to_string(&self.credentials_path).ok()?;
        let mut creds: Value = serde_json::from_str(&s).ok()?;
        let refresh = creds["claudeAiOauth"]["refreshToken"]
            .as_str()?
            .to_string();
        let resp = ureq::post("https://api.anthropic.com/api/oauth/token")
            .timeout(std::time::Duration::from_secs(15))
            .set("Content-Type", "application/json")
            .set("User-Agent", "claude-code/2.1")
            .send_json(json!({"grant_type": "refresh_token", "refresh_token": refresh}))
            .ok()?;
        let r: Value = resp.into_json().ok()?;
        let at = r["access_token"].as_str()?.to_string();
        creds["claudeAiOauth"]["accessToken"] = Value::String(at.clone());
        if let Some(rt) = r["refresh_token"].as_str() {
            creds["claudeAiOauth"]["refreshToken"] = Value::String(rt.to_string());
        }
        if let Some(exp) = r["expires_in"].as_i64() {
            let expires_at = (now_unix() as i64) * 1000 + exp * 1000;
            creds["claudeAiOauth"]["expiresAt"] = Value::from(expires_at);
        }
        let tmp = append_tmp(&self.credentials_path);
        fs::write(&tmp, serde_json::to_string_pretty(&creds).ok()?).ok()?;
        fs::rename(&tmp, &self.credentials_path).ok()?;
        Some(at)
    }

    // ── Subscription detection ──

    fn plan_from_credentials(&self) -> Option<String> {
        let oauth = self.read_oauth()?;
        let sub = oauth.get("subscriptionType").and_then(|x| x.as_str());
        let tier = oauth
            .get("rateLimitTier")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if let Some(s) = sub {
            if s.to_lowercase().contains("max") {
                if tier.contains("20") {
                    return Some("max_20".into());
                }
                if tier.contains('5') {
                    return Some("max_5".into());
                }
                return Some("max".into());
            }
        }
        normalize_plan(sub)
    }

    fn try_fetch_account_info(&mut self) -> Option<String> {
        if self.account_info_fetched {
            return self.account_info_plan.clone();
        }
        self.account_info_fetched = true;
        let token = self.get_token()?;
        for url in [
            "https://api.anthropic.com/api/me",
            "https://api.anthropic.com/api/bootstrap",
        ] {
            if let Ok(resp) = oauth_get(url, &token).call() {
                if let Ok(data) = resp.into_json::<Value>() {
                    if let Some(plan) = find_plan_in_dict(&data, 0) {
                        self.account_info_plan = Some(plan.clone());
                        return Some(plan);
                    }
                }
            }
        }
        None
    }

    fn detect_subscription(&mut self, data: &Value) -> Subscription {
        let mut plan = self.plan_from_credentials();
        if plan.is_none() {
            plan = find_plan_in_dict(data, 0);
        }
        if plan.is_none() {
            plan = self.try_fetch_account_info();
        }
        let has_sonnet = data
            .get("seven_day_sonnet")
            .filter(|w| w.is_object())
            .and_then(|w| w.get("resets_at"))
            .and_then(|r| r.as_str())
            .map_or(false, |s| !s.is_empty());
        let plan = plan.unwrap_or_else(|| if has_sonnet { "max".into() } else { "pro".into() });
        Subscription {
            display: plan_display(&plan),
            models: plan_models(&plan),
            has_sonnet,
            plan,
        }
    }

    // ── Usage API ──

    fn store_usage(&mut self, data: Value) {
        self.last_usage = Some(data);
        self.last_usage_time = now_unix();
        self.consecutive_429s = 0;
        self.save_cache();
    }

    fn api_get(&self, token: &str) -> Result<Value, ApiErr> {
        let resp = oauth_get(USAGE_API_URL, token)
            .set("Content-Type", "application/json")
            .call()
            .map_err(|e| match e {
                ureq::Error::Status(code, _) => ApiErr::Status(code),
                _ => ApiErr::Other,
            })?;
        resp.into_json::<Value>().map_err(|_| ApiErr::Other)
    }

    fn fetch_usage(&mut self) -> Option<Value> {
        let now = now_unix();
        if self.last_usage.is_none() {
            self.load_cache();
        }
        if self.last_usage.is_some() && (now - self.last_usage_time) < MIN_FETCH_INTERVAL {
            return self.last_usage.clone();
        }
        if now < self.backoff_until {
            return self.last_usage.clone();
        }
        let token = match self.get_token() {
            Some(t) => t,
            None => return self.last_usage.clone(),
        };
        match self.api_get(&token) {
            Ok(v) => {
                self.store_usage(v.clone());
                Some(v)
            }
            Err(ApiErr::Status(401)) => {
                if let Some(nt) = self.refresh_token() {
                    if let Ok(v) = self.api_get(&nt) {
                        self.store_usage(v.clone());
                        return Some(v);
                    }
                }
                self.last_usage.clone()
            }
            Err(ApiErr::Status(429)) => {
                self.consecutive_429s += 1;
                let backoff = (60.0 * 2f64.powi(self.consecutive_429s as i32 - 1)).min(300.0);
                self.backoff_until = now + backoff;
                self.last_usage.clone()
            }
            Err(_) => self.last_usage.clone(),
        }
    }

    fn save_cache(&self) {
        if let Some(data) = &self.last_usage {
            let wrapper = json!({"data": data, "time": self.last_usage_time});
            let tmp = append_tmp(&self.cache_path);
            if fs::write(&tmp, wrapper.to_string()).is_ok() {
                let _ = fs::rename(&tmp, &self.cache_path);
            }
        }
    }

    fn load_cache(&mut self) {
        if let Ok(s) = fs::read_to_string(&self.cache_path) {
            if let Ok(v) = serde_json::from_str::<Value>(&s) {
                if let Some(data) = v.get("data") {
                    if !data.is_null() {
                        self.last_usage = Some(data.clone());
                        self.last_usage_time = v.get("time").and_then(|t| t.as_f64()).unwrap_or(0.0);
                    }
                }
            }
        }
    }

    fn parse_window(&self, w: Option<&Value>) -> Window {
        let w = match w {
            Some(w) if w.is_object() => w,
            _ => return Window::default(),
        };
        let util = w.get("utilization").and_then(|x| x.as_f64()).unwrap_or(0.0);
        let resets_at = w
            .get("resets_at")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        let dt = resets_at.as_deref().and_then(parse_dt);
        Window {
            utilization: util,
            resets_at,
            resets_in: time_until(dt),
        }
    }

    fn build_usage(&mut self, data: &Value) -> Usage {
        let subscription = self.detect_subscription(data);
        let session = self.parse_window(data.get("five_hour"));
        let weekly_all = self.parse_window(data.get("seven_day"));
        let weekly_sonnet = self.parse_window(data.get("seven_day_sonnet"));
        let weekly_opus = self.parse_window(data.get("seven_day_opus"));
        let age = now_unix() - self.last_usage_time;
        Usage {
            session,
            weekly_all,
            weekly_sonnet,
            weekly_opus,
            subscription,
            updated_ago: fmt_ago(age),
            stale: age > 120.0 || self.consecutive_429s > 0,
        }
    }

    // ── Local JSONL breakdown ──

    fn update_file_cache(&mut self, path: &Path, retention: DateTime<Utc>) {
        let meta = match fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return,
        };
        let size = meta.len();
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Some(fc) = self.jsonl_cache.get(path) {
            if fc.mtime == mtime && fc.size == size {
                return;
            }
        }
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return,
        };
        let mut entries = Vec::new();
        for line in content.lines() {
            if let Some(e) = parse_jsonl_entry(line) {
                if e.ts >= retention {
                    entries.push(e);
                }
            }
        }
        self.jsonl_cache.insert(
            path.to_path_buf(),
            FileCache {
                mtime,
                size,
                entries,
            },
        );
    }

    fn local_breakdown(&mut self) -> Local {
        let now = Utc::now();
        let week_ago = now - Duration::days(7);
        let retention = now - Duration::days(RETENTION_DAYS);

        let mut seen: HashSet<PathBuf> = HashSet::new();
        if self.projects_dir.is_dir() {
            let files: Vec<PathBuf> = WalkDir::new(&self.projects_dir)
                .into_iter()
                .filter_map(|e| e.ok())
                .map(|e| e.into_path())
                .filter(|p| p.extension().map_or(false, |e| e == "jsonl"))
                .collect();
            for path in files {
                seen.insert(path.clone());
                self.update_file_cache(&path, retention);
            }
        }
        self.jsonl_cache.retain(|k, _| seen.contains(k));

        let mut by_model: HashMap<String, u64> = HashMap::new();
        let mut tokens = Tokens::default();
        let mut daily: HashMap<String, [u64; 4]> = HashMap::new();
        for fc in self.jsonl_cache.values() {
            for e in &fc.entries {
                if e.ts < week_ago {
                    continue;
                }
                *by_model.entry(model_key(e.model).to_string()).or_default() += 1;
                let d = e.ts.format("%Y-%m-%d").to_string();
                daily.entry(d).or_insert([0; 4])[e.model as usize] += 1;
                tokens.input += e.in_tok;
                tokens.output += e.out_tok;
                tokens.requests += 1;
            }
        }

        let mut days = Vec::new();
        for i in (0..7).rev() {
            let day_dt = now - Duration::days(i);
            let date = day_dt.format("%Y-%m-%d").to_string();
            let c = daily.get(&date).copied().unwrap_or([0; 4]);
            days.push(DayCount {
                day: day_dt.format("%a").to_string(),
                date,
                total: c.iter().sum(),
                opus: c[0],
                sonnet: c[1],
                haiku: c[2],
                other: c[3],
            });
        }

        Local {
            by_model,
            daily: days,
            weekly_tokens: tokens,
        }
    }

    // ── Combined ──

    pub fn get_data(&mut self) -> AccountData {
        let raw = self.fetch_usage();
        let usage = raw.as_ref().map(|v| self.build_usage(v));
        let local = self.local_breakdown();
        let session_pct = usage
            .as_ref()
            .map(|u| u.session.utilization)
            .unwrap_or(0.0);
        let plan_display = usage
            .as_ref()
            .map(|u| u.subscription.display.clone())
            .unwrap_or_else(|| "Claude".into());
        AccountData {
            name: self.name.clone(),
            session_pct,
            plan_display,
            usage,
            local,
        }
    }
}

// ─── Discovery ───────────────────────────────────────────────────────

/// User-set display names, keyed by folder name (e.g. ".claude-work" -> "Work").
pub fn load_names(cache_dir: &Path) -> HashMap<String, String> {
    fs::read_to_string(cache_dir.join("names.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist a single account's custom name (atomic write).
pub fn save_name(cache_dir: &Path, key: &str, name: &str) {
    let mut map = load_names(cache_dir);
    map.insert(key.to_string(), name.to_string());
    if let Ok(s) = serde_json::to_string_pretty(&map) {
        let tmp = cache_dir.join("names.json.tmp");
        if fs::write(&tmp, s).is_ok() {
            let _ = fs::rename(&tmp, &cache_dir.join("names.json"));
        }
    }
}

pub fn discover_accounts(cache_dir: &Path) -> Vec<Account> {
    let names = load_names(cache_dir);
    let home = dirs::home_dir().unwrap_or_default();

    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = fs::read_dir(&home) {
        dirs = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map_or(false, |n| n.starts_with(".claude"))
            })
            .filter(|p| p.join(".credentials.json").exists())
            .collect();
        dirs.sort();
    }
    if dirs.is_empty() {
        dirs.push(home.join(".claude"));
    }

    // Default name is a neutral "Account N" by position; users can rename in-app.
    dirs.into_iter()
        .enumerate()
        .map(|(i, d)| {
            let key = d
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(".claude")
                .to_string();
            let name = names
                .get(&key)
                .cloned()
                .unwrap_or_else(|| format!("Account {}", i + 1));
            Account::new(name, key, d, cache_dir)
        })
        .collect()
}
