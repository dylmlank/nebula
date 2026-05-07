//! JARVIS v4 System Observer — environment monitoring.
//! Monitors: processes, hardware, peripherals, active windows.
//! Exposes gather_system_state() returning full environment snapshot.

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
    pub network_interfaces: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeripheralState {
    pub microphone_active: bool,
    pub webcam_active: bool,
    pub displays: u32,
    pub bluetooth_devices: Vec<String>,
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
    pub open_projects: Vec<String>,
}

pub fn gather_system_state() -> SystemState {
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything())
    );

    // Need a brief pause for CPU measurements to be meaningful
    std::thread::sleep(std::time::Duration::from_millis(200));
    sys.refresh_cpu_all();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    // Hardware
    let cpu_usage = sys.global_cpu_usage();
    let cpu_cores = sys.cpus().len();
    let mem_total = sys.total_memory() as f64 / 1_073_741_824.0;
    let mem_used = sys.used_memory() as f64 / 1_073_741_824.0;
    let mem_avail = sys.available_memory() as f64 / 1_073_741_824.0;
    let mem_percent = if mem_total > 0.0 { (mem_used / mem_total * 100.0) as f32 } else { 0.0 };

    // Disk
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

    // Network interfaces
    let networks = Networks::new_with_refreshed_list();
    let net_interfaces: Vec<String> = networks.iter()
        .map(|(name, _)| name.clone())
        .filter(|n| !n.starts_with("lo") && !n.starts_with("docker") && !n.starts_with("veth"))
        .collect();

    // Peripherals
    let mic_active = check_mic_active();
    let webcam_active = check_webcam_active();
    let displays = count_displays();
    let bt_devices = get_bluetooth_devices();

    // Processes
    let active_window = get_active_window();
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

    // Categorize running processes
    let categories = categorize_processes(&sys);

    // Workspace detection
    let current_mode = detect_workspace_mode(&categories, &active_window);
    let open_projects = detect_open_projects(&sys);

    SystemState {
        timestamp: chrono::Local::now().to_rfc3339(),
        hardware: HardwareState {
            cpu_usage,
            cpu_cores,
            memory_total_gb: mem_total,
            memory_used_gb: mem_used,
            memory_available_gb: mem_avail,
            memory_percent: mem_percent,
            disk_total_gb: disk_total as f64 / 1_073_741_824.0,
            disk_used_gb: disk_used as f64 / 1_073_741_824.0,
            disk_free_gb: disk_free as f64 / 1_073_741_824.0,
            network_interfaces: net_interfaces,
        },
        peripherals: PeripheralState {
            microphone_active: mic_active,
            webcam_active: webcam_active,
            displays,
            bluetooth_devices: bt_devices,
        },
        processes: ProcessState {
            active_window,
            top_processes: top_procs,
            categories,
        },
        workspace: WorkspaceState {
            current_mode,
            open_projects,
        },
    }
}

fn check_mic_active() -> bool {
    // Check if any process is using the microphone
    Command::new("sh")
        .arg("-c")
        .arg("fuser /dev/snd/* 2>/dev/null | grep -q . || pactl list source-outputs 2>/dev/null | grep -q 'Source Output'")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn check_webcam_active() -> bool {
    // Check if webcam device is in use
    Command::new("sh")
        .arg("-c")
        .arg("fuser /dev/video0 2>/dev/null | grep -q .")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn count_displays() -> u32 {
    Command::new("sh")
        .arg("-c")
        .arg("xrandr --query 2>/dev/null | grep ' connected' | wc -l")
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(1)
}

fn get_bluetooth_devices() -> Vec<String> {
    Command::new("bluetoothctl")
        .args(["devices", "Connected"])
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(|l| l.split_whitespace().nth(2).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn get_active_window() -> String {
    Command::new("sh")
        .arg("-c")
        .arg("xdotool getactivewindow getwindowname 2>/dev/null || echo 'unknown'")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn categorize_processes(sys: &System) -> HashMap<String, Vec<String>> {
    let mut categories: HashMap<String, Vec<String>> = HashMap::new();

    let category_map: &[(&str, &[&str])] = &[
        ("browsers", &["firefox", "chrome", "chromium", "brave", "vivaldi"]),
        ("editors", &["code", "nvim", "vim", "emacs", "sublime", "zed", "cursor"]),
        ("terminals", &["alacritty", "kitty", "wezterm", "gnome-terminal", "konsole", "foot"]),
        ("media", &["spotify", "vlc", "mpv", "obs", "audacity"]),
        ("comms", &["discord", "slack", "telegram", "signal", "teams"]),
        ("dev_tools", &["docker", "node", "cargo", "python", "java", "postgres", "redis"]),
        ("creative", &["gimp", "blender", "inkscape", "kdenlive", "davinci"]),
    ];

    for (category, keywords) in category_map {
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
        if !found.is_empty() {
            categories.insert(category.to_string(), found);
        }
    }

    categories
}

fn detect_workspace_mode(categories: &HashMap<String, Vec<String>>, active_window: &str) -> String {
    let window_lower = active_window.to_lowercase();

    // Streaming mode: OBS or streaming apps running
    if categories.get("media").map(|m| m.iter().any(|p| p.contains("obs"))).unwrap_or(false) {
        return "streaming".into();
    }

    // Coding mode: editor in focus or multiple dev tools
    if categories.contains_key("editors") || window_lower.contains("code") || window_lower.contains("vim") {
        return "coding".into();
    }

    // Creative mode: blender/gimp/etc
    if categories.contains_key("creative") {
        return "creative".into();
    }

    // Communication mode: discord/slack in focus
    if window_lower.contains("discord") || window_lower.contains("slack") {
        return "communication".into();
    }

    "general".into()
}

fn detect_open_projects(sys: &System) -> Vec<String> {
    let mut projects = Vec::new();

    // Look for editor processes with file paths in their args
    for proc in sys.processes().values() {
        let name = proc.name().to_string_lossy().to_lowercase();
        if name.contains("code") || name.contains("nvim") || name.contains("vim") {
            let cmd: Vec<String> = proc.cmd().iter().map(|s| s.to_string_lossy().to_string()).collect();
            for arg in &cmd {
                if arg.starts_with('/') && arg.contains("/home/") && !arg.contains("/usr/") {
                    // Likely a project directory or file
                    let path = std::path::Path::new(arg.as_str());
                    let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
                    let dir_str = dir.to_string_lossy().to_string();
                    if !projects.contains(&dir_str) && projects.len() < 5 {
                        projects.push(dir_str);
                    }
                }
            }
        }
    }

    projects
}
