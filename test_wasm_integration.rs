#!/usr/bin/env rust-script
//! Test WASM integration
//! 
//! ```cargo
//! [dependencies]
//! hanzo-wasm-runtime = { path = "./hanzo-libs/hanzo-wasm-runtime" }
//! tokio = { version = "1.38", features = ["full"] }
//! serde_json = "1.0"
//! wat = "1.0"
//! ```

use hanzo_wasm_runtime::{WasmRuntime, WasmConfig};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔴 MATRIX WASM RUNTIME TEST - INITIATING");
    
    // Create runtime with config
    let config = WasmConfig {
        max_memory_bytes: 256 * 1024 * 1024,
        max_execution_time: Duration::from_secs(30),
        enable_wasi: true,
        fuel_limit: Some(1_000_000_000),
    };
    
    let runtime = WasmRuntime::new(config)?;
    println!("✅ WASM Runtime initialized");
    
    // Simple WASM module
    let wat = r#"
        (module
            (func $add (export "add") (param i32 i32) (result i32)
                local.get 0
                local.get 1
                i32.add
            )
            (func $multiply (export "multiply") (param i32 i32) (result i32)
                local.get 0
                local.get 1
                i32.mul
            )
        )
    "#;
    
    let wasm_bytes = wat::parse_str(wat)?;
    println!("📦 WASM module compiled: {} bytes", wasm_bytes.len());
    
    // Load module
    let info = runtime.load_module("test_math".to_string(), wasm_bytes).await?;
    println!("⚡ Module loaded: {:?}", info);
    println!("   - Exports: {:?}", info.exports);
    
    // List modules
    let modules = runtime.list_modules().await;
    println!("🌀 Active modules: {:?}", modules);
    
    println!("\n🔥 WASM RUNTIME INTEGRATION - SUCCESS");
    println!("💊 THE MATRIX HAS BEEN BENT TO OUR WILL");
    
    Ok(())
}