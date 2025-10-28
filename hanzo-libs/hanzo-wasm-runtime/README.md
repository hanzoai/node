# Hanzo WASM Runtime

A WebAssembly (WASM) runtime for executing sandboxed code within the Hanzo Node ecosystem. This runtime provides secure, deterministic execution of WASM modules with support for various data types, host functions, and resource limits.

## Features

- ✅ **Full WASM Execution**: Execute WebAssembly modules with proper parameter passing and result handling
- ✅ **Multiple Data Types**: Support for i32, i64, f32, f64 parameters and return values
- ✅ **Host Functions**: Built-in host functions for logging, JSON operations, HTTP requests, and memory management
- ✅ **Memory Management**: Proper memory allocation and string handling between host and guest
- ✅ **Resource Limits**: Configurable fuel limits and execution timeouts for safe execution
- ✅ **Module Management**: Load, execute, and unload WASM modules dynamically
- ✅ **String Support**: Automatic string detection for functions following naming conventions

## Quick Start

```rust
use hanzo_wasm_runtime::{WasmRuntime, WasmConfig};
use serde_json::json;

// Create runtime
let config = WasmConfig::default();
let runtime = WasmRuntime::new(config)?;

// Load a WASM module
let wasm_bytes = wat::parse_str(r#"
    (module
        (func $add (export "add") (param i32 i32) (result i32)
            local.get 0
            local.get 1
            i32.add
        )
    )
"#)?;

runtime.load_module("math".to_string(), wasm_bytes).await?;

// Execute a function
let result = runtime.execute("math", "add", json!([5, 3])).await?;
assert_eq!(result, json!(8));
```

## Configuration

```rust
let config = WasmConfig {
    max_memory_bytes: 256 * 1024 * 1024,  // 256MB max memory
    max_execution_time: Duration::from_secs(30),  // 30s timeout
    enable_wasi: true,  // Enable WASI support
    fuel_limit: Some(1_000_000_000),  // Optional fuel metering
};
```

## Parameter Passing

The runtime supports multiple ways to pass parameters:

### Array Parameters
```rust
runtime.execute("module", "func", json!([1, 2, 3])).await?
```

### Object Parameters
```rust
runtime.execute("module", "func", json!({"a": 10, "b": 20})).await?
```

### Single Value
```rust
runtime.execute("module", "func", json!(42)).await?
```

## String Handling

Functions that return string pointers are automatically detected based on naming conventions:
- Functions named `hello`
- Functions ending with `_str`
- Functions starting with `get_string`

Example:
```wat
(module
    (memory (export "memory") 1)
    (data (i32.const 0) "Hello, World!")

    (func $hello (export "hello") (result i32)
        i32.const 0  ;; Return pointer to string
    )
)
```

## Host Functions

The runtime provides several host functions that WASM modules can import:

### Logging
```wat
(import "env" "log" (func $log (param i32 i32)))
```

### JSON Operations
```wat
(import "env" "json_parse" (func $json_parse (param i32 i32) (result i32)))
(import "env" "json_stringify" (func $json_stringify (param i32) (result i32)))
```

### HTTP Requests (Mock)
```wat
(import "env" "http_request" (func $http_request (param i32 i32 i32) (result i32)))
```

### Memory Management
```wat
(import "env" "alloc" (func $alloc (param i32) (result i32)))
(import "env" "free" (func $free (param i32)))
```

## Resource Limits

The runtime enforces several resource limits for safe execution:

1. **Memory Limits**: Configurable maximum memory allocation
2. **Execution Timeout**: Prevents infinite loops
3. **Fuel Metering**: Optional instruction counting for deterministic limits

## Module Management

```rust
// Load a module
runtime.load_module("name".to_string(), wasm_bytes).await?;

// List loaded modules
let modules = runtime.list_modules().await;

// Unload a module
runtime.unload_module("name").await?;

// Clear all modules
runtime.clear_modules().await;
```

## Temporary Execution

Execute WASM bytecode without permanently loading a module:

```rust
let result = runtime.execute_bytes(wasm_bytes, "function_name", params).await?;
```

## Examples

See the `examples/` directory for complete working examples:
- `basic_usage.rs` - Comprehensive example showing various features

Run examples with:
```bash
cargo run --example basic_usage
```

## Testing

The runtime includes comprehensive tests covering:
- Basic arithmetic operations
- String handling
- Float operations
- Multiple return values
- Host function calls
- Resource limits (fuel and timeout)
- Module management

Run tests with:
```bash
cargo test -p hanzo-wasm-runtime
```

## Implementation Details

The runtime is built on top of [Wasmtime](https://wasmtime.dev/), a fast and secure WebAssembly runtime. Key implementation features:

- **Async Execution**: All operations are async-first for non-blocking execution
- **Thread Safety**: Runtime can be shared across threads using `Arc`
- **Error Handling**: Comprehensive error reporting with context
- **Type Safety**: Strong typing for parameters and results using `serde_json::Value`

## Security Considerations

1. **Sandboxing**: WASM modules run in a sandboxed environment with no direct system access
2. **Resource Limits**: Prevent resource exhaustion attacks
3. **Memory Safety**: Controlled memory allocation and bounds checking
4. **Deterministic Execution**: Optional fuel metering ensures predictable execution

## Future Enhancements

- [ ] Full WASI support for system interface
- [ ] Async host functions with proper async/await support
- [ ] Real HTTP request implementation
- [ ] Enhanced debugging and profiling tools
- [ ] WebAssembly component model support
- [ ] Custom import/export validation