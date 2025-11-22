//! WASM Runtime for Hanzo Node
//!
//! Provides sandboxed execution of WebAssembly modules as tools,
//! enabling language-agnostic compute with deterministic execution.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, error};
use wasmtime::*;
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiView};

use hanzo_tools::tools::error::ToolError;

/// Configuration for WASM runtime
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmConfig {
    /// Maximum memory in bytes (default: 256MB)
    pub max_memory_bytes: u64,
    /// Maximum execution time
    pub max_execution_time: Duration,
    /// Enable WASI (WebAssembly System Interface)
    pub enable_wasi: bool,
    /// Fuel limit for deterministic execution (if Some)
    pub fuel_limit: Option<u64>,
}

impl Default for WasmConfig {
    fn default() -> Self {
        Self {
            max_memory_bytes: 256 * 1024 * 1024, // 256MB
            max_execution_time: Duration::from_secs(30),
            enable_wasi: true,
            fuel_limit: Some(1_000_000_000), // 1 billion units
        }
    }
}

/// WASM module metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmModuleInfo {
    pub name: String,
    pub version: String,
    pub hash: String,
    pub exports: Vec<String>,
    pub memory_pages: u32,
}

/// Store data for WASM instances with shared memory for host-guest communication
struct StoreData {
    wasi_ctx: WasiCtx,
    table: ResourceTable,
    limits: StoreLimitsBuilder,
    /// Shared data for host-guest communication
    host_data: Arc<RwLock<HostData>>,
}

impl WasiView for StoreData {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi_ctx
    }
}

/// Host data shared between host and WASM guest
#[derive(Debug, Default)]
struct HostData {
    /// Log messages from WASM
    logs: Vec<String>,
    /// JSON values for parsing/stringifying
    json_values: HashMap<i32, Value>,
    /// String buffers for memory transfer
    string_buffers: HashMap<i32, String>,
    /// Next available buffer ID
    next_buffer_id: i32,
}

/// WASM Runtime for executing WebAssembly modules
pub struct WasmRuntime {
    engine: Engine,
    config: WasmConfig,
    modules: Arc<Mutex<HashMap<String, Module>>>,
}

impl WasmRuntime {
    /// Create a new WASM runtime with the given configuration
    pub fn new(config: WasmConfig) -> Result<Self> {
        let mut engine_config = Config::new();

        // Enable optimizations
        engine_config.cranelift_opt_level(OptLevel::Speed);
        engine_config.wasm_bulk_memory(true);
        engine_config.wasm_multi_value(true);
        engine_config.wasm_reference_types(true);

        // Set resource limits
        // Note: In wasmtime 23.0, allocation_strategy takes no arguments
        engine_config.allocation_strategy(InstanceAllocationStrategy::OnDemand);

        // Enable fuel metering if configured
        if config.fuel_limit.is_some() {
            engine_config.consume_fuel(true);
        }

        let engine = Engine::new(&engine_config)?;

        Ok(Self {
            engine,
            config,
            modules: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Load a WASM module from bytes
    pub async fn load_module(&self, name: String, wasm_bytes: Vec<u8>) -> Result<WasmModuleInfo> {
        info!("Loading WASM module: {}", name);

        // Compile the module
        let module = Module::new(&self.engine, &wasm_bytes)?;

        // Extract module information
        let exports: Vec<String> = module
            .exports()
            .filter_map(|exp| {
                if let ExternType::Func(_) = exp.ty() {
                    Some(exp.name().to_string())
                } else {
                    None
                }
            })
            .collect();

        // Calculate hash
        let hash = format!("{:x}", md5::compute(&wasm_bytes));

        let info = WasmModuleInfo {
            name: name.clone(),
            version: "1.0.0".to_string(), // TODO: Extract from custom section
            hash,
            exports,
            memory_pages: 0, // TODO: Extract from module
        };

        // Store the module
        let mut modules = self.modules.lock().await;
        modules.insert(name, module);

        Ok(info)
    }

    /// Execute a function from a loaded WASM module
    ///
    /// Special conventions:
    /// - Functions ending with "_str" or named "hello" are expected to return string pointers
    pub async fn execute(
        &self,
        module_name: &str,
        function_name: &str,
        params: Value,
    ) -> Result<Value, ToolError> {
        info!("Executing WASM function: {}::{}", module_name, function_name);

        // Get the module
        let modules = self.modules.lock().await;
        let module = modules
            .get(module_name)
            .ok_or_else(|| ToolError::ExecutionError(format!("Module {} not found", module_name)))?
            .clone();
        drop(modules); // Release lock early

        // Create host data for this execution
        let host_data = Arc::new(RwLock::new(HostData::default()));

        // Create store with limits
        let mut store = Store::new(
            &self.engine,
            StoreData {
                wasi_ctx: WasiCtxBuilder::new().build(),
                table: ResourceTable::new(),
                limits: StoreLimitsBuilder::new(),
                host_data: host_data.clone(),
            },
        );

        // Set fuel if configured
        if let Some(fuel) = self.config.fuel_limit {
            store.set_fuel(fuel).map_err(|e| {
                ToolError::ExecutionError(format!("Failed to set fuel: {}", e))
            })?;
        }

        // Create linker and add host functions
        let mut linker: Linker<StoreData> = Linker::new(&self.engine);

        // Define memory and export it for the guest
        let memory_ty = MemoryType::new(1, Some(256)); // 1 page min, 256 pages max (16MB)
        let memory = Memory::new(&mut store, memory_ty).map_err(|e| {
            ToolError::ExecutionError(format!("Failed to create memory: {}", e))
        })?;
        linker.define(&store, "env", "memory", memory).map_err(|e| {
            ToolError::ExecutionError(format!("Failed to define memory: {}", e))
        })?;

        // Add custom host functions
        self.add_host_functions(&mut linker)
            .map_err(|e| ToolError::ExecutionError(format!("Failed to add host functions: {}", e)))?;

        // Instantiate the module
        let instance = linker.instantiate(&mut store, &module).map_err(|e| {
            ToolError::ExecutionError(format!("Failed to instantiate module: {}", e))
        })?;

        // Get the exported memory (if any) for reading/writing data
        let memory = instance
            .get_memory(&mut store, "memory")
            .or_else(|| linker.get(&mut store, "env", "memory")?.into_memory())
            .ok_or_else(|| ToolError::ExecutionError("No memory found".to_string()))?;

        // Get the function
        let func = instance
            .get_func(&mut store, function_name)
            .ok_or_else(|| {
                ToolError::ExecutionError(format!("Function {} not found", function_name))
            })?;

        // Check function signature and prepare arguments
        let func_ty = func.ty(&store);
        let param_types: Vec<ValType> = func_ty.params().collect();
        let result_types: Vec<ValType> = func_ty.results().collect();

        debug!("Function signature: {:?} -> {:?}", param_types, result_types);

        // Prepare parameters based on function signature
        let wasm_params = self.prepare_wasm_params(&mut store, &memory, &params, &param_types)
            .map_err(|e| ToolError::ExecutionError(format!("Failed to prepare params: {}", e)))?;

        debug!("Prepared {} WASM params: {:?}", wasm_params.len(), wasm_params);

        // Check if this function returns a string pointer
        let returns_string = function_name.ends_with("_str") ||
                            function_name == "hello" ||
                            function_name.starts_with("get_string");

        // Call the function with timeout
        let result = tokio::time::timeout(
            self.config.max_execution_time,
            tokio::task::spawn_blocking(move || {
                // Call the WASM function
                let mut results = vec![Val::I32(0); result_types.len()];
                func.call(&mut store, &wasm_params, &mut results)
                    .map_err(|e| ToolError::ExecutionError(format!("Function call failed: {}", e)))?;

                // Process the results based on return type
                let result = if results.is_empty() {
                    Value::Null
                } else if results.len() == 1 {
                    // Single return value
                    match &results[0] {
                        Val::I32(n) => {
                            // Check if this is a string pointer based on function name convention
                            if returns_string && *n >= 0 {
                                // Try to read string from memory
                                if let Ok(s) = Self::read_string_from_memory(&store, &memory, *n as usize) {
                                    Value::String(s)
                                } else {
                                    // Fallback to number if can't read string
                                    Value::Number(serde_json::Number::from(*n))
                                }
                            } else {
                                Value::Number(serde_json::Number::from(*n))
                            }
                        },
                        Val::I64(n) => Value::Number(serde_json::Number::from(*n)),
                        Val::F32(n) => {
                            // Convert F32 bits to f32 then to f64 for JSON
                            let f = f32::from_bits(*n);
                            if let Some(num) = serde_json::Number::from_f64(f64::from(f)) {
                                Value::Number(num)
                            } else {
                                Value::Null
                            }
                        },
                        Val::F64(n) => {
                            // Convert F64 bits to f64 for JSON
                            let f = f64::from_bits(*n);
                            if let Some(num) = serde_json::Number::from_f64(f) {
                                Value::Number(num)
                            } else {
                                Value::Null
                            }
                        },
                        _ => Value::Null,
                    }
                } else {
                    // Multiple return values - return as array
                    let values: Vec<Value> = results.iter().map(|val| match val {
                        Val::I32(n) => Value::Number(serde_json::Number::from(*n)),
                        Val::I64(n) => Value::Number(serde_json::Number::from(*n)),
                        _ => Value::Null,
                    }).collect();
                    Value::Array(values)
                };

                Ok(result)
            }),
        )
        .await
        .map_err(|_| ToolError::ExecutionError("Execution timeout".to_string()))?
        .map_err(|e| ToolError::ExecutionError(format!("Execution failed: {}", e)))?;

        // Add any logs to the result
        let logs = host_data.read().await.logs.clone();
        if !logs.is_empty() {
            debug!("WASM execution logs: {:?}", logs);
        }

        result
    }

    /// Prepare WASM parameters based on function signature
    fn prepare_wasm_params(
        &self,
        store: &mut Store<StoreData>,
        memory: &Memory,
        params: &Value,
        param_types: &[ValType],
    ) -> Result<Vec<Val>> {
        let mut wasm_params = Vec::new();

        if param_types.is_empty() {
            // No parameters needed
        } else {
            // Handle parameter patterns based on the JSON input type
            match params {
                Value::Array(arr) => {
                    // Array input - use indexed access
                    for (i, param_type) in param_types.iter().enumerate() {
                        let val = if i < arr.len() {
                            match param_type {
                                ValType::I32 => {
                                    Val::I32(arr[i].as_i64().unwrap_or(0) as i32)
                                },
                                ValType::I64 => {
                                    Val::I64(arr[i].as_i64().unwrap_or(0))
                                },
                                ValType::F32 => {
                                    Val::F32((arr[i].as_f64().unwrap_or(0.0) as f32).to_bits())
                                },
                                ValType::F64 => {
                                    Val::F64(arr[i].as_f64().unwrap_or(0.0).to_bits())
                                },
                                _ => Val::I32(0),
                            }
                        } else {
                            // Not enough parameters provided
                            match param_type {
                                ValType::I32 => Val::I32(0),
                                ValType::I64 => Val::I64(0),
                                ValType::F32 => Val::F32(0.0_f32.to_bits()),
                                ValType::F64 => Val::F64(0.0_f64.to_bits()),
                                _ => Val::I32(0),
                            }
                        };
                        wasm_params.push(val);
                    }
                },
                Value::Object(obj) => {
                    // Object input - try to get parameters by name (a, b, c, ...)
                    let param_names = ["a", "b", "c", "d", "e", "f", "g", "h"];
                    for (i, param_type) in param_types.iter().enumerate() {
                        let param_name = if i < param_names.len() { param_names[i] } else { "" };
                        let val = if let Some(value) = obj.get(param_name) {
                            match param_type {
                                ValType::I32 => {
                                    Val::I32(value.as_i64().unwrap_or(0) as i32)
                                },
                                ValType::I64 => {
                                    Val::I64(value.as_i64().unwrap_or(0))
                                },
                                ValType::F32 => {
                                    Val::F32((value.as_f64().unwrap_or(0.0) as f32).to_bits())
                                },
                                ValType::F64 => {
                                    Val::F64(value.as_f64().unwrap_or(0.0).to_bits())
                                },
                                _ => Val::I32(0),
                            }
                        } else {
                            // Parameter not found in object
                            match param_type {
                                ValType::I32 => Val::I32(0),
                                ValType::I64 => Val::I64(0),
                                ValType::F32 => Val::F32(0.0_f32.to_bits()),
                                ValType::F64 => Val::F64(0.0_f64.to_bits()),
                                _ => Val::I32(0),
                            }
                        };
                        wasm_params.push(val);
                    }
                },
                Value::Number(n) => {
                    // Single number - use for first parameter only
                    for (i, param_type) in param_types.iter().enumerate() {
                        let val = if i == 0 {
                            match param_type {
                                ValType::I32 => Val::I32(n.as_i64().unwrap_or(0) as i32),
                                ValType::I64 => Val::I64(n.as_i64().unwrap_or(0)),
                                ValType::F32 => Val::F32((n.as_f64().unwrap_or(0.0) as f32).to_bits()),
                                ValType::F64 => Val::F64(n.as_f64().unwrap_or(0.0).to_bits()),
                                _ => Val::I32(0),
                            }
                        } else {
                            // Use default for other parameters
                            match param_type {
                                ValType::I32 => Val::I32(0),
                                ValType::I64 => Val::I64(0),
                                ValType::F32 => Val::F32(0.0_f32.to_bits()),
                                ValType::F64 => Val::F64(0.0_f64.to_bits()),
                                _ => Val::I32(0),
                            }
                        };
                        wasm_params.push(val);
                    }
                },
                _ => {
                    // Other types - use defaults
                    for param_type in param_types.iter() {
                        let val = match param_type {
                            ValType::I32 => Val::I32(0),
                            ValType::I64 => Val::I64(0),
                            ValType::F32 => Val::F32(0.0_f32.to_bits()),
                            ValType::F64 => Val::F64(0.0_f64.to_bits()),
                            _ => Val::I32(0),
                        };
                        wasm_params.push(val);
                    }
                }
            }
        }

        Ok(wasm_params)
    }

    /// Read a null-terminated string from WASM memory
    fn read_string_from_memory(
        store: &Store<StoreData>,
        memory: &Memory,
        offset: usize,
    ) -> Result<String> {
        let data = memory.data(store);
        let mut end = offset;

        // Find null terminator or read up to 4KB
        while end < data.len() && end < offset + 4096 && data[end] != 0 {
            end += 1;
        }

        let bytes = &data[offset..end];
        String::from_utf8(bytes.to_vec()).map_err(|e| anyhow::anyhow!("Invalid UTF-8: {}", e))
    }

    /// Execute WASM bytes directly without loading
    pub async fn execute_bytes(
        &self,
        wasm_bytes: Vec<u8>,
        function_name: &str,
        params: Value,
    ) -> Result<Value, ToolError> {
        let module_name = format!("temp_{}", uuid::Uuid::new_v4());
        self.load_module(module_name.clone(), wasm_bytes).await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;
        let result = self.execute(&module_name, function_name, params).await;

        // Clean up temporary module
        let mut modules = self.modules.lock().await;
        modules.remove(&module_name);

        result
    }

    /// Add host functions that WASM modules can call
    fn add_host_functions(&self, linker: &mut Linker<StoreData>) -> Result<()> {
        // Add logging function
        linker.func_wrap(
            "env",
            "log",
            |mut caller: Caller<'_, StoreData>, ptr: i32, len: i32| {
                // Read string from WASM memory
                if let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) {
                    let data = memory.data(&caller);
                    if ptr >= 0 && len >= 0 {
                        let start = ptr as usize;
                        let end = (ptr + len) as usize;
                        if end <= data.len() {
                            if let Ok(msg) = String::from_utf8(data[start..end].to_vec()) {
                                info!("WASM log: {}", msg);
                                // Store in host data
                                if let Ok(mut host_data) = caller.data().host_data.try_write() {
                                    host_data.logs.push(msg);
                                }
                            }
                        }
                    }
                }
            },
        )?;

        // Add JSON parsing helper
        linker.func_wrap(
            "env",
            "json_parse",
            |mut caller: Caller<'_, StoreData>, ptr: i32, len: i32| -> i32 {
                // Read JSON string from WASM memory
                if let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) {
                    let data = memory.data(&caller);
                    if ptr >= 0 && len >= 0 {
                        let start = ptr as usize;
                        let end = (ptr + len) as usize;
                        if end <= data.len() {
                            if let Ok(json_str) = String::from_utf8(data[start..end].to_vec()) {
                                if let Ok(value) = serde_json::from_str::<Value>(&json_str) {
                                    // Store parsed value and return handle
                                    if let Ok(mut host_data) = caller.data().host_data.try_write() {
                                        let id = host_data.next_buffer_id;
                                        host_data.next_buffer_id += 1;
                                        host_data.json_values.insert(id, value);
                                        return id;
                                    }
                                }
                            }
                        }
                    }
                }
                -1 // Error
            },
        )?;

        // Add JSON stringify helper
        linker.func_wrap(
            "env",
            "json_stringify",
            |mut caller: Caller<'_, StoreData>, handle: i32| -> i32 {
                // Get JSON value from handle and convert to string first
                let json_str = {
                    if let Ok(host_data) = caller.data().host_data.try_read() {
                        if let Some(value) = host_data.json_values.get(&handle) {
                            serde_json::to_string(value).ok()
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };

                // Now write to memory without holding the host_data lock
                if let Some(json_str) = json_str {
                    if let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) {
                        let offset = 0x2000;
                        let bytes = json_str.as_bytes();
                        if memory.write(&mut caller, offset, bytes).is_ok() {
                            // Return offset where string was written
                            return offset as i32;
                        }
                    }
                }
                -1 // Error
            },
        )?;

        // Add HTTP request capability (simplified for now)
        linker.func_wrap(
            "env",
            "http_request",
            |mut caller: Caller<'_, StoreData>, method_ptr: i32, url_ptr: i32, body_ptr: i32| -> i32 {
                // This is a placeholder - real implementation would need async support
                // For now, just log the request
                if let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) {
                    let data = memory.data(&caller);

                    // Read method string
                    let method = Self::read_cstring_from_data(&data, method_ptr as usize).unwrap_or_default();
                    let url = Self::read_cstring_from_data(&data, url_ptr as usize).unwrap_or_default();
                    let body = if body_ptr > 0 {
                        Self::read_cstring_from_data(&data, body_ptr as usize).unwrap_or_default()
                    } else {
                        String::new()
                    };

                    info!("WASM HTTP request: {} {} body_len={}", method, url, body.len());

                    // Return a mock response handle
                    if let Ok(mut host_data) = caller.data().host_data.try_write() {
                        let id = host_data.next_buffer_id;
                        host_data.next_buffer_id += 1;
                        // Store mock response
                        host_data.json_values.insert(id, Value::Object(serde_json::Map::from_iter([
                            ("status".to_string(), Value::Number(200.into())),
                            ("body".to_string(), Value::String("Mock response".to_string())),
                        ])));
                        return id;
                    }
                }
                -1 // Error
            },
        )?;

        // Add memory allocation function for guest to request memory
        linker.func_wrap(
            "env",
            "alloc",
            |_caller: Caller<'_, StoreData>, size: i32| -> i32 {
                // Simple bump allocator starting at 0x3000
                static mut HEAP_PTR: i32 = 0x3000;
                unsafe {
                    let ptr = HEAP_PTR;
                    HEAP_PTR += size;
                    ptr
                }
            },
        )?;

        // Add memory free function (no-op for simplicity)
        linker.func_wrap(
            "env",
            "free",
            |_caller: Caller<'_, StoreData>, _ptr: i32| {
                // No-op for now
            },
        )?;

        Ok(())
    }

    /// Read a C-style null-terminated string from memory data
    fn read_cstring_from_data(data: &[u8], offset: usize) -> Option<String> {
        if offset >= data.len() {
            return None;
        }

        let mut end = offset;
        while end < data.len() && data[end] != 0 {
            end += 1;
        }

        String::from_utf8(data[offset..end].to_vec()).ok()
    }

    /// List all loaded modules
    pub async fn list_modules(&self) -> Vec<String> {
        let modules = self.modules.lock().await;
        modules.keys().cloned().collect()
    }

    /// Unload a module
    pub async fn unload_module(&self, name: &str) -> Result<()> {
        let mut modules = self.modules.lock().await;
        modules.remove(name);
        Ok(())
    }

    /// Clear all modules
    pub async fn clear_modules(&self) {
        let mut modules = self.modules.lock().await;
        modules.clear();
    }
}

/// WASM tool wrapper for integration with hanzo_node tool system
pub struct WasmTool {
    runtime: Arc<WasmRuntime>,
    module_name: String,
    function_name: String,
}

impl WasmTool {
    pub fn new(runtime: Arc<WasmRuntime>, module_name: String, function_name: String) -> Self {
        Self {
            runtime,
            module_name,
            function_name,
        }
    }

    pub async fn run(&self, params: Value) -> Result<Value, ToolError> {
        self.runtime.execute(&self.module_name, &self.function_name, params).await
    }
}

#[cfg(test)]
mod test_wasm;

