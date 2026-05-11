// Hanzo node machine subsystem: thin lifetime owner around a hanzo_machine
// backend so a single hanzod process can manage host VMs without a sidecar
// daemon. Falls back to a no-op shim when libluxmachine is not installed so
// callers can hold a Subsystem unconditionally.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use hanzo_machine::{Error, Info, MachineBackend, Result, Spec};
use once_cell::sync::Lazy;

/// Default state directory: `$HOME/.hanzo/machine` (parallel to keys, db, etc).
fn default_state_dir() -> PathBuf {
    // WHY: align with `~/.hanzod/keys` convention used by cli/keys.rs but keep
    // VM state in a sibling dir so it can be wiped independently.
    let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(".hanzo").join("machine")
}

/// Per-process machine subsystem. One instance per hanzod, lives for the
/// lifetime of the node and is shut down on SIGTERM/Ctrl-C alongside the API
/// server and the libp2p node.
///
/// `MachineBackend` already has Send + Sync as supertrait bounds, so the bare
/// trait object is itself Send + Sync — no extra marker needed here.
pub struct Subsystem {
    backend: Box<dyn MachineBackend>,
    state_dir: PathBuf,
    installed: bool,
}

impl std::fmt::Debug for Subsystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subsystem")
            .field("state_dir", &self.state_dir)
            .field("installed", &self.installed)
            .finish()
    }
}

impl Subsystem {
    /// Open the preferred backend rooted at `state_dir`. On
    /// `Error::NotInstalled` returns a no-op shim that surfaces the same error
    /// from every call instead of failing the whole node.
    pub fn init(state_dir: &Path) -> Self {
        if let Err(e) = std::fs::create_dir_all(state_dir) {
            log::warn!(
                "machine: failed to create state dir {:?}: {} (subsystem disabled)",
                state_dir,
                e
            );
            return Self::shim(state_dir.to_path_buf());
        }
        match hanzo_machine::open_backend(state_dir) {
            Ok(backend) => {
                log::info!(
                    "machine: subsystem online (state={:?}, native={})",
                    state_dir,
                    hanzo_machine::prefer_native()
                );
                Self {
                    backend,
                    state_dir: state_dir.to_path_buf(),
                    installed: true,
                }
            }
            Err(Error::NotInstalled) => {
                log::info!(
                    "machine: subsystem disabled — libluxmachine not present, install at /usr/local or set LUXMACHINE_DIR"
                );
                Self::shim(state_dir.to_path_buf())
            }
            Err(e) => {
                log::warn!(
                    "machine: backend open failed: {} (subsystem disabled)",
                    e
                );
                Self::shim(state_dir.to_path_buf())
            }
        }
    }

    /// Convenience: init the subsystem at the default state dir.
    pub fn init_default() -> Self {
        Self::init(&default_state_dir())
    }

    fn shim(state_dir: PathBuf) -> Self {
        Self {
            backend: Box::new(NotInstalledShim),
            state_dir,
            installed: false,
        }
    }

    pub fn is_installed(&self) -> bool {
        self.installed
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub fn backend(&self) -> &dyn MachineBackend {
        self.backend.as_ref()
    }

    /// Drop the backend handle. Idempotent and infallible. Async to match the
    /// other shutdown points and leave room for future quiesce calls into the
    /// C++ lib.
    pub async fn shutdown(self) {
        log::debug!("machine: subsystem shutdown (state={:?})", self.state_dir);
        drop(self);
    }
}

/// Backend used when libluxmachine is not installed. Every call returns
/// `Error::NotInstalled` so callers can handle the missing-lib case uniformly.
#[derive(Debug)]
struct NotInstalledShim;

impl MachineBackend for NotInstalledShim {
    fn create(&self, _spec: &Spec) -> Result<Info> {
        Err(Error::NotInstalled)
    }
    fn start(&self, _name: &str) -> Result<()> {
        Err(Error::NotInstalled)
    }
    fn stop(&self, _name: &str) -> Result<()> {
        Err(Error::NotInstalled)
    }
    fn delete(&self, _name: &str) -> Result<()> {
        Err(Error::NotInstalled)
    }
    fn get(&self, _name: &str) -> Result<Info> {
        Err(Error::NotInstalled)
    }
    fn list(&self) -> Result<Vec<Info>> {
        Err(Error::NotInstalled)
    }
}

/// Library version string from libluxmachine, or `"not-installed"` shim
/// constant when the C++ lib isn't linked.
pub fn version() -> &'static str {
    hanzo_machine::Manager::version()
}

// WHY: a single subsystem per process; runner stashes it here at startup so a
// future signal handler can call shutdown_global without threading the handle
// through every task.
static GLOBAL: Lazy<Mutex<Option<Subsystem>>> = Lazy::new(|| Mutex::new(None));

pub fn register_global(sub: Subsystem) {
    let mut g = GLOBAL.lock().expect("machine global poisoned");
    *g = Some(sub);
}

pub async fn shutdown_global() {
    let taken = GLOBAL.lock().expect("machine global poisoned").take();
    if let Some(sub) = taken {
        sub.shutdown().await;
    }
}

// ---------- CLI surface ----------

/// Parse `argv[1..]` for the `machine` subcommand and dispatch. Returns
/// `Some(exit_code)` if a machine command ran; `None` if the user did not
/// request the machine surface and the node should boot normally.
pub fn try_handle_cli(argv: &[String]) -> Option<i32> {
    if argv.first().map(String::as_str) != Some("machine") {
        return None;
    }
    let rest: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
    Some(dispatch(&rest))
}

fn dispatch(args: &[&str]) -> i32 {
    let sub = match args.first() {
        Some(s) => *s,
        None => {
            eprintln!("{}", machine_help());
            return 2;
        }
    };
    let rest = &args[1..];
    let result: Result<()> = match sub {
        "version" => {
            println!("{}", version());
            return 0;
        }
        "list" => cmd_list(),
        "create" => cmd_create(rest),
        "start" => cmd_one("start", rest),
        "stop" => cmd_one("stop", rest),
        "delete" => cmd_one("delete", rest),
        "inspect" => cmd_inspect(rest),
        "help" | "--help" | "-h" => {
            println!("{}", machine_help());
            return 0;
        }
        other => {
            eprintln!("hanzod machine: unknown subcommand `{other}`\n\n{}", machine_help());
            return 2;
        }
    };
    match result {
        Ok(()) => 0,
        Err(Error::NotInstalled) => {
            eprintln!(
                "hanzod machine: libluxmachine not installed; build with LUXMACHINE_DIR set or install via `cmake --install` from luxcpp/machine"
            );
            1
        }
        Err(e) => {
            eprintln!("hanzod machine: {e}");
            1
        }
    }
}

fn open_for_cli() -> Subsystem {
    Subsystem::init_default()
}

fn cmd_list() -> Result<()> {
    let sub = open_for_cli();
    let infos = sub.backend().list()?;
    print_table(&infos);
    Ok(())
}

fn cmd_create(args: &[&str]) -> Result<()> {
    let mut name: Option<String> = None;
    let mut distro = "ubuntu".to_string();
    let mut cpus: u32 = 2;
    let mut memory_mb: u64 = 2048;
    let mut disk_gb: u64 = 20;
    let mut rosetta = false;
    for a in args {
        if let Some(v) = a.strip_prefix("--distro=") {
            distro = v.to_string();
        } else if let Some(v) = a.strip_prefix("--cpus=") {
            cpus = v.parse().map_err(|_| Error::Invalid)?;
        } else if let Some(v) = a.strip_prefix("--memory=") {
            memory_mb = v.parse().map_err(|_| Error::Invalid)?;
        } else if let Some(v) = a.strip_prefix("--disk=") {
            disk_gb = v.parse().map_err(|_| Error::Invalid)?;
        } else if *a == "--rosetta" {
            rosetta = true;
        } else if a.starts_with("--") {
            return Err(Error::Invalid);
        } else if name.is_none() {
            name = Some((*a).to_string());
        } else {
            return Err(Error::Invalid);
        }
    }
    let name = name.ok_or(Error::Invalid)?;
    let spec = Spec { name, distro, cpus, memory_mb, disk_gb, rosetta };
    let sub = open_for_cli();
    let info = sub.backend().create(&spec)?;
    print_table(std::slice::from_ref(&info));
    Ok(())
}

fn cmd_one(verb: &str, args: &[&str]) -> Result<()> {
    let name = args.first().ok_or(Error::Invalid)?;
    let sub = open_for_cli();
    match verb {
        "start" => sub.backend().start(name)?,
        "stop" => sub.backend().stop(name)?,
        "delete" => sub.backend().delete(name)?,
        _ => return Err(Error::Invalid),
    }
    println!("{verb}: {name}");
    Ok(())
}

fn cmd_inspect(args: &[&str]) -> Result<()> {
    let name = args.first().ok_or(Error::Invalid)?;
    let sub = open_for_cli();
    let info = sub.backend().get(name)?;
    let json = serde_json::to_string_pretty(&info).unwrap_or_else(|_| format!("{info:?}"));
    println!("{json}");
    Ok(())
}

fn machine_help() -> &'static str {
    "hanzod machine — host VM lifecycle (libluxmachine)\n\
     \n\
     USAGE:\n\
       hanzod machine <command> [args]\n\
     \n\
     COMMANDS:\n\
       list                                 list all machines\n\
       create <name> [--distro=ubuntu] [--cpus=N] [--memory=MB] [--disk=GB] [--rosetta]\n\
       start <name>                         start a machine\n\
       stop <name>                          stop a machine\n\
       delete <name>                        delete a machine\n\
       inspect <name>                       print machine info as JSON\n\
       version                              print libluxmachine version\n\
       help                                 show this help"
}

fn print_table(infos: &[Info]) {
    if infos.is_empty() {
        println!("(no machines)");
        return;
    }
    // WHY: hand-rolled formatter avoids pulling tabwriter/comfy_table just for one CLI surface.
    let headers = ["NAME", "DISTRO", "STATE", "CPUS", "MEM(MB)", "DISK(GB)", "IP"];
    let mut widths: [usize; 7] = headers.map(str::len);
    let rows: Vec<[String; 7]> = infos
        .iter()
        .map(|i| {
            [
                i.name.clone(),
                i.distro.clone(),
                format!("{:?}", i.state).to_lowercase(),
                i.cpus.to_string(),
                i.memory_mb.to_string(),
                i.disk_gb.to_string(),
                i.ip.clone().unwrap_or_else(|| "-".into()),
            ]
        })
        .collect();
    for row in &rows {
        for (idx, cell) in row.iter().enumerate() {
            if cell.len() > widths[idx] {
                widths[idx] = cell.len();
            }
        }
    }
    let print_row = |cells: &[&str; 7]| {
        let mut s = String::new();
        for (idx, cell) in cells.iter().enumerate() {
            s.push_str(&format!("{:<width$}", cell, width = widths[idx]));
            if idx + 1 < cells.len() {
                s.push_str("  ");
            }
        }
        println!("{s}");
    };
    print_row(&headers);
    for row in &rows {
        let view: [&str; 7] = [
            row[0].as_str(),
            row[1].as_str(),
            row[2].as_str(),
            row[3].as_str(),
            row[4].as_str(),
            row[5].as_str(),
            row[6].as_str(),
        ];
        print_row(&view);
    }
}
