//! Example of using the WASM runtime to execute WebAssembly modules

use hanzo_wasm_runtime::{WasmRuntime, WasmConfig};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    println!("=== Hanzo WASM Runtime Example ===\n");

    // Create runtime with default configuration
    let config = WasmConfig::default();
    let runtime = WasmRuntime::new(config)?;

    // Example 1: Simple arithmetic function
    println!("1. Loading arithmetic module...");
    let arithmetic_wat = r#"
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

            (func $factorial (export "factorial") (param i32) (result i32)
                (local $result i32)
                (local $counter i32)

                i32.const 1
                local.set $result

                local.get 0
                local.set $counter

                (loop $calculate
                    local.get $counter
                    i32.const 1
                    i32.le_s
                    (if
                        (then)
                        (else
                            local.get $result
                            local.get $counter
                            i32.mul
                            local.set $result

                            local.get $counter
                            i32.const 1
                            i32.sub
                            local.set $counter

                            br $calculate
                        )
                    )
                )

                local.get $result
            )
        )
    "#;

    let wasm_bytes = wat::parse_str(arithmetic_wat)?;
    let info = runtime.load_module("arithmetic".to_string(), wasm_bytes).await?;
    println!("  Loaded module '{}' with exports: {:?}", info.name, info.exports);

    // Test arithmetic functions
    println!("\n2. Testing arithmetic functions:");

    let result = runtime.execute("arithmetic", "add", json!([10, 32])).await?;
    println!("  add(10, 32) = {}", result);

    let result = runtime.execute("arithmetic", "multiply", json!([7, 6])).await?;
    println!("  multiply(7, 6) = {}", result);

    let result = runtime.execute("arithmetic", "factorial", json!([5])).await?;
    println!("  factorial(5) = {}", result);

    // Example 2: String manipulation (using memory)
    println!("\n3. Loading string module...");
    let string_wat = r#"
        (module
            (memory (export "memory") 1)

            ;; Store some strings in memory
            (data (i32.const 0) "Hello, WASM!")
            (data (i32.const 16) "Hanzo Runtime")
            (data (i32.const 32) "WebAssembly rocks!")

            (func $hello (export "hello") (result i32)
                ;; Return pointer to "Hello, WASM!"
                i32.const 0
            )

            (func $get_string_2 (export "get_string_2") (result i32)
                ;; Return pointer to "Hanzo Runtime"
                i32.const 16
            )

            (func $get_string_3 (export "get_string_3") (result i32)
                ;; Return pointer to "WebAssembly rocks!"
                i32.const 32
            )
        )
    "#;

    let wasm_bytes = wat::parse_str(string_wat)?;
    runtime.load_module("strings".to_string(), wasm_bytes).await?;

    println!("\n4. Testing string functions:");

    let result = runtime.execute("strings", "hello", json!(null)).await?;
    println!("  hello() = {:?}", result);

    let result = runtime.execute("strings", "get_string_2", json!(null)).await?;
    println!("  get_string_2() = {:?}", result);

    let result = runtime.execute("strings", "get_string_3", json!(null)).await?;
    println!("  get_string_3() = {:?}", result);

    // Example 3: Float operations
    println!("\n5. Loading float module...");
    let float_wat = r#"
        (module
            (func $pythagoras (export "pythagoras") (param f32 f32) (result f32)
                ;; Calculate sqrt(a² + b²)
                local.get 0
                local.get 0
                f32.mul

                local.get 1
                local.get 1
                f32.mul

                f32.add
                f32.sqrt
            )

            (func $circle_area (export "circle_area") (param f32) (result f32)
                ;; Calculate π * r²
                f32.const 3.14159265
                local.get 0
                local.get 0
                f32.mul
                f32.mul
            )
        )
    "#;

    let wasm_bytes = wat::parse_str(float_wat)?;
    runtime.load_module("geometry".to_string(), wasm_bytes).await?;

    println!("\n6. Testing float functions:");

    let result = runtime.execute("geometry", "pythagoras", json!([3.0, 4.0])).await?;
    println!("  pythagoras(3, 4) = {}", result);

    let result = runtime.execute("geometry", "circle_area", json!([5.0])).await?;
    println!("  circle_area(5) = {}", result);

    // Example 4: Using execute_bytes for temporary execution
    println!("\n7. Testing execute_bytes (temporary module):");
    let temp_wat = r#"
        (module
            (func $double (export "double") (param i32) (result i32)
                local.get 0
                i32.const 2
                i32.mul
            )
        )
    "#;

    let wasm_bytes = wat::parse_str(temp_wat)?;
    let result = runtime.execute_bytes(wasm_bytes, "double", json!([21])).await?;
    println!("  double(21) = {}", result);

    // Show loaded modules
    println!("\n8. Currently loaded modules:");
    let modules = runtime.list_modules().await;
    for module in modules {
        println!("  - {}", module);
    }

    println!("\n=== Example completed successfully! ===");

    Ok(())
}