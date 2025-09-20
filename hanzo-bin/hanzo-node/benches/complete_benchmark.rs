//! Ultimate Performance Benchmarks for Hanzo Node
//! Benchmarks all runtime engines, database ops, WASM loading, and TEE attestation

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use hanzo_node::tools::tool_execution::*;
use hanzo_node::security::{TEEAttestation, AttestationCache, PrivacyTier};
use hanzo_node::managers::{NodeManager, JobQueueManager};
use hanzo_kbs::{AttestationReport, TEEEnvironment};
use hanzo_wasm_runtime::WasmRuntime;
use hanzo_sqlite::Database;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::runtime::Runtime;
use std::time::Duration;

/// Benchmark all runtime execution speeds
fn benchmark_runtime_engines(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("runtime_engines");

    // Benchmark Native Rust execution
    group.bench_function("native_rust", |b| {
        b.to_async(&rt).iter(|| async {
            let engine = NativeEngine::new();
            let result = engine.execute_tool(
                "echo",
                black_box(serde_json::json!({"message": "benchmark"})),
                None
            ).await;
            result
        });
    });

    // Benchmark Deno JavaScript execution
    group.bench_function("deno_javascript", |b| {
        let engine = rt.block_on(async {
            DenoEngine::new().await.unwrap()
        });
        let code = r#"
            export default async function(params) {
                return { result: params.message.toUpperCase() };
            }
        "#;

        b.to_async(&rt).iter(|| async {
            let result = engine.execute_code(
                black_box(code),
                black_box(serde_json::json!({"message": "benchmark"})),
                Duration::from_secs(10)
            ).await;
            result
        });
    });

    // Benchmark Python UV execution
    group.bench_function("python_uv", |b| {
        let engine = rt.block_on(async {
            PythonEngine::new().await.unwrap()
        });
        let code = r#"
def main(params):
    return {"result": params["message"].upper()}
        "#;

        b.to_async(&rt).iter(|| async {
            let result = engine.execute_code(
                black_box(code),
                black_box(serde_json::json!({"message": "benchmark"})),
                Duration::from_secs(10)
            ).await;
            result
        });
    });

    // Benchmark WASM execution
    group.bench_function("wasm", |b| {
        let engine = rt.block_on(async {
            WASMEngine::new().await.unwrap()
        });
        // Minimal valid WASM module
        let wasm_bytes = wat::parse_str(r#"
            (module
                (func $add (param i32 i32) (result i32)
                    local.get 0
                    local.get 1
                    i32.add)
                (export "add" (func $add))
            )
        "#).unwrap();

        b.to_async(&rt).iter(|| async {
            let result = engine.execute_module(
                black_box(&wasm_bytes),
                "add",
                black_box(serde_json::json!({"a": 5, "b": 3})),
                Duration::from_secs(5)
            ).await;
            result
        });
    });

    // Benchmark Docker execution (if available)
    if rt.block_on(DockerEngine::new()).is_ok() {
        group.bench_function("docker", |b| {
            let engine = rt.block_on(DockerEngine::new()).unwrap();

            b.to_async(&rt).iter(|| async {
                let result = engine.execute_container(
                    black_box("alpine:latest"),
                    black_box(vec!["echo", "benchmark"]),
                    None,
                    Duration::from_secs(30)
                ).await;
                result
            });
        });
    }

    // Benchmark MCP execution (if server available)
    if rt.block_on(MCPEngine::new(vec!["http://localhost:3000".to_string()])).is_ok() {
        group.bench_function("mcp", |b| {
            let engine = rt.block_on(MCPEngine::new(vec!["http://localhost:3000".to_string()])).unwrap();

            b.to_async(&rt).iter(|| async {
                let result = engine.call_tool(
                    black_box("test"),
                    black_box("echo"),
                    black_box(serde_json::json!({"message": "benchmark"})),
                    Duration::from_secs(10)
                ).await;
                result
            });
        });
    }

    group.finish();
}

/// Benchmark database operations
fn benchmark_database_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("database_operations");

    // Setup test database
    let db = rt.block_on(async {
        let db_path = "/tmp/benchmark_test.db";
        Database::new(db_path).await.unwrap()
    });

    // Benchmark INSERT operations
    group.bench_function("insert_job", |b| {
        b.to_async(&rt).iter(|| async {
            let job_id = uuid::Uuid::new_v4().to_string();
            db.execute(
                "INSERT INTO jobs (id, user_id, status, created_at) VALUES (?, ?, ?, ?)",
                params![
                    black_box(&job_id),
                    black_box("test_user"),
                    black_box("pending"),
                    black_box(chrono::Utc::now().timestamp())
                ]
            ).await;
        });
    });

    // Benchmark SELECT operations
    group.bench_function("select_jobs", |b| {
        b.to_async(&rt).iter(|| async {
            let result = db.query(
                "SELECT * FROM jobs WHERE user_id = ? LIMIT 100",
                params![black_box("test_user")]
            ).await;
            result
        });
    });

    // Benchmark UPDATE operations
    group.bench_function("update_job_status", |b| {
        b.to_async(&rt).iter(|| async {
            db.execute(
                "UPDATE jobs SET status = ? WHERE id = ?",
                params![
                    black_box("completed"),
                    black_box("test_job_id")
                ]
            ).await;
        });
    });

    // Benchmark vector similarity search (pgvector-like)
    group.bench_function("vector_similarity", |b| {
        let embedding = vec![0.1_f32; 768]; // 768-dim embedding

        b.to_async(&rt).iter(|| async {
            // Simulated vector similarity search
            let query_embedding = black_box(&embedding);
            let result = db.query(
                "SELECT * FROM embeddings ORDER BY embedding <-> ? LIMIT 10",
                params![serde_json::to_string(query_embedding).unwrap()]
            ).await;
            result
        });
    });

    // Benchmark transaction throughput
    group.bench_function("transaction_batch", |b| {
        b.to_async(&rt).iter(|| async {
            db.transaction(|tx| async move {
                for i in 0..10 {
                    tx.execute(
                        "INSERT INTO logs (message, level) VALUES (?, ?)",
                        params![
                            black_box(format!("Log message {}", i)),
                            black_box("info")
                        ]
                    ).await?;
                }
                Ok(())
            }).await
        });
    });

    group.finish();
}

/// Benchmark WASM module loading
fn benchmark_wasm_loading(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("wasm_loading");

    // Small WASM module (< 1KB)
    let small_wasm = wat::parse_str(r#"
        (module
            (func $nop)
            (export "nop" (func $nop))
        )
    "#).unwrap();

    // Medium WASM module (~10KB)
    let medium_wasm = wat::parse_str(r#"
        (module
            (memory 1)
            (func $process (param i32) (result i32)
                local.get 0
                i32.const 42
                i32.mul)
            (export "process" (func $process))
            (export "memory" (memory 0))
        )
    "#).unwrap();

    // Benchmark small module loading
    group.bench_function("load_small_module", |b| {
        b.to_async(&rt).iter(|| async {
            let runtime = WasmRuntime::new().await.unwrap();
            let result = runtime.load_module(black_box(&small_wasm)).await;
            result
        });
    });

    // Benchmark medium module loading
    group.bench_function("load_medium_module", |b| {
        b.to_async(&rt).iter(|| async {
            let runtime = WasmRuntime::new().await.unwrap();
            let result = runtime.load_module(black_box(&medium_wasm)).await;
            result
        });
    });

    // Benchmark module instantiation
    group.bench_function("instantiate_module", |b| {
        let runtime = rt.block_on(async {
            let rt = WasmRuntime::new().await.unwrap();
            rt.load_module(&small_wasm).await.unwrap();
            rt
        });

        b.to_async(&rt).iter(|| async {
            let result = runtime.instantiate().await;
            result
        });
    });

    // Benchmark WASM function calls
    group.bench_function("wasm_function_call", |b| {
        let runtime = rt.block_on(async {
            let rt = WasmRuntime::new().await.unwrap();
            rt.load_module(&medium_wasm).await.unwrap();
            rt.instantiate().await.unwrap();
            rt
        });

        b.to_async(&rt).iter(|| async {
            let result = runtime.call_function(
                "process",
                black_box(&[42])
            ).await;
            result
        });
    });

    // Benchmark memory operations
    group.bench_function("wasm_memory_ops", |b| {
        let runtime = rt.block_on(async {
            let rt = WasmRuntime::new().await.unwrap();
            rt.load_module(&medium_wasm).await.unwrap();
            rt.instantiate().await.unwrap();
            rt
        });

        b.to_async(&rt).iter(|| async {
            // Write and read from WASM memory
            let data = black_box(vec![0u8; 1024]);
            runtime.write_memory(0, &data).await.unwrap();
            let result = runtime.read_memory(0, 1024).await;
            result
        });
    });

    group.finish();
}

/// Benchmark TEE attestation
fn benchmark_tee_attestation(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("tee_attestation");

    let cache = Arc::new(RwLock::new(AttestationCache::new()));

    // Benchmark Tier 0 (Open) - no attestation
    group.bench_function("tier0_open", |b| {
        let cache_clone = cache.clone();

        b.to_async(&rt).iter(|| async {
            let result = TEEAttestation::verify_tier(
                black_box(PrivacyTier::Open),
                None,
                &cache_clone
            ).await;
            result
        });
    });

    // Benchmark SEV-SNP attestation
    group.bench_function("sev_snp_attestation", |b| {
        let cache_clone = cache.clone();
        let report = AttestationReport {
            tee_type: TEEEnvironment::SEVSNP,
            report_data: vec![0u8; 64],
            measurement: vec![0u8; 48],
            timestamp: chrono::Utc::now(),
            signature: vec![0u8; 512],
        };

        b.to_async(&rt).iter(|| async {
            let result = TEEAttestation::verify_tier(
                black_box(PrivacyTier::BasicTEE),
                black_box(Some(report.clone())),
                &cache_clone
            ).await;
            result
        });
    });

    // Benchmark TDX attestation
    group.bench_function("tdx_attestation", |b| {
        let cache_clone = cache.clone();
        let report = AttestationReport {
            tee_type: TEEEnvironment::TDX,
            report_data: vec![0u8; 64],
            measurement: vec![0u8; 48],
            timestamp: chrono::Utc::now(),
            signature: vec![0u8; 512],
        };

        b.to_async(&rt).iter(|| async {
            let result = TEEAttestation::verify_tier(
                black_box(PrivacyTier::EnhancedTEE),
                black_box(Some(report.clone())),
                &cache_clone
            ).await;
            result
        });
    });

    // Benchmark H100 CC attestation
    group.bench_function("h100_cc_attestation", |b| {
        let cache_clone = cache.clone();
        let report = AttestationReport {
            tee_type: TEEEnvironment::H100CC,
            report_data: vec![0u8; 64],
            measurement: vec![0u8; 48],
            timestamp: chrono::Utc::now(),
            signature: vec![0u8; 768],
        };

        b.to_async(&rt).iter(|| async {
            let result = TEEAttestation::verify_tier(
                black_box(PrivacyTier::ConfidentialCompute),
                black_box(Some(report.clone())),
                &cache_clone
            ).await;
            result
        });
    });

    // Benchmark Blackwell TEE-I/O attestation
    group.bench_function("blackwell_tee_io", |b| {
        let cache_clone = cache.clone();
        let report = AttestationReport {
            tee_type: TEEEnvironment::BlackwellTEEIO,
            report_data: vec![0u8; 128],
            measurement: vec![0u8; 64],
            timestamp: chrono::Utc::now(),
            signature: vec![0u8; 1024],
        };

        b.to_async(&rt).iter(|| async {
            let result = TEEAttestation::verify_tier(
                black_box(PrivacyTier::TEEIO),
                black_box(Some(report.clone())),
                &cache_clone
            ).await;
            result
        });
    });

    // Benchmark cache performance
    group.bench_function("attestation_cache_hit", |b| {
        let cache_clone = cache.clone();
        // Pre-populate cache
        rt.block_on(async {
            let report = AttestationReport {
                tee_type: TEEEnvironment::SEVSNP,
                report_data: vec![0u8; 64],
                measurement: vec![0u8; 48],
                timestamp: chrono::Utc::now(),
                signature: vec![0u8; 512],
            };
            cache_clone.write().await.insert("test_key", report);
        });

        b.to_async(&rt).iter(|| async {
            let result = cache_clone.read().await.get(black_box("test_key"));
            result
        });
    });

    group.finish();
}

/// Benchmark end-to-end workflow
fn benchmark_e2e_workflow(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("e2e_workflow");

    // Setup components
    let node = rt.block_on(async {
        Arc::new(RwLock::new(
            NodeManager::new_test("benchmark_node".to_string()).await
        ))
    });

    // Benchmark complete job submission and execution
    group.bench_function("complete_job_flow", |b| {
        let node_clone = node.clone();

        b.to_async(&rt).iter(|| async {
            let job_queue = JobQueueManager::new(node_clone.clone());

            // Submit job
            let job = JobForWorker {
                job_id: uuid::Uuid::new_v4().to_string(),
                user_id: "bench_user".to_string(),
                prompt: "Process this data".to_string(),
                tool_calls: vec![
                    ToolCallConfig {
                        tool_name: "processor".to_string(),
                        runtime: ExecutionEngine::Native,
                        privacy_tier: PrivacyTier::Open,
                        timeout_ms: 5000,
                    }
                ],
                hardware_requirements: Default::default(),
            };

            let job_id = job_queue.submit_job(black_box(job)).await.unwrap();

            // Process job (simulated)
            tokio::time::sleep(Duration::from_millis(10)).await;

            // Complete job
            job_queue.complete_job(
                &job_id,
                serde_json::json!({"result": "processed"})
            ).await.unwrap();

            job_id
        });
    });

    // Benchmark parallel job processing
    group.bench_function("parallel_jobs", |b| {
        let node_clone = node.clone();

        b.to_async(&rt).iter(|| async {
            let job_queue = JobQueueManager::new(node_clone.clone());

            // Submit multiple jobs in parallel
            let futures = (0..10).map(|i| {
                let jq = job_queue.clone();
                async move {
                    let job = JobForWorker {
                        job_id: format!("job_{}", i),
                        user_id: "bench_user".to_string(),
                        prompt: format!("Process job {}", i),
                        tool_calls: vec![],
                        hardware_requirements: Default::default(),
                    };
                    jq.submit_job(job).await
                }
            });

            let results = futures::future::join_all(futures).await;
            results
        });
    });

    group.finish();
}

// Define benchmark groups
criterion_group!(
    benches,
    benchmark_runtime_engines,
    benchmark_database_operations,
    benchmark_wasm_loading,
    benchmark_tee_attestation,
    benchmark_e2e_workflow
);

criterion_main!(benches);