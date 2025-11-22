//! MATRIX MODE: Performance benchmarks for Hanzo node
//!
//! Run with: cargo bench --bench performance_bench

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use hanzo_node::monitoring::PerfTimer;
use hanzo_db_sqlite::{SqliteManager, performance::*};
use hanzo_messages::schemas::hanzo_name::HanzoName;
use hanzo_embed::model_type::EmbeddingModelType;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use tokio::runtime::Runtime;

/// Benchmark database operations
fn bench_database_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("bench.db");

    // Initialize database
    let db = rt.block_on(async {
        SqliteManager::new(
            &db_path,
            "http://localhost:3690".to_string(),
            EmbeddingModelType::MiniLML6V2,
        ).unwrap()
    });

    let db = Arc::new(db);

    let mut group = c.benchmark_group("database");

    // Benchmark single insert
    group.bench_function("single_insert", |b| {
        b.iter(|| {
            rt.block_on(async {
                let conn = db.get_connection().unwrap();
                conn.execute(
                    "INSERT INTO test_bench (id, data) VALUES (?1, ?2)",
                    &[&black_box(1), &black_box("test_data")],
                ).ok();
            });
        });
    });

    // Benchmark batch insert
    group.bench_function("batch_insert_100", |b| {
        b.iter(|| {
            rt.block_on(async {
                let conn = db.get_connection().unwrap();
                let tx = conn.transaction().unwrap();
                for i in 0..100 {
                    tx.execute(
                        "INSERT INTO test_bench (id, data) VALUES (?1, ?2)",
                        &[&black_box(i), &black_box("test_data")],
                    ).ok();
                }
                tx.commit().ok();
            });
        });
    });

    // Benchmark query with index
    group.bench_function("indexed_query", |b| {
        b.iter(|| {
            rt.block_on(async {
                let conn = db.get_connection().unwrap();
                conn.query_row(
                    "SELECT data FROM test_bench WHERE id = ?1",
                    &[&black_box(50)],
                    |_row| Ok(()),
                ).ok();
            });
        });
    });

    // Benchmark connection pool
    group.bench_function("connection_pool_get", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _conn = db.get_connection().unwrap();
                // Connection automatically returned to pool when dropped
            });
        });
    });

    group.finish();
}

/// Benchmark clone vs reference operations
fn bench_clone_vs_ref(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory");

    // Create test data
    let large_string = "x".repeat(10000);
    let large_vec: Vec<String> = (0..1000).map(|i| format!("item_{}", i)).collect();

    // Benchmark string clone
    group.bench_function("string_clone", |b| {
        b.iter(|| {
            let _cloned = black_box(large_string.clone());
        });
    });

    // Benchmark string reference
    group.bench_function("string_ref", |b| {
        b.iter(|| {
            let _ref = black_box(&large_string);
        });
    });

    // Benchmark vec clone
    group.bench_function("vec_clone", |b| {
        b.iter(|| {
            let _cloned = black_box(large_vec.clone());
        });
    });

    // Benchmark vec reference
    group.bench_function("vec_ref", |b| {
        b.iter(|| {
            let _ref = black_box(&large_vec);
        });
    });

    // Benchmark Arc creation vs clone
    let arc_data = Arc::new(large_vec.clone());

    group.bench_function("arc_new", |b| {
        b.iter(|| {
            let _arc = Arc::new(black_box(large_vec.clone()));
        });
    });

    group.bench_function("arc_clone", |b| {
        b.iter(|| {
            let _arc = black_box(arc_data.clone());
        });
    });

    group.finish();
}

/// Benchmark async task spawning patterns
fn bench_async_patterns(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("async");

    // Benchmark tokio::spawn without error handling
    group.bench_function("spawn_basic", |b| {
        b.iter(|| {
            rt.block_on(async {
                let handle = tokio::spawn(async {
                    black_box(42)
                });
                handle.await.ok();
            });
        });
    });

    // Benchmark tokio::spawn with proper error handling
    group.bench_function("spawn_with_error_handling", |b| {
        b.iter(|| {
            rt.block_on(async {
                let handle = tokio::spawn(async move {
                    Ok::<_, Box<dyn std::error::Error>>(black_box(42))
                });

                match handle.await {
                    Ok(Ok(result)) => black_box(result),
                    Ok(Err(_e)) => 0,
                    Err(_e) => 0,
                };
            });
        });
    });

    // Benchmark parallel execution
    group.bench_function("parallel_spawn_10", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut handles = Vec::with_capacity(10);

                for i in 0..10 {
                    handles.push(tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_micros(1)).await;
                        black_box(i)
                    }));
                }

                for handle in handles {
                    handle.await.ok();
                }
            });
        });
    });

    // Benchmark sequential vs parallel
    let work_items: Vec<i32> = (0..100).collect();

    group.bench_function("sequential_processing", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut results = Vec::with_capacity(work_items.len());
                for item in &work_items {
                    results.push(black_box(item * 2));
                }
                results
            });
        });
    });

    group.bench_function("parallel_processing", |b| {
        b.iter(|| {
            rt.block_on(async {
                use futures::future::join_all;

                let futures = work_items.iter().map(|item| async move {
                    black_box(item * 2)
                });

                join_all(futures).await
            });
        });
    });

    group.finish();
}

/// Benchmark WASM runtime operations
fn bench_wasm_runtime(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("wasm");

    // Simple WASM module (add function)
    let wat = r#"
        (module
            (func $add (export "add") (param i32 i32) (result i32)
                local.get 0
                local.get 1
                i32.add
            )
        )
    "#;

    // Skip if wat crate not available
    if cfg!(feature = "wasm") {
        group.bench_function("module_load", |b| {
            b.iter(|| {
                rt.block_on(async {
                    // Simulate module loading
                    tokio::time::sleep(Duration::from_micros(100)).await;
                    black_box(wat.len())
                });
            });
        });

        group.bench_function("function_call", |b| {
            b.iter(|| {
                rt.block_on(async {
                    // Simulate WASM function call
                    let result = black_box(42) + black_box(37);
                    black_box(result)
                });
            });
        });
    }

    group.finish();
}

/// Benchmark container operations
fn bench_container_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("containers");

    // Simulate container pool operations
    let pool_size = 10;
    let containers: Vec<String> = (0..pool_size)
        .map(|i| format!("container_{}", i))
        .collect();

    group.bench_function("pool_get", |b| {
        let mut index = 0;
        b.iter(|| {
            let container = &containers[index % pool_size];
            index += 1;
            black_box(container)
        });
    });

    group.bench_function("pool_resize", |b| {
        b.iter(|| {
            let mut pool = containers.clone();
            for _ in 0..5 {
                pool.push(format!("container_{}", pool.len()));
            }
            black_box(pool.len())
        });
    });

    group.finish();
}

/// Benchmark JSON serialization/deserialization
fn bench_json_operations(c: &mut Criterion) {
    use serde_json::{json, Value};
    let mut group = c.benchmark_group("json");

    let small_json = json!({
        "tool": "test_tool",
        "parameters": {
            "input": "test"
        }
    });

    let large_json = json!({
        "tools": (0..100).map(|i| {
            json!({
                "id": format!("tool_{}", i),
                "name": format!("Tool {}", i),
                "config": {
                    "param1": i,
                    "param2": format!("value_{}", i),
                    "param3": vec![i, i+1, i+2]
                }
            })
        }).collect::<Vec<_>>(),
        "metadata": {
            "timestamp": "2024-01-01T00:00:00Z",
            "version": "1.0.0"
        }
    });

    group.bench_function("serialize_small", |b| {
        b.iter(|| {
            let serialized = serde_json::to_string(&small_json).unwrap();
            black_box(serialized)
        });
    });

    group.bench_function("deserialize_small", |b| {
        let serialized = serde_json::to_string(&small_json).unwrap();
        b.iter(|| {
            let value: Value = serde_json::from_str(&serialized).unwrap();
            black_box(value)
        });
    });

    group.bench_function("serialize_large", |b| {
        b.iter(|| {
            let serialized = serde_json::to_string(&large_json).unwrap();
            black_box(serialized)
        });
    });

    group.bench_function("deserialize_large", |b| {
        let serialized = serde_json::to_string(&large_json).unwrap();
        b.iter(|| {
            let value: Value = serde_json::from_str(&serialized).unwrap();
            black_box(value)
        });
    });

    group.finish();
}

/// Benchmark HLLM regime switching
fn bench_hllm_regime_switching(c: &mut Criterion) {
    let mut group = c.benchmark_group("hllm");

    // Simulate regime configurations
    let regimes = vec!["fast", "balanced", "quality", "creative"];

    group.bench_function("regime_switch", |b| {
        let mut current = 0;
        b.iter(|| {
            let from = &regimes[current % regimes.len()];
            let to = &regimes[(current + 1) % regimes.len()];
            current += 1;

            // Simulate regime switch operations
            black_box((from, to))
        });
    });

    group.finish();
}

/// Benchmark TEE attestation operations
fn bench_tee_attestation(c: &mut Criterion) {
    let mut group = c.benchmark_group("tee");

    // Simulate attestation cache
    let mut cache = std::collections::HashMap::new();
    for i in 0..100 {
        cache.insert(format!("key_{}", i), vec![0u8; 256]);
    }

    group.bench_function("cache_lookup", |b| {
        let mut index = 0;
        b.iter(|| {
            let key = format!("key_{}", index % 100);
            index += 1;
            let value = cache.get(&key);
            black_box(value)
        });
    });

    group.bench_function("attestation_verify", |b| {
        b.iter(|| {
            // Simulate cryptographic verification
            let data = vec![0u8; 256];
            let hash = blake3::hash(&data);
            black_box(hash)
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_database_operations,
    bench_clone_vs_ref,
    bench_async_patterns,
    bench_wasm_runtime,
    bench_container_ops,
    bench_json_operations,
    bench_hllm_regime_switching,
    bench_tee_attestation
);

criterion_main!(benches);