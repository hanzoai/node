// HARDWARE DETECTION - SEE THE MATRIX
// Detect all available TEE hardware capabilities

use super::{HardwareCapabilities, PrivacyTier};
use std::fs;
use std::path::Path;
use std::process::Command;

/// Detect all hardware security capabilities
pub fn detect_hardware_capabilities() -> HardwareCapabilities {
    let mut caps = HardwareCapabilities::default();
    
    // Check CPU-based TEE support
    caps.sev_snp_available = detect_sev_snp();
    caps.tdx_available = detect_tdx();
    caps.sgx_available = detect_sgx();
    
    // Check GPU-based TEE support
    let (h100_cc, blackwell) = detect_nvidia_capabilities();
    caps.h100_cc_available = h100_cc;
    caps.blackwell_tee_io_available = blackwell;
    
    // Check SIM card support
    caps.sim_eid_available = detect_sim_card();
    
    // Determine maximum supported tier
    caps.max_supported_tier = determine_max_tier(&caps);
    
    log::info!("🔍 Hardware capabilities detected:");
    log::info!("  SEV-SNP: {}", caps.sev_snp_available);
    log::info!("  TDX: {}", caps.tdx_available);
    log::info!("  SGX: {}", caps.sgx_available);
    log::info!("  H100 CC: {}", caps.h100_cc_available);
    log::info!("  Blackwell TEE-I/O: {}", caps.blackwell_tee_io_available);
    log::info!("  SIM EID: {}", caps.sim_eid_available);
    log::info!("  Max tier: {:?}", caps.max_supported_tier);
    
    caps
}

/// Detect AMD SEV-SNP support
fn detect_sev_snp() -> bool {
    // Check for SEV-SNP device
    if Path::new("/dev/sev-guest").exists() {
        return true;
    }
    
    // Check CPU flags for SEV support
    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        if cpuinfo.contains("sev_snp") || cpuinfo.contains("sev_es") {
            return true;
        }
    }
    
    // Check if we're running on AMD EPYC with SEV support
    if let Ok(output) = Command::new("lscpu").output() {
        let info = String::from_utf8_lossy(&output.stdout);
        if info.contains("AMD EPYC") && info.contains("sev") {
            return true;
        }
    }
    
    false
}

/// Detect Intel TDX support
fn detect_tdx() -> bool {
    // Check for TDX device
    if Path::new("/dev/tdx-guest").exists() {
        return true;
    }
    
    // Check CPU flags for TDX support
    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        if cpuinfo.contains("tdx") || cpuinfo.contains("tdx_guest") {
            return true;
        }
    }
    
    // Check if we're on Intel Xeon with TDX
    if let Ok(output) = Command::new("lscpu").output() {
        let info = String::from_utf8_lossy(&output.stdout);
        if info.contains("Intel") && (info.contains("TDX") || info.contains("Sapphire Rapids")) {
            return true;
        }
    }
    
    false
}

/// Detect Intel SGX support
fn detect_sgx() -> bool {
    // Check for SGX device
    if Path::new("/dev/sgx_enclave").exists() || Path::new("/dev/sgx/enclave").exists() {
        return true;
    }
    
    // Check CPU flags
    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        if cpuinfo.contains("sgx") {
            return true;
        }
    }
    
    false
}

/// Detect NVIDIA GPU capabilities
fn detect_nvidia_capabilities() -> (bool, bool) {
    let mut h100_cc = false;
    let mut blackwell = false;
    
    // Try to run nvidia-smi
    if let Ok(output) = Command::new("nvidia-smi")
        .arg("--query-gpu=name,compute_cap")
        .arg("--format=csv,noheader")
        .output()
    {
        let info = String::from_utf8_lossy(&output.stdout);
        
        // Check for H100 with CC support
        if info.contains("H100") || info.contains("Hopper") {
            h100_cc = check_nvidia_cc_mode();
        }
        
        // Check for Blackwell GPUs
        if info.contains("Blackwell") || info.contains("GB100") || info.contains("GB200") {
            blackwell = true;
            log::info!("🚀 Blackwell GPU detected - TEE-I/O capable!");
        }
        
        // Check compute capability (9.0 = Hopper/H100, 10.0 = Blackwell)
        for line in info.lines() {
            if let Some(cap_str) = line.split(',').nth(1) {
                if let Ok(cap) = cap_str.trim().parse::<f32>() {
                    if cap >= 9.0 && !h100_cc {
                        h100_cc = check_nvidia_cc_mode();
                    }
                    if cap >= 10.0 {
                        blackwell = true;
                    }
                }
            }
        }
    }
    
    // Check CUDA version for CC support
    if let Ok(output) = Command::new("nvcc").arg("--version").output() {
        let version_info = String::from_utf8_lossy(&output.stdout);
        if version_info.contains("release 12") || version_info.contains("release 13") {
            // CUDA 12+ supports confidential computing
            log::debug!("CUDA 12+ detected, CC features may be available");
        }
    }
    
    (h100_cc, blackwell)
}

/// Check if NVIDIA Confidential Computing mode is enabled
fn check_nvidia_cc_mode() -> bool {
    // Check for CC mode in driver
    if let Ok(output) = Command::new("nvidia-smi")
        .arg("-q")
        .arg("-d")
        .arg("COMPUTE_MODE")
        .output()
    {
        let info = String::from_utf8_lossy(&output.stdout);
        if info.contains("CONFIDENTIAL_COMPUTE") || info.contains("CC_MODE") {
            return true;
        }
    }
    
    // Check for MIG with CC support
    if let Ok(output) = Command::new("nvidia-smi")
        .arg("mig")
        .arg("-lgip")
        .output()
    {
        let info = String::from_utf8_lossy(&output.stdout);
        if info.contains("CC") || info.contains("Confidential") {
            return true;
        }
    }
    
    // In development mode, check environment variable
    if cfg!(debug_assertions) {
        if std::env::var("HANZO_MOCK_H100_CC").unwrap_or_default() == "true" {
            log::warn!("⚠️ Mock H100 CC mode enabled via environment variable");
            return true;
        }
    }
    
    false
}

/// Detect SIM card reader
fn detect_sim_card() -> bool {
    // Check for common SIM card reader devices
    let sim_devices = [
        "/dev/sim0",
        "/dev/ttyUSB0",
        "/dev/ttyACM0",
        "/sys/class/smartcard",
    ];
    
    for device in &sim_devices {
        if Path::new(device).exists() {
            return true;
        }
    }
    
    // Check for PC/SC daemon (pcscd) for smart card support
    if let Ok(output) = Command::new("systemctl")
        .arg("is-active")
        .arg("pcscd")
        .output()
    {
        if output.status.success() {
            let status = String::from_utf8_lossy(&output.stdout);
            if status.trim() == "active" {
                return true;
            }
        }
    }
    
    // Check for mobile broadband modems
    if let Ok(output) = Command::new("mmcli").arg("-L").output() {
        if output.status.success() {
            return true;
        }
    }
    
    false
}

/// Determine maximum supported privacy tier based on capabilities
fn determine_max_tier(caps: &HardwareCapabilities) -> PrivacyTier {
    if caps.blackwell_tee_io_available {
        PrivacyTier::GpuTeeIo
    } else if caps.h100_cc_available {
        PrivacyTier::GpuCc
    } else if caps.sev_snp_available || caps.tdx_available || caps.sgx_available {
        PrivacyTier::CpuTee
    } else if caps.sim_eid_available {
        PrivacyTier::AtRest
    } else {
        PrivacyTier::Open
    }
}

/// Check if running in a cloud environment with TEE support
pub fn detect_cloud_tee_support() -> Option<String> {
    // AWS Nitro Enclaves
    if Path::new("/dev/nitro_enclaves").exists() {
        return Some("AWS Nitro Enclaves".to_string());
    }
    
    // Azure Confidential Computing
    if let Ok(dmi) = fs::read_to_string("/sys/class/dmi/id/sys_vendor") {
        if dmi.contains("Microsoft") {
            if detect_sev_snp() || detect_tdx() {
                return Some("Azure Confidential Computing".to_string());
            }
        }
    }
    
    // Google Confidential VMs
    if let Ok(dmi) = fs::read_to_string("/sys/class/dmi/id/product_name") {
        if dmi.contains("Google") {
            if detect_sev_snp() || detect_tdx() {
                return Some("Google Confidential VMs".to_string());
            }
        }
    }
    
    // IBM Secure Execution
    if Path::new("/sys/firmware/uv").exists() {
        return Some("IBM Secure Execution".to_string());
    }
    
    None
}

/// Get detailed hardware security info for diagnostics
pub fn get_security_diagnostics() -> String {
    let mut diag = String::new();
    
    diag.push_str("=== TEE Hardware Diagnostics ===\n\n");
    
    // CPU info
    if let Ok(output) = Command::new("lscpu").output() {
        let info = String::from_utf8_lossy(&output.stdout);
        for line in info.lines() {
            if line.contains("Model name") || line.contains("Vendor ID") || 
               line.contains("Flags") && line.contains("sev") {
                diag.push_str(&format!("{}\n", line));
            }
        }
    }
    
    // GPU info
    if let Ok(output) = Command::new("nvidia-smi").arg("-L").output() {
        diag.push_str("\nGPUs:\n");
        diag.push_str(&String::from_utf8_lossy(&output.stdout));
    }
    
    // Kernel modules
    if let Ok(output) = Command::new("lsmod").output() {
        let modules = String::from_utf8_lossy(&output.stdout);
        diag.push_str("\nSecurity modules:\n");
        for line in modules.lines() {
            if line.contains("sev") || line.contains("tdx") || line.contains("sgx") {
                diag.push_str(&format!("{}\n", line));
            }
        }
    }
    
    // Cloud environment
    if let Some(cloud) = detect_cloud_tee_support() {
        diag.push_str(&format!("\nCloud TEE: {}\n", cloud));
    }
    
    diag
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hardware_detection() {
        let caps = detect_hardware_capabilities();
        
        // At minimum we should detect open tier
        assert!(caps.max_supported_tier >= PrivacyTier::Open);
        
        // Check that tier calculation is consistent
        let calculated_tier = determine_max_tier(&caps);
        assert_eq!(calculated_tier, caps.max_supported_tier);
    }

    #[test]
    fn test_diagnostics_generation() {
        let diag = get_security_diagnostics();
        assert!(diag.contains("TEE Hardware Diagnostics"));
    }
}