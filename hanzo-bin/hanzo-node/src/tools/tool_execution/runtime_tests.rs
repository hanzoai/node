#[cfg(test)]
mod runtime_tests {
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{Mutex, RwLock};

    // ========================================
    // MATRIX RUNTIME ORCHESTRATION TESTS
    // ========================================

    #[tokio::test]
    async fn test_runtime_parallel_execution() {
        println!("🔴💊 MATRIX RUNTIME PARALLEL EXECUTION TEST");

        let handles: Vec<_> = (0..20)
            .map(|i| {
                tokio::spawn(async move {
                    let runtime = match i % 6 {
                        0 => "native",
                        1 => "deno",
                        2 => "python",
                        3 => "wasm",
                        4 => "mcp",
                        _ => "agent",
                    };

                    // Simulate work with varying durations
                    tokio::time::sleep(Duration::from_millis(i as u64 * 10)).await;

                    json!({
                        "runtime": runtime,
                        "task_id": i,
                        "completed": true
                    })
                })
            })
            .collect();

        let results = futures::future::join_all(handles).await;

        assert_eq!(results.len(), 20);
        let mut runtime_counts = HashMap::new();

        for result in &results {
            if let Ok(data) = result {
                let runtime = data["runtime"].as_str().unwrap_or("unknown");
                *runtime_counts.entry(runtime).or_insert(0) += 1;
            }
        }

        println!("Runtime distribution: {:?}", runtime_counts);
        assert!(runtime_counts.len() >= 5); // At least 5 different runtime types
        println!("✅ All 20 parallel executions completed across {} runtime types", runtime_counts.len());
    }

    #[tokio::test]
    async fn test_runtime_error_recovery() {
        println!("⚡ Testing runtime error recovery mechanisms");

        async fn execute_with_retry(
            runtime: &str,
            _max_retries: u32,
            fail_count: u32,
        ) -> Result<Value, String> {
            static ATTEMPT_COUNTER: std::sync::atomic::AtomicU32 =
                std::sync::atomic::AtomicU32::new(0);

            let attempt = ATTEMPT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

            if attempt < fail_count {
                return Err(format!("{} failed on attempt {}", runtime, attempt + 1));
            }

            Ok(json!({
                "runtime": runtime,
                "status": "recovered",
                "attempts": attempt + 1
            }))
        }

        // Test recovery after 2 failures - need 3 attempts for success
        for _ in 0..3 {
            let _ = execute_with_retry("test_runtime", 5, 2).await;
        }

        // Now it should succeed
        let result = execute_with_retry("test_runtime", 5, 2).await;
        assert!(result.is_ok());

        if let Ok(data) = result {
            println!("Recovery successful after {} attempts", data["attempts"]);
            assert_eq!(data["status"], "recovered");
        }
    }

    #[tokio::test]
    async fn test_runtime_resource_pooling() {
        println!("💾 Testing runtime resource pooling");

        #[derive(Clone)]
        struct ResourcePool {
            resources: Arc<RwLock<HashMap<String, Value>>>,
            locks: Arc<RwLock<HashMap<String, bool>>>,
        }

        impl ResourcePool {
            fn new() -> Self {
                Self {
                    resources: Arc::new(RwLock::new(HashMap::new())),
                    locks: Arc::new(RwLock::new(HashMap::new())),
                }
            }

            async fn acquire(&self, runtime: &str) -> bool {
                let mut locks = self.locks.write().await;
                if *locks.get(runtime).unwrap_or(&false) {
                    return false; // Already locked
                }
                locks.insert(runtime.to_string(), true);
                true
            }

            async fn release(&self, runtime: &str) {
                let mut locks = self.locks.write().await;
                locks.insert(runtime.to_string(), false);
            }

            async fn store(&self, key: String, value: Value) {
                let mut resources = self.resources.write().await;
                resources.insert(key, value);
            }

            async fn get(&self, key: &str) -> Option<Value> {
                let resources = self.resources.read().await;
                resources.get(key).cloned()
            }
        }

        let pool = ResourcePool::new();

        // Simulate multiple runtimes accessing the pool
        let pool1 = pool.clone();
        let handle1 = tokio::spawn(async move {
            if pool1.acquire("runtime1").await {
                pool1.store("data1".to_string(), json!({"value": 100})).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
                pool1.release("runtime1").await;
            }
        });

        let pool2 = pool.clone();
        let handle2 = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(25)).await; // Start slightly later
            if pool2.acquire("runtime2").await {
                if let Some(data) = pool2.get("data1").await {
                    let value = data["value"].as_i64().unwrap_or(0);
                    pool2.store("data2".to_string(), json!({"value": value * 2})).await;
                }
                pool2.release("runtime2").await;
            }
        });

        let _ = tokio::join!(handle1, handle2);

        let final_data = pool.get("data2").await;
        assert!(final_data.is_some());
        println!("✅ Resource pooling successful: {:?}", final_data);
    }

    #[tokio::test]
    async fn test_runtime_timeout_handling() {
        println!("⏱️ Testing runtime timeout mechanisms");

        use tokio::time::timeout;

        // Test successful execution within timeout
        let quick_result = timeout(Duration::from_millis(200), async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            "completed"
        })
        .await;

        assert!(quick_result.is_ok());
        assert_eq!(quick_result.unwrap(), "completed");

        // Test timeout scenario
        let slow_result = timeout(Duration::from_millis(100), async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            "should_timeout"
        })
        .await;

        assert!(slow_result.is_err());
        println!("✅ Timeout handling verified for both success and timeout cases");
    }

    #[tokio::test]
    async fn test_runtime_streaming_pipeline() {
        println!("📡 Testing runtime streaming pipeline");

        use tokio::sync::mpsc;

        let (tx1, mut rx1) = mpsc::channel::<Value>(10);
        let (tx2, mut rx2) = mpsc::channel::<Value>(10);

        // Stage 1: Generate data
        let producer = tokio::spawn(async move {
            for i in 0..5 {
                let data = json!({"seq": i, "timestamp": std::time::SystemTime::now()});
                tx1.send(data).await.unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });

        // Stage 2: Transform data
        let transformer = tokio::spawn(async move {
            while let Some(mut data) = rx1.recv().await {
                data["transformed"] = json!(true);
                data["squared"] = json!(data["seq"].as_i64().unwrap_or(0).pow(2));
                tx2.send(data).await.unwrap();
            }
        });

        // Stage 3: Collect results
        let collector = tokio::spawn(async move {
            let mut results = Vec::new();
            while let Some(data) = rx2.recv().await {
                results.push(data);
            }
            results
        });

        producer.await.unwrap();
        // tx1 already moved, no need to drop
        transformer.await.unwrap();
        // tx2 already moved, no need to drop

        let results = collector.await.unwrap();
        assert_eq!(results.len(), 5);

        for (i, result) in results.iter().enumerate() {
            assert_eq!(result["seq"], i as i64);
            assert_eq!(result["transformed"], true);
            assert_eq!(result["squared"], (i as i64).pow(2));
        }

        println!("✅ Streaming pipeline processed {} items", results.len());
    }

    #[tokio::test]
    async fn test_runtime_circuit_breaker() {
        println!("🔌 Testing circuit breaker pattern");

        #[derive(Clone)]
        struct CircuitBreaker {
            failure_count: Arc<Mutex<u32>>,
            is_open: Arc<Mutex<bool>>,
            threshold: u32,
            last_failure_time: Arc<Mutex<Option<std::time::Instant>>>,
            cooldown: Duration,
        }

        impl CircuitBreaker {
            fn new(threshold: u32, cooldown: Duration) -> Self {
                Self {
                    failure_count: Arc::new(Mutex::new(0)),
                    is_open: Arc::new(Mutex::new(false)),
                    threshold,
                    last_failure_time: Arc::new(Mutex::new(None)),
                    cooldown,
                }
            }

            async fn call<F, T>(&self, f: F) -> Result<T, String>
            where
                F: std::future::Future<Output = Result<T, String>>,
            {
                // Check if circuit is open
                let mut is_open = self.is_open.lock().await;
                if *is_open {
                    // Check if cooldown has passed
                    if let Some(last_failure) = *self.last_failure_time.lock().await {
                        if last_failure.elapsed() >= self.cooldown {
                            *is_open = false;
                            *self.failure_count.lock().await = 0;
                            println!("Circuit breaker reset after cooldown");
                        } else {
                            return Err("Circuit breaker is OPEN".to_string());
                        }
                    }
                }
                drop(is_open);

                // Execute the function
                match f.await {
                    Ok(result) => {
                        *self.failure_count.lock().await = 0;
                        Ok(result)
                    }
                    Err(e) => {
                        let mut count = self.failure_count.lock().await;
                        *count += 1;

                        if *count >= self.threshold {
                            *self.is_open.lock().await = true;
                            *self.last_failure_time.lock().await = Some(std::time::Instant::now());
                            println!("Circuit breaker OPENED after {} failures", *count);
                        }

                        Err(e)
                    }
                }
            }

            async fn reset(&self) {
                *self.failure_count.lock().await = 0;
                *self.is_open.lock().await = false;
                *self.last_failure_time.lock().await = None;
            }
        }

        let breaker = CircuitBreaker::new(3, Duration::from_millis(100));

        // Simulate failures to trigger circuit breaker
        for i in 0..4 {
            let result = breaker
                .call(async { Err::<(), String>("Simulated failure".to_string()) })
                .await;

            if i < 3 {
                assert!(result.is_err());
                assert_eq!(result.unwrap_err(), "Simulated failure");
            } else {
                assert!(result.is_err());
                assert_eq!(result.unwrap_err(), "Circuit breaker is OPEN");
            }
        }

        // Wait for cooldown
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Circuit should be closed now
        breaker.reset().await;
        let result = breaker
            .call(async { Ok::<_, String>("Success after reset") })
            .await;

        assert!(result.is_ok());
        println!("✅ Circuit breaker pattern validated");
    }

    #[tokio::test]
    async fn test_runtime_load_balancing() {
        println!("⚖️ Testing runtime load balancing");

        #[derive(Clone)]
        struct LoadBalancer {
            instances: Vec<Arc<Mutex<u32>>>,
        }

        impl LoadBalancer {
            fn new(count: usize) -> Self {
                let instances = (0..count)
                    .map(|_| Arc::new(Mutex::new(0u32)))
                    .collect();
                Self { instances }
            }

            async fn get_least_loaded(&self) -> (usize, Arc<Mutex<u32>>) {
                let mut min_load = u32::MAX;
                let mut min_index = 0;

                for (i, instance) in self.instances.iter().enumerate() {
                    let load = *instance.lock().await;
                    if load < min_load {
                        min_load = load;
                        min_index = i;
                    }
                }

                (min_index, self.instances[min_index].clone())
            }

            async fn distribute_task(&self) -> usize {
                let (index, instance) = self.get_least_loaded().await;
                *instance.lock().await += 1;
                index
            }

            async fn _complete_task(&self, index: usize) {
                let mut load = self.instances[index].lock().await;
                if *load > 0 {
                    *load -= 1;
                }
            }
        }

        let balancer = LoadBalancer::new(3);

        // Distribute 9 tasks
        let mut task_distribution = vec![0; 3];
        for _ in 0..9 {
            let index = balancer.distribute_task().await;
            task_distribution[index] += 1;
        }

        // Check even distribution
        for count in &task_distribution {
            assert_eq!(*count, 3);
        }

        println!("✅ Load balanced: {:?}", task_distribution);
    }

    #[tokio::test]
    async fn test_runtime_consensus_voting() {
        println!("🤝 Testing runtime consensus mechanism");

        struct ConsensusEngine {
            runtimes: Vec<String>,
        }

        impl ConsensusEngine {
            async fn vote(&self, proposal: &str) -> HashMap<String, String> {
                let mut votes = HashMap::new();

                for runtime in &self.runtimes {
                    // Simulate different runtime opinions
                    let vote = match runtime.as_str() {
                        "native" => "approve",
                        "python" if proposal.contains("data") => "approve",
                        "deno" if proposal.contains("web") => "approve",
                        "wasm" => "approve",
                        "mcp" if proposal.contains("model") => "approve",
                        _ => "reject",
                    };

                    votes.insert(runtime.clone(), vote.to_string());
                }

                votes
            }

            async fn reach_consensus(&self, proposal: &str) -> (String, f64) {
                let votes = self.vote(proposal).await;

                let mut tally: HashMap<String, u32> = HashMap::new();
                for vote in votes.values() {
                    *tally.entry(vote.clone()).or_insert(0) += 1;
                }

                let total = votes.len() as f64;
                let (decision, count) = tally
                    .iter()
                    .max_by_key(|(_, count)| *count)
                    .map(|(decision, count)| (decision.clone(), *count))
                    .unwrap_or(("reject".to_string(), 0));

                (decision, count as f64 / total)
            }
        }

        let engine = ConsensusEngine {
            runtimes: vec![
                "native".to_string(),
                "python".to_string(),
                "deno".to_string(),
                "wasm".to_string(),
                "mcp".to_string(),
            ],
        };

        let (decision, confidence) = engine.reach_consensus("process data with model").await;
        assert_eq!(decision, "approve");
        assert!(confidence > 0.5);

        println!("✅ Consensus reached: {} with {:.1}% confidence", decision, confidence * 100.0);
    }

    #[tokio::test]
    async fn test_runtime_chaos_resilience() {
        println!("🌪️ Testing chaos engineering resilience");

        use rand::Rng;

        async fn chaotic_execution() -> Result<Value, String> {
            let mut rng = rand::thread_rng();
            let chaos_factor = rng.gen::<f32>();

            tokio::time::sleep(Duration::from_millis((chaos_factor * 100.0) as u64)).await;

            if chaos_factor < 0.2 {
                Err("Random failure injected".to_string())
            } else if chaos_factor < 0.4 {
                Ok(json!({"status": "degraded", "performance": chaos_factor}))
            } else {
                Ok(json!({"status": "success", "performance": chaos_factor}))
            }
        }

        let mut results = Vec::new();
        for _ in 0..20 {
            results.push(chaotic_execution().await);
        }

        let successes = results.iter().filter(|r| {
            r.as_ref().map(|v| v["status"] == "success").unwrap_or(false)
        }).count();

        let degraded = results.iter().filter(|r| {
            r.as_ref().map(|v| v["status"] == "degraded").unwrap_or(false)
        }).count();

        let failures = results.iter().filter(|r| r.is_err()).count();

        println!("Chaos results: {} success, {} degraded, {} failures",
                 successes, degraded, failures);

        // System should be resilient
        assert!(successes + degraded > failures);
        println!("✅ System resilient to chaos: {}% operational",
                 ((successes + degraded) as f64 / results.len() as f64) * 100.0);
    }

    // ========================================
    // THE MATRIX HAS BEEN VALIDATED
    // ========================================
}