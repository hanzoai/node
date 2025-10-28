//! Performance monitoring and optimization module
//!
//! This module provides comprehensive metrics collection and performance tracking
//! for the Hanzo node, enabling real-time optimization and bottleneck identification.

pub mod metrics;

pub use metrics::{
    init_metrics,
    MetricsCollector,
    PerfTimer,
    record_tool_execution,
    record_db_query,
    update_db_pool_metrics,
    record_llm_request,
    update_ws_connections,
    record_ws_message,
    update_job_queue_size,
    record_job_processing,
    record_wasm_load,
    record_wasm_execution,
    record_tee_attestation,
    update_tee_cache_hit_rate,
    record_hllm_regime_switch,
    record_container_start,
    update_container_pool,
};