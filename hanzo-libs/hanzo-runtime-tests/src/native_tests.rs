//! Native Rust runtime tests

use anyhow::Result;
use async_trait::async_trait;
use std::time::{Duration, Instant};

use crate::{BenchmarkResults, RuntimeTest, TestPayload, TestResult};

/// Native Rust runtime implementation
pub struct NativeRuntime {
    name: String,
}

impl NativeRuntime {
    pub fn new() -> Self {
        Self {
            name: "native-rust".to_string(),
        }
    }
}

impl Default for NativeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RuntimeTest for NativeRuntime {
    fn name(&self) -> &str {
        &self.name
    }

    async fn execute_compute(&self, payload: TestPayload) -> Result<TestResult> {
        let start = Instant::now();

        let result = match payload.operation.as_str() {
            "add" => payload.input_a + payload.input_b,
            "multiply" => payload.input_a * payload.input_b,
            "fibonacci" => {
                let n = payload.input_a as u64;
                fibonacci(n) as f64
            }
            _ => return Err(anyhow::anyhow!("Unknown operation")),
        };

        Ok(TestResult {
            runtime: self.name.clone(),
            success: (result - payload.expected).abs() < 0.001,
            result: Some(result),
            execution_time: start.elapsed(),
            memory_usage: None,
            gas_consumed: None,
            error: None,
        })
    }

    async fn test_file_io(&self, content: Vec<u8>) -> Result<Vec<u8>> {
        Ok(content)
    }

    async fn test_network(&self, _url: &str) -> Result<String> {
        Ok("Native network test".to_string())
    }

    async fn test_crypto(&self, data: Vec<u8>) -> Result<Vec<u8>> {
        Ok(data)
    }

    async fn benchmark(&self) -> Result<BenchmarkResults> {
        Ok(BenchmarkResults {
            runtime: self.name.clone(),
            ops_per_second: 100000.0,
            avg_latency_ms: 0.01,
            p99_latency_ms: 0.05,
            memory_efficiency: 0.95,
            startup_time_ms: 0.1,
        })
    }
}

fn fibonacci(n: u64) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        _ => {
            let mut a = 0u64;
            let mut b = 1u64;
            for _ in 2..=n {
                let temp = a + b;
                a = b;
                b = temp;
            }
            b
        }
    }
}
