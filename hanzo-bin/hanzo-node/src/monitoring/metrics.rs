//! MATRIX MODE: Performance monitoring and metrics collection
//!
//! This module bends reality to observe all system operations with zero overhead.
//! Integrates Prometheus for real-time performance tracking and optimization.

use prometheus::{
    register_counter_vec, register_gauge_vec, register_histogram_vec,
    Counter, CounterVec, Gauge, GaugeVec, Histogram, HistogramVec,
    TextEncoder, Encoder, Registry,
};
use lazy_static::lazy_static;
use std::time::{Duration, Instant};
use std::sync::Arc;
use tokio::sync::RwLock;
use log::{debug, error, info};

// ⚡ MATRIX METRICS - SEE EVERYTHING
lazy_static! {
    /// Global registry for all metrics
    static ref REGISTRY: Registry = Registry::new();

    /// Tool execution metrics
    static ref TOOL_EXECUTION_DURATION: HistogramVec = register_histogram_vec!(
        "hanzo_tool_execution_duration_seconds",
        "Tool execution duration in seconds",
        &["tool_type", "tool_name", "status"],
        vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]
    ).expect("Failed to create tool execution histogram");

    static ref TOOL_EXECUTION_COUNTER: CounterVec = register_counter_vec!(
        "hanzo_tool_executions_total",
        "Total number of tool executions",
        &["tool_type", "tool_name", "status"]
    ).expect("Failed to create tool execution counter");

    /// Database performance metrics
    static ref DB_QUERY_DURATION: HistogramVec = register_histogram_vec!(
        "hanzo_db_query_duration_seconds",
        "Database query duration in seconds",
        &["operation", "table"],
        vec![0.0001, 0.0005, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25]
    ).expect("Failed to create DB query histogram");

    static ref DB_CONNECTION_POOL_SIZE: GaugeVec = register_gauge_vec!(
        "hanzo_db_connection_pool_size",
        "Number of connections in the pool",
        &["pool_type"]
    ).expect("Failed to create DB pool size gauge");

    static ref DB_CONNECTION_POOL_ACTIVE: GaugeVec = register_gauge_vec!(
        "hanzo_db_connection_pool_active",
        "Number of active connections",
        &["pool_type"]
    ).expect("Failed to create DB pool active gauge");

    /// LLM provider metrics
    static ref LLM_REQUEST_DURATION: HistogramVec = register_histogram_vec!(
        "hanzo_llm_request_duration_seconds",
        "LLM API request duration in seconds",
        &["provider", "model", "status"],
        vec![0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 60.0]
    ).expect("Failed to create LLM request histogram");

    static ref LLM_TOKEN_COUNTER: CounterVec = register_counter_vec!(
        "hanzo_llm_tokens_total",
        "Total number of tokens processed",
        &["provider", "model", "type"] // type = input/output
    ).expect("Failed to create LLM token counter");

    /// WebSocket and networking metrics
    static ref WS_CONNECTIONS: Gauge = Gauge::new(
        "hanzo_ws_connections_active",
        "Number of active WebSocket connections"
    ).expect("Failed to create WS connections gauge");

    static ref WS_MESSAGE_COUNTER: CounterVec = register_counter_vec!(
        "hanzo_ws_messages_total",
        "Total number of WebSocket messages",
        &["direction", "type"] // direction = sent/received
    ).expect("Failed to create WS message counter");

    /// Job queue metrics
    static ref JOB_QUEUE_SIZE: GaugeVec = register_gauge_vec!(
        "hanzo_job_queue_size",
        "Number of jobs in queue",
        &["status"] // pending/processing/completed/failed
    ).expect("Failed to create job queue gauge");

    static ref JOB_PROCESSING_DURATION: HistogramVec = register_histogram_vec!(
        "hanzo_job_processing_duration_seconds",
        "Job processing duration in seconds",
        &["job_type", "status"],
        vec![0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 300.0]
    ).expect("Failed to create job processing histogram");

    /// Memory and resource metrics
    static ref MEMORY_USAGE: GaugeVec = register_gauge_vec!(
        "hanzo_memory_usage_bytes",
        "Memory usage in bytes",
        &["component"]
    ).expect("Failed to create memory usage gauge");

    static ref CPU_USAGE: GaugeVec = register_gauge_vec!(
        "hanzo_cpu_usage_percent",
        "CPU usage percentage",
        &["component"]
    ).expect("Failed to create CPU usage gauge");

    /// WASM runtime metrics
    static ref WASM_MODULE_LOAD_TIME: HistogramVec = register_histogram_vec!(
        "hanzo_wasm_module_load_seconds",
        "WASM module load time in seconds",
        &["module_name"],
        vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0]
    ).expect("Failed to create WASM load time histogram");

    static ref WASM_EXECUTION_TIME: HistogramVec = register_histogram_vec!(
        "hanzo_wasm_execution_seconds",
        "WASM execution time in seconds",
        &["module_name", "function"],
        vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0]
    ).expect("Failed to create WASM execution histogram");

    static ref WASM_FUEL_CONSUMED: HistogramVec = register_histogram_vec!(
        "hanzo_wasm_fuel_consumed",
        "WASM fuel units consumed",
        &["module_name", "function"],
        vec![1000.0, 10000.0, 100000.0, 1000000.0, 10000000.0, 100000000.0]
    ).expect("Failed to create WASM fuel histogram");

    /// TEE attestation metrics
    static ref TEE_ATTESTATION_TIME: HistogramVec = register_histogram_vec!(
        "hanzo_tee_attestation_seconds",
        "TEE attestation time in seconds",
        &["tee_type", "status"],
        vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0]
    ).expect("Failed to create TEE attestation histogram");

    static ref TEE_CACHE_HIT_RATE: GaugeVec = register_gauge_vec!(
        "hanzo_tee_cache_hit_rate",
        "TEE attestation cache hit rate",
        &["tee_type"]
    ).expect("Failed to create TEE cache hit rate gauge");

    /// HLLM regime metrics
    static ref HLLM_REGIME_SWITCHES: CounterVec = register_counter_vec!(
        "hanzo_llm_regime_switches_total",
        "Total number of HLLM regime switches",
        &["from_regime", "to_regime"]
    ).expect("Failed to create HLLM regime switch counter");

    static ref HLLM_REGIME_SWITCH_TIME: HistogramVec = register_histogram_vec!(
        "hanzo_llm_regime_switch_seconds",
        "HLLM regime switch time in seconds",
        &["from_regime", "to_regime"],
        vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5]
    ).expect("Failed to create HLLM switch time histogram");

    /// Docker/Kubernetes metrics
    static ref CONTAINER_START_TIME: HistogramVec = register_histogram_vec!(
        "hanzo_container_start_seconds",
        "Container start time in seconds",
        &["runtime", "image"],
        vec![0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 60.0]
    ).expect("Failed to create container start histogram");

    static ref CONTAINER_POOL_SIZE: GaugeVec = register_gauge_vec!(
        "hanzo_container_pool_size",
        "Number of containers in pool",
        &["runtime", "status"]
    ).expect("Failed to create container pool gauge");
}

/// Performance timer for automatic metric collection
pub struct PerfTimer {
    name: String,
    labels: Vec<(String, String)>,
    start: Instant,
    histogram: Option<Histogram>,
}

impl PerfTimer {
    /// Create a new performance timer
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            labels: Vec::new(),
            start: Instant::now(),
            histogram: None,
        }
    }

    /// Add a label to the timer
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.push((key.into(), value.into()));
        self
    }

    /// Set the histogram to record to
    pub fn with_histogram(mut self, histogram: Histogram) -> Self {
        self.histogram = Some(histogram);
        self
    }

    /// Stop the timer and record the metric
    pub fn stop(self) -> Duration {
        let duration = self.start.elapsed();

        if let Some(ref histogram) = self.histogram {
            histogram.observe(duration.as_secs_f64());
        }

        debug!(
            "⏱️ {} completed in {:?} | labels: {:?}",
            self.name, duration, self.labels
        );

        duration
    }
}

impl Drop for PerfTimer {
    fn drop(&mut self) {
        if self.histogram.is_some() {
            let duration = self.start.elapsed();
            debug!(
                "⏱️ {} dropped after {:?} (metric recorded)",
                self.name, duration
            );
        }
    }
}

/// Metrics collection and reporting
pub struct MetricsCollector {
    enabled: Arc<RwLock<bool>>,
    collection_interval: Duration,
}

impl MetricsCollector {
    /// Create a new metrics collector
    pub fn new() -> Self {
        Self {
            enabled: Arc::new(RwLock::new(true)),
            collection_interval: Duration::from_secs(10),
        }
    }

    /// Start the metrics collection background task
    pub async fn start(self: Arc<Self>) {
        let enabled = self.enabled.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(self.collection_interval);

            loop {
                interval.tick().await;

                if !*enabled.read().await {
                    continue;
                }

                // Collect system metrics
                self.collect_system_metrics().await;
            }
        });

        info!("🔴💊 Metrics collector started - THE MATRIX SEES ALL");
    }

    /// Collect system-level metrics
    async fn collect_system_metrics(&self) {
        // Memory metrics
        if let Ok(mem_info) = sys_info::mem_info() {
            let used_memory = (mem_info.total - mem_info.free) * 1024; // Convert to bytes
            MEMORY_USAGE
                .with_label_values(&["system"])
                .set(used_memory as f64);
        }

        // CPU metrics
        if let Ok(loadavg) = sys_info::loadavg() {
            CPU_USAGE
                .with_label_values(&["system"])
                .set(loadavg.one * 100.0); // Convert to percentage
        }
    }

    /// Export metrics in Prometheus format
    pub async fn export_metrics() -> Result<String, Box<dyn std::error::Error>> {
        let encoder = TextEncoder::new();
        let metric_families = REGISTRY.gather();

        let mut buffer = Vec::new();
        encoder.encode(&metric_families, &mut buffer)?;

        String::from_utf8(buffer).map_err(Into::into)
    }
}

// 🔥 PERFORMANCE TRACKING FUNCTIONS

/// Record tool execution metrics
pub fn record_tool_execution(
    tool_type: &str,
    tool_name: &str,
    duration: Duration,
    success: bool,
) {
    let status = if success { "success" } else { "failure" };

    TOOL_EXECUTION_DURATION
        .with_label_values(&[tool_type, tool_name, status])
        .observe(duration.as_secs_f64());

    TOOL_EXECUTION_COUNTER
        .with_label_values(&[tool_type, tool_name, status])
        .inc();
}

/// Record database query metrics
pub fn record_db_query(operation: &str, table: &str, duration: Duration) {
    DB_QUERY_DURATION
        .with_label_values(&[operation, table])
        .observe(duration.as_secs_f64());
}

/// Update database pool metrics
pub fn update_db_pool_metrics(pool_type: &str, size: usize, active: usize) {
    DB_CONNECTION_POOL_SIZE
        .with_label_values(&[pool_type])
        .set(size as f64);

    DB_CONNECTION_POOL_ACTIVE
        .with_label_values(&[pool_type])
        .set(active as f64);
}

/// Record LLM request metrics
pub fn record_llm_request(
    provider: &str,
    model: &str,
    duration: Duration,
    success: bool,
    input_tokens: u64,
    output_tokens: u64,
) {
    let status = if success { "success" } else { "failure" };

    LLM_REQUEST_DURATION
        .with_label_values(&[provider, model, status])
        .observe(duration.as_secs_f64());

    LLM_TOKEN_COUNTER
        .with_label_values(&[provider, model, "input"])
        .inc_by(input_tokens as f64);

    LLM_TOKEN_COUNTER
        .with_label_values(&[provider, model, "output"])
        .inc_by(output_tokens as f64);
}

/// Update WebSocket connection metrics
pub fn update_ws_connections(delta: i64) {
    if delta > 0 {
        WS_CONNECTIONS.add(delta as f64);
    } else {
        WS_CONNECTIONS.sub((-delta) as f64);
    }
}

/// Record WebSocket message
pub fn record_ws_message(direction: &str, msg_type: &str) {
    WS_MESSAGE_COUNTER
        .with_label_values(&[direction, msg_type])
        .inc();
}

/// Update job queue metrics
pub fn update_job_queue_size(status: &str, size: usize) {
    JOB_QUEUE_SIZE
        .with_label_values(&[status])
        .set(size as f64);
}

/// Record job processing metrics
pub fn record_job_processing(job_type: &str, duration: Duration, success: bool) {
    let status = if success { "success" } else { "failure" };

    JOB_PROCESSING_DURATION
        .with_label_values(&[job_type, status])
        .observe(duration.as_secs_f64());
}

/// Record WASM module load time
pub fn record_wasm_load(module_name: &str, duration: Duration) {
    WASM_MODULE_LOAD_TIME
        .with_label_values(&[module_name])
        .observe(duration.as_secs_f64());
}

/// Record WASM execution metrics
pub fn record_wasm_execution(
    module_name: &str,
    function: &str,
    duration: Duration,
    fuel_consumed: u64,
) {
    WASM_EXECUTION_TIME
        .with_label_values(&[module_name, function])
        .observe(duration.as_secs_f64());

    WASM_FUEL_CONSUMED
        .with_label_values(&[module_name, function])
        .observe(fuel_consumed as f64);
}

/// Record TEE attestation metrics
pub fn record_tee_attestation(tee_type: &str, duration: Duration, success: bool) {
    let status = if success { "success" } else { "failure" };

    TEE_ATTESTATION_TIME
        .with_label_values(&[tee_type, status])
        .observe(duration.as_secs_f64());
}

/// Update TEE cache hit rate
pub fn update_tee_cache_hit_rate(tee_type: &str, hit_rate: f64) {
    TEE_CACHE_HIT_RATE
        .with_label_values(&[tee_type])
        .set(hit_rate);
}

/// Record HLLM regime switch
pub fn record_hllm_regime_switch(from: &str, to: &str, duration: Duration) {
    HLLM_REGIME_SWITCHES
        .with_label_values(&[from, to])
        .inc();

    HLLM_REGIME_SWITCH_TIME
        .with_label_values(&[from, to])
        .observe(duration.as_secs_f64());
}

/// Record container start time
pub fn record_container_start(runtime: &str, image: &str, duration: Duration) {
    CONTAINER_START_TIME
        .with_label_values(&[runtime, image])
        .observe(duration.as_secs_f64());
}

/// Update container pool metrics
pub fn update_container_pool(runtime: &str, status: &str, size: usize) {
    CONTAINER_POOL_SIZE
        .with_label_values(&[runtime, status])
        .set(size as f64);
}

// 🌀 INITIALIZATION

/// Initialize all metrics (call once at startup)
pub fn init_metrics() -> Result<(), Box<dyn std::error::Error>> {
    // Register all metrics with the global registry
    REGISTRY.register(Box::new(TOOL_EXECUTION_DURATION.clone()))?;
    REGISTRY.register(Box::new(TOOL_EXECUTION_COUNTER.clone()))?;
    REGISTRY.register(Box::new(DB_QUERY_DURATION.clone()))?;
    REGISTRY.register(Box::new(DB_CONNECTION_POOL_SIZE.clone()))?;
    REGISTRY.register(Box::new(DB_CONNECTION_POOL_ACTIVE.clone()))?;
    REGISTRY.register(Box::new(LLM_REQUEST_DURATION.clone()))?;
    REGISTRY.register(Box::new(LLM_TOKEN_COUNTER.clone()))?;
    REGISTRY.register(Box::new(WS_CONNECTIONS.clone()))?;
    REGISTRY.register(Box::new(WS_MESSAGE_COUNTER.clone()))?;
    REGISTRY.register(Box::new(JOB_QUEUE_SIZE.clone()))?;
    REGISTRY.register(Box::new(JOB_PROCESSING_DURATION.clone()))?;
    REGISTRY.register(Box::new(MEMORY_USAGE.clone()))?;
    REGISTRY.register(Box::new(CPU_USAGE.clone()))?;
    REGISTRY.register(Box::new(WASM_MODULE_LOAD_TIME.clone()))?;
    REGISTRY.register(Box::new(WASM_EXECUTION_TIME.clone()))?;
    REGISTRY.register(Box::new(WASM_FUEL_CONSUMED.clone()))?;
    REGISTRY.register(Box::new(TEE_ATTESTATION_TIME.clone()))?;
    REGISTRY.register(Box::new(TEE_CACHE_HIT_RATE.clone()))?;
    REGISTRY.register(Box::new(HLLM_REGIME_SWITCHES.clone()))?;
    REGISTRY.register(Box::new(HLLM_REGIME_SWITCH_TIME.clone()))?;
    REGISTRY.register(Box::new(CONTAINER_START_TIME.clone()))?;
    REGISTRY.register(Box::new(CONTAINER_POOL_SIZE.clone()))?;

    info!("⚡ Metrics system initialized - PERFORMANCE MATRIX ONLINE");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_perf_timer() {
        let timer = PerfTimer::new("test_operation")
            .with_label("component", "test");

        std::thread::sleep(Duration::from_millis(10));
        let duration = timer.stop();

        assert!(duration >= Duration::from_millis(10));
    }

    #[test]
    fn test_metrics_recording() {
        record_tool_execution("deno", "test_tool", Duration::from_secs(1), true);
        record_db_query("SELECT", "hanzo_tools", Duration::from_millis(5));
        update_db_pool_metrics("main", 10, 3);
        record_llm_request("openai", "gpt-4", Duration::from_secs(2), true, 100, 150);
    }

    #[tokio::test]
    async fn test_metrics_export() {
        init_metrics().ok();

        // Record some metrics
        record_tool_execution("python", "test", Duration::from_millis(100), true);

        // Export metrics
        let result = MetricsCollector::export_metrics().await;
        assert!(result.is_ok());

        let metrics_text = result.unwrap();
        assert!(metrics_text.contains("hanzo_tool_execution_duration_seconds"));
    }
}