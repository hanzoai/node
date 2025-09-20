(module
  ;; Import memory
  (import "env" "memory" (memory 1))

  ;; Import host functions
  (import "env" "log" (func $log (param i32 i32)))
  (import "env" "json_parse" (func $json_parse (param i32 i32) (result i32)))
  (import "env" "json_stringify" (func $json_stringify (param i32) (result i32)))

  ;; Export an add function for testing
  (func $add (export "add") (param $a i32) (param $b i32) (result i32)
    local.get $a
    local.get $b
    i32.add
  )

  ;; Export a process_json function that takes JSON input
  (func $process_json (export "process_json") (param $ptr i32) (param $len i32) (result i32)
    ;; Parse the JSON input
    local.get $ptr
    local.get $len
    call $json_parse
  )

  ;; Export a hello function that returns a string pointer
  (func $hello (export "hello") (result i32)
    ;; Return pointer to "Hello, WASM!" string at offset 0
    i32.const 0
  )

  ;; Initialize memory with test data
  (data (i32.const 0) "Hello, WASM!")
)