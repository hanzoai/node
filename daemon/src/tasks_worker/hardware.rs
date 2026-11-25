// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Hardware & host environment discovery for `hanzod`'s linked-machine worker.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GpuInventory {
    pub name: String,
    #[serde(rename = "memoryTotal", skip_serializing_if = "String::is_empty", default)]
    pub memory_total: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub arch: String,
    #[serde(default)]
    pub unified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareInventory {
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub cpus: usize,
    pub cpu_model: String,
    pub memory_bytes: u64,
    pub cuda: String,
    pub driver: String,
    pub rocm: String,
    pub gpus: Vec<GpuInventory>,
    pub backends: Vec<String>,
}

pub fn sanitize_hostname(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for ch in raw.trim().chars() {
        if ch == '.' {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if (ch == '-' || ch == '_') && !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "machine".to_string()
    } else {
        out
    }
}

pub(super) fn silent_cmd(program: &str) -> std::process::Command {
    crate::proc::bg_std_cmd(program)
}

#[cfg(windows)]
fn windows_total_memory_bytes() -> u64 {
    #[repr(C)]
    struct MemoryStatusEx {
        dw_length: u32,
        dw_memory_load: u32,
        ull_total_phys: u64,
        ull_avail_phys: u64,
        ull_total_page_file: u64,
        ull_avail_page_file: u64,
        ull_total_virtual: u64,
        ull_avail_virtual: u64,
        ull_avail_extended_virtual: u64,
    }
    extern "system" {
        fn GlobalMemoryStatusEx(lp_buffer: *mut MemoryStatusEx) -> i32;
    }
    let mut mem = MemoryStatusEx {
        dw_length: std::mem::size_of::<MemoryStatusEx>() as u32,
        dw_memory_load: 0,
        ull_total_phys: 0,
        ull_avail_phys: 0,
        ull_total_page_file: 0,
        ull_avail_page_file: 0,
        ull_total_virtual: 0,
        ull_avail_virtual: 0,
        ull_avail_extended_virtual: 0,
    };
    let ok = unsafe { GlobalMemoryStatusEx(&mut mem) };
    if ok != 0 { mem.ull_total_phys } else { 0 }
}

pub(super) fn detect_short_hostname() -> String {
    #[cfg(not(windows))]
    if let Ok(out) = silent_cmd("hostname").arg("-s").output() {
        if out.status.success() {
            let s = sanitize_hostname(&String::from_utf8_lossy(&out.stdout));
            if !s.is_empty() {
                return s;
            }
        }
    }
    if let Ok(out) = silent_cmd("hostname").output() {
        if out.status.success() {
            let s = sanitize_hostname(&String::from_utf8_lossy(&out.stdout));
            if !s.is_empty() {
                return s;
            }
        }
    }
    for key in ["HOSTNAME", "COMPUTERNAME"] {
        if let Ok(h) = std::env::var(key) {
            let s = sanitize_hostname(&h);
            if !s.is_empty() {
                return s;
            }
        }
    }
    "machine".to_string()
}

pub(crate) fn companion_hanzo_link_running() -> bool {
    if cfg!(target_os = "linux") {
        if let Ok(out) = silent_cmd("systemctl")
            .args(["--user", "is-active", "hanzo-node.service"])
            .output()
        {
            if String::from_utf8_lossy(&out.stdout).trim() == "active" {
                return true;
            }
        }
        if let Ok(entries) = std::fs::read_dir("/proc") {
            let my_pid = std::process::id().to_string();
            for entry in entries.flatten() {
                let fname = entry.file_name();
                let pid_str = fname.to_string_lossy();
                if pid_str == my_pid || !pid_str.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                if let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) {
                    let parts: Vec<&[u8]> = cmdline
                        .split(|b| *b == 0)
                        .filter(|s| !s.is_empty())
                        .collect();
                    if parts.len() >= 2 {
                        let argv0 = String::from_utf8_lossy(parts[0]);
                        let argv1 = String::from_utf8_lossy(parts[1]);
                        if argv0.ends_with("/hanzo") && argv1 == "link" {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

pub fn detect_hardware() -> HardwareInventory {
    let hostname = detect_short_hostname();
    let os = match std::env::consts::OS {
        "macos" => "darwin".to_string(),
        other => other.to_string(),
    };
    let arch = silent_cmd("uname")
        .arg("-m")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::consts::ARCH.to_string());
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    let mut cpu_model = String::new();
    let mut memory_bytes: u64 = 0;

    if cfg!(target_os = "macos") {
        if let Ok(o) = silent_cmd("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
        {
            cpu_model = String::from_utf8_lossy(&o.stdout).trim().to_string();
        }
        if let Ok(o) = silent_cmd("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
        {
            memory_bytes = String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<u64>()
                .unwrap_or(0);
        }
    } else if cfg!(target_os = "linux") {
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            for line in meminfo.lines() {
                if let Some(rest) = line.strip_prefix("MemTotal:") {
                    if let Some(kb_str) = rest.split_whitespace().next() {
                        if let Ok(kb) = kb_str.parse::<u64>() {
                            memory_bytes = kb * 1024;
                        }
                    }
                    break;
                }
            }
        }
        if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in cpuinfo.lines() {
                if let Some((_, val)) = line.split_once(':') {
                    let key = line.split(':').next().unwrap_or("").trim();
                    if matches!(key, "model name" | "Model name" | "Hardware" | "Processor") {
                        let v = val.trim();
                        if !v.is_empty() {
                            cpu_model = v.to_string();
                            break;
                        }
                    }
                }
            }
        }
        if cpu_model.is_empty() {
            if let Ok(o) = silent_cmd("lscpu").output() {
                for line in String::from_utf8_lossy(&o.stdout).lines() {
                    if let Some(rest) = line.strip_prefix("Model name:") {
                        let v = rest.trim();
                        if !v.is_empty() {
                            cpu_model = v.to_string();
                            break;
                        }
                    }
                }
            }
        }
    } else if cfg!(target_os = "windows") {
        #[cfg(windows)]
        {
            memory_bytes = windows_total_memory_bytes();
        }
        if let Ok(o) = silent_cmd("reg")
            .args([
                "query",
                r"HKLM\HARDWARE\Description\System\CentralProcessor\0",
                "/v",
                "ProcessorNameString",
            ])
            .output()
        {
            if o.status.success() {
                for line in String::from_utf8_lossy(&o.stdout).lines() {
                    if let Some((_, rhs)) = line.split_once("REG_SZ") {
                        let v = rhs.trim();
                        if !v.is_empty() {
                            cpu_model = v.to_string();
                            break;
                        }
                    }
                }
            }
        }
        if cpu_model.is_empty() {
            if let Ok(id) = std::env::var("PROCESSOR_IDENTIFIER") {
                cpu_model = id.trim().to_string();
            }
        }
    }

    let mut gpus = Vec::new();
    let mut cuda = String::new();
    let mut driver = String::new();
    let mut rocm = String::new();
    let mut backends = vec!["CPU".to_string()];

    if let Ok(o) = silent_cmd("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total,compute_cap,driver_version",
            "--format=csv,noheader",
        ])
        .output()
    {
        if o.status.success() {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                let cols: Vec<&str> = line.split(',').map(str::trim).collect();
                if cols.is_empty() || cols[0].is_empty() {
                    continue;
                }
                let name = cols[0].to_string();
                let raw_mem = cols.get(1).copied().unwrap_or("");
                let cap = cols.get(2).copied().unwrap_or("");
                if let Some(drv) = cols.get(3).copied().filter(|s| !s.is_empty()) {
                    driver = drv.to_string();
                }
                let unified = raw_mem.contains("N/A") || raw_mem.is_empty();
                let memory_total = if unified && memory_bytes > 0 {
                    format!("{} MiB", memory_bytes >> 20)
                } else {
                    raw_mem.to_string()
                };
                gpus.push(GpuInventory {
                    name,
                    memory_total,
                    arch: if cap.is_empty() {
                        String::new()
                    } else {
                        format!("sm_{}", cap.replace('.', ""))
                    },
                    unified,
                });
            }
            if !gpus.is_empty() {
                backends.push("CUDA".to_string());
                backends.push("VULKAN".to_string());
                if let Ok(hdr) = silent_cmd("nvidia-smi").output() {
                    let txt = String::from_utf8_lossy(&hdr.stdout);
                    if let Some(pos) = txt.find("CUDA Version:") {
                        let after = &txt[pos + "CUDA Version:".len()..];
                        if let Some(ver) = after.split_whitespace().next() {
                            cuda = ver.trim_matches('|').to_string();
                        }
                    }
                }
            }
        }
    }

    if gpus.is_empty() && cfg!(target_os = "macos") && (arch == "arm64" || arch == "aarch64") {
        let name = if cpu_model.is_empty() {
            "Apple Silicon GPU".to_string()
        } else {
            cpu_model.clone()
        };
        gpus.push(GpuInventory {
            name,
            memory_total: if memory_bytes > 0 {
                format!("{} MiB", memory_bytes >> 20)
            } else {
                String::new()
            },
            arch: "metal".to_string(),
            unified: true,
        });
        backends.push("METAL".to_string());
    }

    if gpus.is_empty() && cfg!(target_os = "linux") {
        if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let s = name.to_string_lossy();
                if !s.starts_with("card") || s.contains('-') {
                    continue;
                }
                let vendor = std::fs::read_to_string(entry.path().join("device/vendor"))
                    .unwrap_or_default();
                if vendor.trim() == "0x1002" {
                    gpus.push(GpuInventory {
                        name: "AMD Radeon / Instinct GPU".to_string(),
                        memory_total: String::new(),
                        arch: "rocm".to_string(),
                        unified: false,
                    });
                    rocm = std::fs::read_to_string("/opt/rocm/.info/version")
                        .ok()
                        .map(|v| v.trim().to_string())
                        .unwrap_or_default();
                    backends.push("ROCM".to_string());
                    backends.push("VULKAN".to_string());
                    break;
                }
            }
        }
    }

    if gpus.is_empty() {
        if let Ok(o) = silent_cmd("vulkaninfo").arg("--summary").output() {
            if o.status.success() {
                let mut cur_name = String::new();
                let mut cur_type = String::new();
                let mut cur_driver = String::new();
                let flush_gpu = |gpus: &mut Vec<GpuInventory>,
                                 driver_out: &mut String,
                                 name: &mut String,
                                 dtype: &mut String,
                                 drv: &mut String| {
                    let n = name.trim();
                    if !n.is_empty()
                        && !dtype.contains("CPU")
                        && !n.to_ascii_lowercase().contains("llvmpipe")
                    {
                        let unified = dtype.contains("INTEGRATED_GPU");
                        let memory_total = if unified && memory_bytes > 0 {
                            format!("{} MiB", memory_bytes >> 20)
                        } else {
                            String::new()
                        };
                        if driver_out.is_empty() && !drv.trim().is_empty() {
                            *driver_out = drv.trim().to_string();
                        }
                        gpus.push(GpuInventory {
                            name: n.to_string(),
                            memory_total,
                            arch: "vulkan".to_string(),
                            unified,
                        });
                    }
                    name.clear();
                    dtype.clear();
                    drv.clear();
                };
                for line in String::from_utf8_lossy(&o.stdout).lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("GPU") && trimmed.ends_with(':') {
                        flush_gpu(&mut gpus, &mut driver, &mut cur_name, &mut cur_type, &mut cur_driver);
                        continue;
                    }
                    if let Some((k, v)) = trimmed.split_once('=') {
                        match k.trim() {
                            "deviceName" => cur_name = v.trim().to_string(),
                            "deviceType" => cur_type = v.trim().to_string(),
                            "driverInfo" => cur_driver = v.trim().to_string(),
                            _ => {}
                        }
                    }
                }
                flush_gpu(&mut gpus, &mut driver, &mut cur_name, &mut cur_type, &mut cur_driver);
                if !gpus.is_empty() && !backends.iter().any(|b| b == "VULKAN") {
                    backends.push("VULKAN".to_string());
                }
            }
        }
    }

    HardwareInventory {
        hostname,
        os,
        arch,
        cpus,
        cpu_model,
        memory_bytes,
        cuda,
        driver,
        rocm,
        gpus,
        backends,
    }
}
