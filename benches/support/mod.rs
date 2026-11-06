use reqwest::{Client, Method, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{collections::BTreeMap, path::Path, time::Duration};

pub type BenchResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub struct Fixture {
    client: Client,
    base: String,
    observer: PgPool,
    pub alice: String,
    pub dave: String,
    expected: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Snapshot {
    pub counters: BTreeMap<String, u64>,
    pub pg_statements: BTreeMap<String, u64>,
}

impl Fixture {
    pub async fn connect(mode: &str) -> BenchResult<Self> {
        let base = match mode {
            "off" => {
                std::env::var("UNCACHED_URL").unwrap_or_else(|_| "http://127.0.0.1:3001".into())
            }
            "on" => std::env::var("CACHED_URL").unwrap_or_else(|_| "http://127.0.0.1:3000".into()),
            _ => return Err("AUTHZ_BENCH_MODE must be on or off".into()),
        };
        let url = Url::parse(&base)?;
        if url.scheme() != "http" || !matches!(url.host_str(), Some("127.0.0.1" | "localhost")) {
            return Err("benchmarks require an isolated loopback HTTP endpoint".into());
        }
        let client = Client::builder()
            .no_proxy()
            .http1_only()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()?;
        tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                if client
                    .get(format!("{base}/healthz"))
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await?;
        let observer = PgPoolOptions::new().max_connections(1).connect(
            &std::env::var("BENCH_OBSERVER_DATABASE_URL").unwrap_or_else(|_|
                "postgres://petlab:petlab-local-only@127.0.0.1:55432/postgres?sslmode=disable".into()),
        ).await?;
        let mut fixture = Self {
            client,
            base,
            observer,
            alice: String::new(),
            dave: String::new(),
            expected: json!({"id":"fox","name":"Mika","species":"fox",
                "biography":"A curious fox who likes quiet afternoons."}),
        };
        fixture.alice = match std::env::var("BENCH_ALICE_TOKEN") {
            Ok(token) => token,
            Err(_) => fixture.login("alice").await?,
        };
        fixture.dave = match std::env::var("BENCH_DAVE_TOKEN") {
            Ok(token) => token,
            Err(_) => fixture.login("dave").await?,
        };
        fixture.expect("", Method::GET, "/pets/fox", 401).await?;
        fixture.read("allow_hot").await?;
        fixture.read("deny_hot").await?;
        Ok(fixture)
    }

    pub async fn login(&self, user: &str) -> BenchResult<String> {
        let value: Value = self
            .client
            .post(format!("{}/session", self.base))
            .json(
                &json!({"username":user,"password":std::env::var("DEMO_PASSWORD")
                .unwrap_or_else(|_| "local-demo-password".into())}),
            )
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(value["token"]
            .as_str()
            .ok_or("missing login token")?
            .to_owned())
    }

    pub async fn expect(
        &self,
        token: &str,
        method: Method,
        path: &str,
        expected: u16,
    ) -> BenchResult<Vec<u8>> {
        let mut request = self
            .client
            .request(method.clone(), format!("{}{path}", self.base));
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?;
        let status = response.status().as_u16();
        let body = response.bytes().await?.to_vec();
        if status != expected {
            return Err(format!("{method} {path}: expected {expected}, got {status}").into());
        }
        Ok(body)
    }

    pub async fn read(&self, case: &str) -> BenchResult {
        match case {
            "allow_hot" => {
                let bytes = self
                    .expect(&self.alice, Method::GET, "/pets/fox", 200)
                    .await?;
                if serde_json::from_slice::<Value>(&bytes)? != self.expected {
                    return Err("incorrect allowed response body".into());
                }
            }
            "deny_hot" => {
                let bytes = self
                    .expect(&self.dave, Method::GET, "/pets/fox", 403)
                    .await?;
                if !bytes.is_empty() {
                    return Err("denied response leaked a body".into());
                }
            }
            _ => return Err("unknown benchmark case".into()),
        }
        Ok(())
    }

    pub async fn snapshot(&self) -> BenchResult<Snapshot> {
        let text = self
            .client
            .get(format!("{}/metrics", self.base))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let mut counters = BTreeMap::new();
        for line in text.lines().filter(|line| line.starts_with("authz_")) {
            let (name, value) = line.rsplit_once(' ').ok_or("malformed authz metric")?;
            counters.insert(name.to_owned(), value.parse()?);
        }
        // Maintenance DB: the observer's own queries are excluded from both databases.
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT d.datname::text, sum(s.calls)::bigint FROM pg_stat_statements s \
             JOIN pg_database d ON d.oid=s.dbid WHERE d.datname IN ('petlab','openfga') \
             GROUP BY d.datname ORDER BY d.datname",
        )
        .fetch_all(&self.observer)
        .await?;
        Ok(Snapshot {
            counters,
            pg_statements: rows
                .into_iter()
                .map(|(name, calls)| Ok((name, u64::try_from(calls)?)))
                .collect::<BenchResult<_>>()?,
        })
    }
}

pub fn delta(before: &Snapshot, after: &Snapshot) -> BenchResult<Snapshot> {
    fn subtract(
        before: &BTreeMap<String, u64>,
        after: &BTreeMap<String, u64>,
    ) -> BenchResult<BTreeMap<String, u64>> {
        let mut result = BTreeMap::new();
        for key in before.keys().chain(after.keys()) {
            let value = after
                .get(key)
                .copied()
                .unwrap_or(0)
                .checked_sub(before.get(key).copied().unwrap_or(0))
                .ok_or("counter reset")?;
            result.insert(key.clone(), value);
        }
        Ok(result)
    }
    Ok(Snapshot {
        counters: subtract(&before.counters, &after.counters)?,
        pg_statements: subtract(&before.pg_statements, &after.pg_statements)?,
    })
}

pub fn validate_counters(snapshot: &Snapshot, mode: &str) -> BenchResult<u64> {
    let event = |name: &str| {
        snapshot
            .counters
            .get(&format!("authz_cache_events_total{{event=\"{name}\"}}"))
            .copied()
            .unwrap_or(0)
    };
    if event("retry") != 0 || event("bypass_pending") != 0 {
        return Err("authorization contention during a read-only experiment".into());
    }
    let hits = event("hit_allow") + event("hit_deny");
    let count = hits + event("miss") + event("uncached");
    if count == 0
        || (mode == "off" && event("uncached") != count)
        || (mode == "on" && (hits == 0 || event("uncached") != 0))
    {
        return Err("cache mode not confirmed by server counters".into());
    }
    Ok(count)
}

pub fn write_json(path: &Path, value: &impl Serialize) -> BenchResult {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}
