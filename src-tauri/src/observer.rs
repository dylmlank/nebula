//! System Observer — environment monitoring via sysinfo.

use serde::{Deserialize, Serialize};
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, Networks, System, RefreshKind};
use std::collections::HashMap;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemState {
    pub timestamp: String,
    pub hardware: HardwareState,
    pub peripherals: PeripheralState,
    pub processes: ProcessState,
    pub workspace: WorkspaceState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareState {
    pub cpu_usage: f32,
    pub cpu_cores: usize,
    pub memory_total_gb: f64,
    pub memory_used_gb: f64,
    pub memory_available_gb: f64,
    pub memory_percent: f32,
    pub disk_total_gb: f64,
    pub disk_used_gb: f64,
    pub disk_free_gb: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeripheralState {
    pub microphone_active: bool,
    pub webcam_active: bool,
    pub displays: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub name: String,
    pub pid: u32,
    pub cpu_percent: f32,
    pub memory_mb: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessState {
    pub active_window: String,
    pub top_processes: Vec<ProcessInfo>,
    pub categories: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceState {
    pub current_mode: String,
}

pub fn gather_system_state() -> SystemState {
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything())
    );
    std::thread::sleep(std::time::Duration::from_millis(200));
    sys.refresh_cpu_all();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let cpu_usage = sys.global_cpu_usage();
    let cpu_cores = sys.cpus().len();
    let mem_total = sys.total_memory() as f64 / 1_073_741_824.0;
    let mem_used = sys.used_memory() as f64 / 1_073_741_824.0;
    let mem_avail = sys.available_memory() as f64 / 1_073_741_824.0;
    let mem_percent = if mem_total > 0.0 { (mem_used / mem_total * 100.0) as f32 } else { 0.0 };

    let disks = Disks::new_with_refreshed_list();
    let (mut disk_total, mut disk_used, mut disk_free) = (0u64, 0u64, 0u64);
    for disk in disks.list() {
        if disk.mount_point() == std::path::Path::new("/") {
            disk_total = disk.total_space();
            disk_free = disk.available_space();
            disk_used = disk_total.saturating_sub(disk_free);
            break;
        }
    }

    let mic_active = Command::new("sh").arg("-c")
        .arg("pactl list source-outputs 2>/dev/null | grep -q 'Source Output'")
        .status().map(|s| s.success()).unwrap_or(false);

    let webcam_active = Command::new("sh").arg("-c")
        .arg("fuser /dev/video0 2>/dev/null | grep -q .")
        .status().map(|s| s.success()).unwrap_or(false);

    let displays = Command::new("sh").arg("-c")
        .arg("xrandr --query 2>/dev/null | grep ' connected' | wc -l")
        .output().ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(1);

    let active_window = Command::new("sh").arg("-c")
        .arg("xdotool getactivewindow getwindowname 2>/dev/null || echo unknown")
        .output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());

    let mut top_procs: Vec<ProcessInfo> = sys.processes().values()
        .map(|p| ProcessInfo {
            name: p.name().to_string_lossy().to_string(),
            pid: p.pid().as_u32(),
            cpu_percent: p.cpu_usage(),
            memory_mb: p.memory() as f64 / 1_048_576.0,
        })
        .filter(|p| p.cpu_percent > 0.1 || p.memory_mb > 50.0)
        .collect();
    top_procs.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap_or(std::cmp::Ordering::Equal));
    top_procs.truncate(15);

    let categories = categorize_processes(&sys);
    let current_mode = detect_mode(&categories, &active_window);

    SystemState {
        timestamp: chrono::Local::now().to_rfc3339(),
        hardware: HardwareState {
            cpu_usage, cpu_cores,
            memory_total_gb: mem_total, memory_used_gb: mem_used,
            memory_available_gb: mem_avail, memory_percent: mem_percent,
            disk_total_gb: disk_total as f64 / 1_073_741_824.0,
            disk_used_gb: disk_used as f64 / 1_073_741_824.0,
            disk_free_gb: disk_free as f64 / 1_073_741_824.0,
        },
        peripherals: PeripheralState { microphone_active: mic_active, webcam_active, displays },
        processes: ProcessState { active_window, top_processes: top_procs, categories },
        workspace: WorkspaceState { current_mode },
    }
}

fn categorize_processes(sys: &System) -> HashMap<String, Vec<String>> {
    let mut categories: HashMap<String, Vec<String>> = HashMap::new();
    let map: &[(&str, &[&str])] = &[
        ("browsers", &["firefox", "chrome", "chromium", "brave"]),
        ("editors", &["code", "nvim", "vim", "zed", "cursor"]),
        ("terminals", &["alacritty", "kitty", "wezterm", "foot"]),
        ("media", &["spotify", "vlc", "mpv", "obs"]),
        ("comms", &["discord", "slack", "telegram"]),
        ("dev", &["docker", "node", "cargo", "python", "postgres"]),
    ];
    for (cat, keywords) in map {
        let mut found = Vec::new();
        for proc in sys.processes().values() {
            let name = proc.name().to_string_lossy().to_lowercase();
            for kw in *keywords {
                if name.contains(kw) && !found.contains(&name) {
                    found.push(name.clone());
                    break;
                }
            }
        }
        if !found.is_empty() { categories.insert(cat.to_string(), found); }
    }
    categories
}

fn detect_mode(categories: &HashMap<String, Vec<String>>, window: &str) -> String {
    let w = window.to_lowercase();
    if categories.get("media").map(|m| m.iter().any(|p| p.contains("obs"))).unwrap_or(false) { return "streaming".into(); }
    if categories.contains_key("editors") || w.contains("code") || w.contains("vim") { return "coding".into(); }
    if w.contains("discord") || w.contains("slack") { return "communication".into(); }
    "general".into()
}
