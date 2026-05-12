# hanzo-machine

Rust bindings for the canonical luxmachine C ABI (host VM lifecycle on macOS
via vfkit). Linked into hanzod so the agent node ships Docker-Desktop-style VM
management without re-implementing it.

One canonical impl: `~/work/luxcpp/machine`. Two language wrappers:
- Rust: this crate.
- Go: cgo binding in `~/work/zoo/node`.

## Build

```sh
# Build & install the C++ library (one time, system-wide):
cmake -S ~/work/luxcpp/machine -B ~/work/luxcpp/machine/build
cmake --build ~/work/luxcpp/machine/build
sudo cmake --install ~/work/luxcpp/machine/build

# Then:
cargo build -p hanzo-machine
```

If you can't install system-wide, point `LUXMACHINE_DIR` at the install root:

```sh
LUXMACHINE_DIR=$HOME/work/luxcpp/machine/build cargo build -p hanzo-machine
```

The build script searches in this order:

1. `pkg-config --cflags --libs luxmachine`
2. `$LUXMACHINE_DIR/{include,lib}`
3. `/usr/local/{include,lib}`

If none have the header, build.rs writes stub bindings and emits a
`cargo:warning`. The crate still compiles; `Manager::open` returns
`Error::NotInstalled` at runtime and callers fall back to the sidecar
(`SidecarClient` over `~/.hanzo/run/machined.sock`).

## Features

- (default) link `libluxmachine.dylib` / `.so` dynamically.
- `static` — link `libluxmachine.a` instead.

Both pull in the C++ runtime (`libc++` on macOS, `libstdc++` on Linux).

## Usage

```rust
use hanzo_machine::{open_backend, Spec};

let backend = open_backend("~/.hanzo/state/machines")?;
backend.create(&Spec {
    name: "dev".into(),
    distro: "ubuntu".into(),
    cpus: 2,
    memory_mb: 2048,
    disk_gb: 20,
    rosetta: false,
})?;
backend.start("dev")?;
```

## License

MIT.
