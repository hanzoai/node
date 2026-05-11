# node — AI Assistant Context

<h1 align="center">
  <img src="files/icon.png"/><br/>
  Hanzo Node
</h1>
<p align="center">Hanzo allows you to create AI agents without touching code. Define tasks, schedule actions, and let Hanzo write custom code for you. Native crypto support included.<br/><br/> There is a companion repo called Hanzo Apps which contains the frontend that encapsulates this project, you can find it <a href="https://github.com/hanzoai/hanzo-apps">here</a>.</p><br/>

## Machines

`machine/` is the macOS-native VM lifecycle daemon and CLI surface for running
Linux guests with docker / k3s for desktop development. It is Hanzo's
all-in-one Docker Desktop alternative: vfkit (Apple Virtualization.framework) for the VM,
gvproxy + mDNS for networking, virtio-vsock for the docker socket bridge, and
k3s as the optional in-guest Kubernetes runtime.

### Layout

```
machine/
  proto.go         # HTTP/wire contract (single source of truth)
  paths.go         # ~/.hanzo/{run,machines,cache} layout
  store.go         # spec.json/state.json persistence (atomic writes)
  image.go         # cloud-image fetch + SHA256 + on-disk cache
  net/             # gvproxy + mDNS + DNS resolver + port forwarder
  dockerbridge/    # host UNIX socket → guest docker over virtio-vsock
  k3s/             # k3s install / start / kubeconfig extraction
  e2e/             # opt-in (build tag `e2e`) full-VM tests
```

The Tauri desktop side (`hanzoai/desktop`, `apps/hanzo-desktop/src-tauri/src/machine/`)
is a thin client over the daemon's UNIX socket; the React UI lives in
`apps/hanzo-desktop/src/pages/machines/`.

### Daemon contract

Authoritative in `machine/proto.go`. Daemon listens on
`~/.hanzo/run/machined.sock` (override via `HANZO_MACHINED_SOCKET`):

```
GET    /v1/health                   200 {ok,version}
GET    /v1/machines                 200 [Machine,...]
POST   /v1/machines                 201 Machine          body: MachineSpec
GET    /v1/machines/{name}          200 Machine
DELETE /v1/machines/{name}          204
POST   /v1/machines/{name}/start    204
POST   /v1/machines/{name}/stop     204
POST   /v1/machines/{name}/exec     200 ndjson ExecChunk
GET    /v1/machines/{name}/events   SSE Event
```

Versioned at `machine.Version`. CLI, Tauri, and tests all dial the same socket
and parse the same types — no second source.

### Build & run

```
make machine        # build hanzod with the machine subcommand
make machine-deps   # brew install vfkit (idempotent)
make machine-test   # go test ./machine/...
make machine-clean  # rm ~/.hanzo/run/*.sock and ~/.hanzo/machines/* (prompts)
```

Run the daemon once vfkit is installed:

```
hanzod machined &                 # foreground daemon
hanzod machine ls                 # CLI client over the unix socket
```

### Testing

* `go test ./machine/...` runs unit tests (no VM, hermetic).
* `go test -tags=e2e ./machine/e2e/...` boots a real VM, dials the docker
  bridge, runs `docker run hello-world`, and tears down. Requires
  `make machine-deps` (vfkit) and is **not** run in CI by default — it costs
  ~90 s and needs Apple Silicon + Virtualization.framework entitlement.
