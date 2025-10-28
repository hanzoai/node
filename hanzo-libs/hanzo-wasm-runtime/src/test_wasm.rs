//! Comprehensive tests for WASM runtime functionality

#[cfg(test)]
mod tests {
    use super::super::*;
    use serde_json::json;
    use wat;

    /// Test executing a simple add function
    #[tokio::test]
    async fn test_execute_add_function() {
        let wat = r#"
            (module
                (func $add (export "add") (param i32 i32) (result i32)
                    local.get 0
                    local.get 1
                    i32.add
                )
            )
        "#;

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let config = WasmConfig::default();
        let runtime = WasmRuntime::new(config).unwrap();

        // Load module
        let info = runtime.load_module("math".to_string(), wasm_bytes).await.unwrap();
        assert!(info.exports.contains(&"add".to_string()));

        // Execute add function with array parameters
        let result = runtime.execute("math", "add", json!([5, 3])).await.unwrap();
        assert_eq!(result, json!(8));

        // Execute with object parameters
        let result = runtime.execute("math", "add", json!({"a": 10, "b": 15})).await.unwrap();
        assert_eq!(result, json!(25));
    }

    /// Test executing a function that returns a string from memory
    #[tokio::test]
    async fn test_execute_string_function() {
        let wat = r#"
            (module
                (memory (export "memory") 1)

                ;; Store "Hello, WASM!" at offset 0
                (data (i32.const 0) "Hello, WASM!")

                (func $hello (export "hello") (result i32)
                    ;; Return pointer to string at offset 0
                    i32.const 0
                )
            )
        "#;

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let config = WasmConfig::default();
        let runtime = WasmRuntime::new(config).unwrap();

        runtime.load_module("greeter".to_string(), wasm_bytes).await.unwrap();
        let result = runtime.execute("greeter", "hello", json!(null)).await.unwrap();

        // Should return the string from memory
        assert_eq!(result, json!("Hello, WASM!"));
    }

    /// Test host function: logging
    #[tokio::test]
    async fn test_host_function_log() {
        let wat = r#"
            (module
                (import "env" "memory" (memory 1))
                (import "env" "log" (func $log (param i32 i32)))

                ;; Store test message at offset 0
                (data (i32.const 0) "Test log message")

                (func $test_log (export "test_log")
                    ;; Log the message
                    i32.const 0  ;; pointer
                    i32.const 16 ;; length
                    call $log
                )
            )
        "#;

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let config = WasmConfig::default();
        let runtime = WasmRuntime::new(config).unwrap();

        runtime.load_module("logger".to_string(), wasm_bytes).await.unwrap();

        // Execute should succeed and log the message
        let result = runtime.execute("logger", "test_log", json!(null)).await;
        assert!(result.is_ok());
    }

    /// Test multiple return values
    #[tokio::test]
    async fn test_multiple_returns() {
        let wat = r#"
            (module
                (func $divmod (export "divmod") (param i32 i32) (result i32 i32)
                    local.get 0
                    local.get 1
                    i32.div_s  ;; quotient

                    local.get 0
                    local.get 1
                    i32.rem_s  ;; remainder
                )
            )
        "#;

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let config = WasmConfig::default();
        let runtime = WasmRuntime::new(config).unwrap();

        runtime.load_module("math2".to_string(), wasm_bytes).await.unwrap();
        let result = runtime.execute("math2", "divmod", json!([17, 5])).await.unwrap();

        // Should return array with [quotient, remainder]
        assert_eq!(result, json!([3, 2]));
    }

    /// Test fuel limiting for infinite loops
    #[tokio::test]
    async fn test_fuel_limit() {
        let wat = r#"
            (module
                (func $infinite_loop (export "infinite_loop")
                    (loop $continue
                        br $continue
                    )
                )
            )
        "#;

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let mut config = WasmConfig::default();
        config.fuel_limit = Some(1000); // Very low fuel limit
        let runtime = WasmRuntime::new(config).unwrap();

        runtime.load_module("loops".to_string(), wasm_bytes).await.unwrap();
        let result = runtime.execute("loops", "infinite_loop", json!(null)).await;

        // Should fail due to fuel exhaustion
        assert!(result.is_err());
    }

    /// Test execution timeout
    #[tokio::test]
    async fn test_execution_timeout() {
        let wat = r#"
            (module
                (func $slow (export "slow") (result i32)
                    (local $counter i32)
                    ;; Simulate slow computation
                    i32.const 0
                    local.set $counter
                    (loop $continue
                        local.get $counter
                        i32.const 1
                        i32.add
                        local.tee $counter
                        i32.const 1000000000
                        i32.lt_s
                        br_if $continue
                    )
                    local.get $counter
                )
            )
        "#;

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let mut config = WasmConfig::default();
        config.max_execution_time = Duration::from_millis(100); // Very short timeout
        config.fuel_limit = None; // No fuel limit to test timeout instead
        let runtime = WasmRuntime::new(config).unwrap();

        runtime.load_module("slow_mod".to_string(), wasm_bytes).await.unwrap();
        let result = runtime.execute("slow_mod", "slow", json!(null)).await;

        // Should timeout
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("timeout"));
        }
    }

    /// Test JSON operations
    #[tokio::test]
    async fn test_json_operations() {
        let wat = r#"
            (module
                (import "env" "memory" (memory 1))
                (import "env" "json_parse" (func $json_parse (param i32 i32) (result i32)))
                (import "env" "json_stringify" (func $json_stringify (param i32) (result i32)))

                ;; Store JSON string at offset 0
                (data (i32.const 0) "{\"value\":42}")

                (func $process_json (export "process_json") (result i32)
                    ;; Parse the JSON
                    i32.const 0   ;; pointer to JSON string
                    i32.const 12  ;; length
                    call $json_parse

                    ;; Stringify it back
                    call $json_stringify
                )
            )
        "#;

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let config = WasmConfig::default();
        let runtime = WasmRuntime::new(config).unwrap();

        runtime.load_module("json_test".to_string(), wasm_bytes).await.unwrap();
        let result = runtime.execute("json_test", "process_json", json!(null)).await;

        // Should succeed
        assert!(result.is_ok());
    }

    /// Test execute_bytes for temporary modules
    #[tokio::test]
    async fn test_execute_bytes() {
        let wat = r#"
            (module
                (func $multiply (export "multiply") (param i32 i32) (result i32)
                    local.get 0
                    local.get 1
                    i32.mul
                )
            )
        "#;

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let config = WasmConfig::default();
        let runtime = WasmRuntime::new(config).unwrap();

        // Execute without explicitly loading
        let result = runtime.execute_bytes(wasm_bytes, "multiply", json!([6, 7])).await.unwrap();
        assert_eq!(result, json!(42));

        // Module should not be in the list
        assert_eq!(runtime.list_modules().await.len(), 0);
    }

    /// Test F32/F64 parameters and returns
    #[tokio::test]
    async fn test_float_operations() {
        let wat = r#"
            (module
                (func $sqrt_f32 (export "sqrt_f32") (param f32) (result f32)
                    local.get 0
                    f32.sqrt
                )

                (func $add_f64 (export "add_f64") (param f64 f64) (result f64)
                    local.get 0
                    local.get 1
                    f64.add
                )
            )
        "#;

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let config = WasmConfig::default();
        let runtime = WasmRuntime::new(config).unwrap();

        runtime.load_module("float_ops".to_string(), wasm_bytes).await.unwrap();

        // Test f32 sqrt
        let result = runtime.execute("float_ops", "sqrt_f32", json!(16.0)).await.unwrap();
        if let Value::Number(n) = result {
            assert!((n.as_f64().unwrap() - 4.0).abs() < 0.001);
        } else {
            panic!("Expected number result");
        }

        // Test f64 addition
        let result = runtime.execute("float_ops", "add_f64", json!([1.5, 2.5])).await.unwrap();
        assert_eq!(result, json!(4.0));
    }

    /// Test module management
    #[tokio::test]
    async fn test_module_management() {
        let wat = r#"
            (module
                (func $noop (export "noop"))
            )
        "#;

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let config = WasmConfig::default();
        let runtime = WasmRuntime::new(config).unwrap();

        // Load multiple modules
        runtime.load_module("mod1".to_string(), wasm_bytes.clone()).await.unwrap();
        runtime.load_module("mod2".to_string(), wasm_bytes.clone()).await.unwrap();
        runtime.load_module("mod3".to_string(), wasm_bytes).await.unwrap();

        // Check all are loaded
        let modules = runtime.list_modules().await;
        assert_eq!(modules.len(), 3);
        assert!(modules.contains(&"mod1".to_string()));
        assert!(modules.contains(&"mod2".to_string()));
        assert!(modules.contains(&"mod3".to_string()));

        // Unload one
        runtime.unload_module("mod2").await.unwrap();
        let modules = runtime.list_modules().await;
        assert_eq!(modules.len(), 2);
        assert!(!modules.contains(&"mod2".to_string()));

        // Clear all
        runtime.clear_modules().await;
        assert_eq!(runtime.list_modules().await.len(), 0);
    }
}