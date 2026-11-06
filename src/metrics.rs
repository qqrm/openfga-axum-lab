use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry, TextEncoder,
};

#[derive(Clone)]
pub(crate) struct Metrics {
    registry: Registry,
    pub(crate) requests: IntCounterVec,
    pub(crate) duration: HistogramVec,
    pub(crate) authz_cache: IntCounterVec,
    pub(crate) authz_revision_reads: prometheus::IntCounter,
    pub(crate) authz_openfga: IntCounterVec,
}

impl Metrics {
    pub(crate) fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();
        let requests = IntCounterVec::new(
            Opts::new("http_requests_total", "Completed HTTP requests"),
            &["method", "route", "status"],
        )?;
        let duration = HistogramVec::new(
            HistogramOpts::new("http_request_duration_seconds", "HTTP response latency"),
            &["method", "route"],
        )?;
        registry.register(Box::new(requests.clone()))?;
        registry.register(Box::new(duration.clone()))?;
        let authz_cache = IntCounterVec::new(
            Opts::new(
                "authz_cache_events_total",
                "Authorization decision cache events",
            ),
            &["event"],
        )?;
        registry.register(Box::new(authz_cache.clone()))?;
        let authz_revision_reads = prometheus::IntCounter::new(
            "authz_revision_reads_total",
            "PostgreSQL authorization revision read attempts",
        )?;
        registry.register(Box::new(authz_revision_reads.clone()))?;
        let authz_openfga = IntCounterVec::new(
            Opts::new(
                "authz_openfga_requests_total",
                "OpenFGA HTTP request attempts",
            ),
            &["operation"],
        )?;
        registry.register(Box::new(authz_openfga.clone()))?;
        Ok(Self {
            registry,
            requests,
            duration,
            authz_cache,
            authz_revision_reads,
            authz_openfga,
        })
    }
    pub(crate) fn render(&self) -> Result<String, Box<dyn std::error::Error>> {
        let mut bytes = Vec::new();
        TextEncoder::new().encode(&self.registry.gather(), &mut bytes)?;
        Ok(String::from_utf8(bytes)?)
    }
}
