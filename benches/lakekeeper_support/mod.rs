//! Shared Lakekeeper HTTP fixture and fail-fast evidence checks for Criterion.
use reqwest::{Client, Method, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Duration};

pub type BenchResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub const TABLE: &str = "/projects/acme/warehouses/analytics/tables/requests";
pub const GRANT: &str = "/projects/acme/warehouses/analytics/tables/requests/grants/modify";
const ROUTE: &str = "/projects/{project}/warehouses/{warehouse}/tables/{id}";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    On,
    Off,
}
impl Mode {
    pub fn parse(value: &str) -> BenchResult<Self> {
        match value {
            "on" => Ok(Self::On),
            "off" => Ok(Self::Off),
            _ => Err("AUTHZ_BENCH_MODE must be on or off".into()),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
        }
    }
    pub fn swapped_ports() -> BenchResult<bool> {
        match std::env::var("AUTHZ_BENCH_SWAP_PORTS").as_deref() {
            Ok("true") => Ok(true),
            Ok("false") | Err(std::env::VarError::NotPresent) => Ok(false),
            _ => Err("AUTHZ_BENCH_SWAP_PORTS must be true or false".into()),
        }
    }
    fn default_port(self, swapped: bool) -> u16 {
        match (self, swapped) {
            (Self::On, false) | (Self::Off, true) => 3000,
            (Self::Off, false) | (Self::On, true) => 3001,
        }
    }
    pub fn base(self) -> BenchResult<String> {
        let key = match self {
            Self::On => "CACHED_URL",
            Self::Off => "UNCACHED_URL",
        };
        let default = format!(
            "http://127.0.0.1:{}",
            self.default_port(Self::swapped_ports()?)
        );
        let base = std::env::var(key).unwrap_or(default);
        let parsed = Url::parse(&base)?;
        if parsed.scheme() != "http"
            || !matches!(parsed.host_str(), Some("localhost" | "127.0.0.1"))
            || parsed.username() != ""
            || parsed.password().is_some()
            || parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(
                "benchmark endpoints must be plain HTTP on loopback without URL credentials".into(),
            );
        }
        Ok(base.trim_end_matches('/').to_owned())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    AllowFresh,
    DenyHot,
}
impl Case {
    pub fn name(self) -> &'static str {
        match self {
            Self::AllowFresh => "allow_fresh_check",
            Self::DenyHot => "deny_cached_or_uncached",
        }
    }
    fn status(self) -> u16 {
        match self {
            Self::AllowFresh => 200,
            Self::DenyHot => 403,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Snapshot {
    pub counters: BTreeMap<String, u64>,
}
impl Snapshot {
    pub fn event(&self, event: &str) -> u64 {
        self.counters
            .get(&format!("authz_cache_events_total{{event=\"{event}\"}}"))
            .copied()
            .unwrap_or(0)
    }
    pub fn http(&self, status: u16) -> u64 {
        self.counters
            .iter()
            .filter_map(|(key, value)| {
                (key.starts_with("http_requests_total{")
                    && key.contains("method=\"GET\"")
                    && key.contains(&format!("route=\"{ROUTE}\""))
                    && key.contains(&format!("status=\"{status}\"")))
                .then_some(*value)
            })
            .sum()
    }
    pub fn subtract(&self, earlier: &Self) -> BenchResult<Self> {
        let mut delta = BTreeMap::new();
        for key in self.counters.keys().chain(earlier.counters.keys()) {
            let before = earlier.counters.get(key).copied().unwrap_or(0);
            let after = self.counters.get(key).copied().unwrap_or(0);
            delta.insert(
                key.clone(),
                after.checked_sub(before).ok_or("metric counter reset")?,
            );
        }
        Ok(Self { counters: delta })
    }
    pub fn validate(&self, mode: Mode, case: Case) -> BenchResult<u64> {
        if self.event("hit_allow") != 0
            || self.event("retry") != 0
            || self.event("blocked_pending") != 0
        {
            return Err("unexpected allow-cache hit, retry or durable mutation fence".into());
        }
        let miss = self.event("miss");
        let hit = self.event("hit_deny");
        let uncached = self.event("uncached");
        let total = miss + hit + uncached;
        if total == 0 || total != self.http(case.status()) {
            return Err(format!(
                "missing/extra decisions: cache={total}, HTTP={}",
                self.http(case.status())
            )
            .into());
        }
        match (mode, case) {
            (Mode::On, Case::AllowFresh) if miss == total && hit == 0 && uncached == 0 => {},
            (Mode::On, Case::DenyHot) if hit > 0 && uncached == 0 => {},
            (Mode::Off, _) if uncached == total && miss == 0 && hit == 0 => {},
            _ => return Err(format!("cache path mismatch: mode={mode:?}, case={case:?}, miss={miss}, hit={hit}, uncached={uncached}").into()),
        }
        Ok(total)
    }
}

#[derive(Clone)]
pub struct Fixture {
    client: Client,
    base: String,
    pub alice: String,
    pub carol: String,
    pub dave: String,
}
impl Fixture {
    pub async fn connect(mode: Mode) -> BenchResult<Self> {
        let base = mode.base()?;
        let client = Client::builder()
            .no_proxy()
            .http1_only()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()?;
        let mut f = Self {
            client,
            base,
            alice: String::new(),
            carol: String::new(),
            dave: String::new(),
        };
        f.wait_ready().await?;
        f.alice = f.fixture_token("BENCH_ALICE_TOKEN", "alice").await?;
        f.carol = f.fixture_token("BENCH_CAROL_TOKEN", "carol").await?;
        f.dave = f.fixture_token("BENCH_DAVE_TOKEN", "dave").await?;
        f.read(Case::AllowFresh).await?;
        f.read(Case::DenyHot).await?;
        Ok(f)
    }
    pub async fn wait_ready(&self) -> BenchResult {
        tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                if self
                    .client
                    .get(format!("{}/readyz", self.base))
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await
        .map_err(|_| "Lakekeeper readiness timed out")?;
        Ok(())
    }
    async fn fixture_token(&self, key: &str, user: &str) -> BenchResult<String> {
        // The experiment driver pre-creates tokens once. Criterion subprocesses
        // reuse them so session rows are identical across every timed arm.
        let token = match std::env::var(key) {
            Ok(token) => token,
            Err(std::env::VarError::NotPresent) => self.login(user).await?,
            Err(error) => return Err(format!("{key}: {error}").into()),
        };
        if token.len() != 64 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("{key}: malformed fixture bearer token").into());
        }
        Ok(token)
    }
    async fn login(&self, user: &str) -> BenchResult<String> {
        let password = std::env::var("DEMO_PASSWORD")
            .map_err(|_| "set DEMO_PASSWORD for isolated Lakekeeper fixture")?;
        let value: Value = self
            .client
            .post(format!("{}/session", self.base))
            .json(&json!({"username":user,"password":password}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let token = value["token"]
            .as_str()
            .ok_or("login did not return a bearer token")?;
        if token.len() != 64 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("malformed fixture bearer token".into());
        }
        Ok(token.to_owned())
    }
    pub async fn request(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> BenchResult<(u16, Value)> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?;
        let status = response.status().as_u16();
        let bytes = response.bytes().await?;
        let json = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        };
        Ok((status, json))
    }
    pub async fn read(&self, case: Case) -> BenchResult {
        let token = match case {
            Case::AllowFresh => &self.alice,
            Case::DenyHot => &self.dave,
        };
        let (status, body) = self.request(Method::GET, TABLE, token, None).await?;
        if status != case.status() {
            return Err(format!(
                "{}: expected {}, received {status}",
                case.name(),
                case.status()
            )
            .into());
        }
        match case {
            Case::AllowFresh
                if body["id"] == "requests"
                    && body["project_id"] == "acme"
                    && body["warehouse_id"] == "analytics" =>
            {
                Ok(())
            }
            Case::DenyHot if body == json!({"error":"forbidden"}) => Ok(()),
            _ => Err(format!("unexpected or leaky {} body", case.name()).into()),
        }
    }
    pub async fn change_grant(&self, add: bool) -> BenchResult {
        let method = if add { Method::PUT } else { Method::DELETE };
        let (status, body) = self
            .request(
                method,
                GRANT,
                &self.carol,
                Some(json!({"subject":{"type":"user","id":"dave"}})),
            )
            .await?;
        if status != 204 {
            return Err(format!("grant mutation failed (HTTP {status}): {body}").into());
        }
        Ok(())
    }
    pub async fn snapshot(&self) -> BenchResult<Snapshot> {
        let raw = self
            .client
            .get(format!("{}/metrics", self.base))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let mut counters = BTreeMap::new();
        for line in raw.lines() {
            if !(line.starts_with("authz_cache_events_total{")
                || line.starts_with("http_requests_total{"))
            {
                continue;
            }
            let (key, value) = line
                .rsplit_once(' ')
                .ok_or("malformed Prometheus counter")?;
            if counters
                .insert(key.to_owned(), value.parse::<u64>()?)
                .is_some()
            {
                return Err("duplicate Prometheus counter".into());
            }
        }
        Ok(Snapshot { counters })
    }
}

pub fn write_json(path: &Path, value: &impl Serialize) -> BenchResult {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(temp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn swapped_ports_cross_over_both_modes() {
        assert_eq!(Mode::On.default_port(false), 3000);
        assert_eq!(Mode::Off.default_port(false), 3001);
        assert_eq!(Mode::On.default_port(true), 3001);
        assert_eq!(Mode::Off.default_port(true), 3000);
    }
    #[test]
    fn counter_deltas_reject_resets() {
        let before = Snapshot {
            counters: BTreeMap::from([("authz_cache_events_total{event=\"miss\"}".into(), 9)]),
        };
        let after = Snapshot {
            counters: BTreeMap::from([("authz_cache_events_total{event=\"miss\"}".into(), 8)]),
        };
        assert!(after.subtract(&before).is_err());
    }
    #[test]
    fn mode_parser_rejects_implicit_experiment_arm() {
        assert!(Mode::parse("true").is_err());
        assert_eq!(Mode::parse("on").unwrap(), Mode::On);
    }
    fn counter_fixture(status: u16, events: &[(&str, u64)]) -> Snapshot {
        let mut counters = BTreeMap::from([(
            format!("http_requests_total{{method=\"GET\",route=\"{ROUTE}\",status=\"{status}\"}}"),
            2,
        )]);
        for &(name, count) in events {
            counters.insert(
                format!("authz_cache_events_total{{event=\"{name}\"}}"),
                count,
            );
        }
        Snapshot { counters }
    }
    #[test]
    fn fresh_allow_must_account_for_every_request() {
        let valid = counter_fixture(200, &[("miss", 2)]);
        assert_eq!(valid.validate(Mode::On, Case::AllowFresh).unwrap(), 2);
        let forbidden = counter_fixture(200, &[("miss", 1), ("hit_allow", 1)]);
        assert!(forbidden.validate(Mode::On, Case::AllowFresh).is_err());
        let off = counter_fixture(200, &[("uncached", 2)]);
        assert_eq!(off.validate(Mode::Off, Case::AllowFresh).unwrap(), 2);
    }
    #[test]
    fn denied_cache_hits_are_not_uncached_checks() {
        let cached = counter_fixture(403, &[("miss", 1), ("hit_deny", 1)]);
        assert_eq!(cached.validate(Mode::On, Case::DenyHot).unwrap(), 2);
        assert!(cached.validate(Mode::Off, Case::DenyHot).is_err());
        let uncached = counter_fixture(403, &[("uncached", 2)]);
        assert_eq!(uncached.validate(Mode::Off, Case::DenyHot).unwrap(), 2);
        assert!(uncached.validate(Mode::On, Case::DenyHot).is_err());
    }
}
