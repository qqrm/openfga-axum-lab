use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry, TextEncoder,
};

#[derive(Clone)]
pub(crate) struct Metrics {
    registry: Registry,
    pub(crate) requests: IntCounterVec,
    pub(crate) duration: HistogramVec,
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
        Ok(Self {
            registry,
            requests,
            duration,
        })
    }
    pub(crate) fn render(&self) -> Result<String, Box<dyn std::error::Error>> {
        let mut bytes = Vec::new();
        TextEncoder::new().encode(&self.registry.gather(), &mut bytes)?;
        Ok(String::from_utf8(bytes)?)
    }
}
