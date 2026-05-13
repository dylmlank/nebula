#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod brain;
mod config;
mod observer;
mod tools;
mod vault;
mod voice;

use brain::{Brain, StreamEvent};
use config::Config;
use vault::VaultIndex;
use voice::VoiceEngine;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;

struct AppState {
    brain: Arc<Mutex<Brain>>,
    config: Arc<Mutex<Config>>,
    vault: Arc<VaultIndex>,
    voice: Arc<VoiceEngine>,
}

#[tauri::command]
async fn send_message(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
    message: String,
) -> Result<String, String> {
    let brain = state.brain.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();

    // Spawn task to stream and emit events
    let app_handle = app.clone();
    tokio::spawn(async move {
        let mut full_response = String::new();
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Token(token) => {
                    full_response.push_str(&token);
                    let _ = app_handle.emit("chat-token", &token);
                }
                StreamEvent::ToolStart { name, args } => {
                    let _ = app_handle.emit("tool-start", serde_json::json!({"name": name, "args": args}));
                }
                StreamEvent::ToolResult { name, result } => {
                    let _ = app_handle.emit("tool-result", serde_json::json!({"name": name, "result": result}));
                }
                StreamEvent::Done(full) => {
                    let _ = app_handle.emit("chat-done", &full);
                }
            }
        }
    });

    // Stream the response
    tokio::spawn(async move {
        let mut brain = brain.lock().await;
        brain.chat_stream(&message, tx).await;
    });

    Ok("streaming".into())
}

#[tauri::command]
async fn clear_history(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut brain = state.brain.lock().await;
    brain.clear();
    Ok(())
}

#[tauri::command]
async fn get_config(state: tauri::State<'_, AppState>) -> Result<Config, String> {
    let config = state.config.lock().await;
    Ok(config.clone())
}

#[tauri::command]
async fn save_config(
    state: tauri::State<'_, AppState>,
    config: Config,
) -> Result<(), String> {
    config.save()?;
    let mut current = state.config.lock().await;
    *current = config;
    Ok(())
}

#[tauri::command]
async fn search_vault(
    state: tauri::State<'_, AppState>,
    query: String,
) -> Result<Vec<vault::SearchResult>, String> {
    Ok(state.vault.search(&query, 8))
}

#[tauri::command]
async fn reindex_vault(
    state: tauri::State<'_, AppState>,
    force: bool,
) -> Result<String, String> {
    let stats = state.vault.index(force);
    Ok(format!(
        "Indexed: {}, Skipped: {}, Removed: {}, Total chunks: {}",
        stats.indexed, stats.skipped, stats.removed, stats.total_chunks
    ))
}

#[tauri::command]
async fn toggle_voice(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    Ok(state.voice.toggle())
}

#[tauri::command]
async fn get_voice_status(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    Ok(state.voice.is_enabled())
}

#[tauri::command]
async fn get_system_state() -> Result<observer::SystemState, String> {
    Ok(observer::gather_system_state())
}

#[tauri::command]
async fn start_voice_mode(app: AppHandle) -> Result<String, String> {
    // Find the sidecar script
    let sidecar_path = std::env::current_exe()
        .unwrap_or_default()
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("../../../sidecar/voice_sidecar.py");

    // Fallback paths
    let paths_to_try = [
        sidecar_path.clone(),
        dirs::home_dir().unwrap_or_default().join("jarvis-v3/sidecar/voice_sidecar.py"),
    ];

    let script_path = paths_to_try.iter()
        .find(|p| p.exists())
        .cloned()
        .ok_or_else(|| "Voice sidecar not found".to_string())?;

    let app_handle = app.clone();

    // Spawn the Python sidecar
    tokio::spawn(async move {
        use tokio::process::Command as TokioCommand;
        use tokio::io::{AsyncBufReadExt, BufReader};

        let mut child = match TokioCommand::new("python3")
            .arg(&script_path)
            .stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = app_handle.emit("voice-mode-event", serde_json::json!({"type": "error", "message": format!("Failed to start: {}", e)}));
                return;
            }
        };

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let mut reader = BufReader::new(stdout).lines();

        // Log stderr in background
        let _app_err = app_handle.clone();
        tokio::spawn(async move {
            let mut err_reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = err_reader.next_line().await {
                eprintln!("[voice-sidecar] {}", line);
            }
        });

        while let Ok(Some(line)) = reader.next_line().await {
            if let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) {
                let _ = app_handle.emit("voice-mode-event", &event);
            }
        }

        let _ = app_handle.emit("voice-mode-event", serde_json::json!({"type": "stopped"}));
    });

    Ok("Voice mode started".into())
}

#[tauri::command]
async fn stop_voice_mode() -> Result<(), String> {
    // Kill any running voice sidecar
    let _ = std::process::Command::new("pkill")
        .args(["-f", "voice_sidecar.py"])
        .output();
    Ok(())
}

#[tauri::command]
async fn speak_response(state: tauri::State<'_, AppState>, text: String) -> Result<(), String> {
    state.voice.speak(&text);
    Ok(())
}

#[tauri::command]
async fn stop_speaking() -> Result<(), String> {
    // Kill any running piper/aplay processes to stop speech immediately
    let _ = std::process::Command::new("pkill").args(["-f", "piper"]).output();
    let _ = std::process::Command::new("pkill").args(["-f", "aplay"]).output();
    Ok(())
}

#[tauri::command]
async fn list_conversations() -> Result<Vec<serde_json::Value>, String> {
    let convos_dir = dirs::config_dir()
        .unwrap_or_default()
        .join("nebula/conversations");
    let _ = std::fs::create_dir_all(&convos_dir);

    let mut convos = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&convos_dir) {
        for entry in entries.flatten() {
            if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
                        convos.push(data);
                    }
                }
            }
        }
    }

    // Sort by updated descending
    convos.sort_by(|a, b| {
        let a_time = a.get("updated").and_then(|t| t.as_str()).unwrap_or("");
        let b_time = b.get("updated").and_then(|t| t.as_str()).unwrap_or("");
        b_time.cmp(a_time)
    });

    Ok(convos)
}

#[tauri::command]
async fn save_conversation(id: String, data: serde_json::Value) -> Result<(), String> {
    let convos_dir = dirs::config_dir()
        .unwrap_or_default()
        .join("nebula/conversations");
    let _ = std::fs::create_dir_all(&convos_dir);
    let path = convos_dir.join(format!("{}.json", id));
    let json = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn delete_conversation(id: String) -> Result<(), String> {
    let path = dirs::config_dir()
        .unwrap_or_default()
        .join(format!("jarvis/conversations/{}.json", id));
    let _ = std::fs::remove_file(&path);
    Ok(())
}

fn main() {
    let config = Config::load();

    // Check for API key
    if config.api_key.is_empty() {
        eprintln!("Warning: No ANTHROPIC_API_KEY set. Set it in config or environment.");
    }

    let vault_path = config.vault_path_expanded();
    let vault = Arc::new(VaultIndex::new(vault_path));

    // Background index on startup
    let vault_clone = vault.clone();
    std::thread::spawn(move || {
        let stats = vault_clone.index(false);
        eprintln!(
            "Vault indexed: {} files, {} chunks",
            stats.indexed + stats.skipped,
            stats.total_chunks
        );
    });

    let brain = Brain::new(config.clone());

    // Voice engine needs a tokio runtime for the background worker
    // We'll initialize it inside the Tauri setup hook
    let voice_model = shellexpand::tilde(&config.voice.model).to_string();
    let voice_speed = config.voice.speed;
    let voice_enabled = config.voice.enabled;

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .setup(move |app| {
            let voice = VoiceEngine::new(voice_model, voice_speed, voice_enabled);

            // Set vault on brain synchronously (no tokio needed)
            let mut brain = brain;
            brain.set_vault(vault.clone());

            let state = AppState {
                brain: Arc::new(Mutex::new(brain)),
                config: Arc::new(Mutex::new(config)),
                vault: vault.clone(),
                voice,
            };

            // Start file watcher for auto-reindex
            vault.start_watcher();

            // Proactive system monitor — alerts on high CPU/RAM/disk
            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(30));
                    let state = observer::gather_system_state();

                    // Alert on high memory
                    if state.hardware.memory_percent > 90.0 {
                        let _ = app_handle.emit("system-alert", serde_json::json!({
                            "type": "warning",
                            "message": format!("High memory usage: {:.0}%", state.hardware.memory_percent)
                        }));
                    }

                    // Alert on low disk
                    if state.hardware.disk_free_gb < 5.0 {
                        let _ = app_handle.emit("system-alert", serde_json::json!({
                            "type": "warning",
                            "message": format!("Low disk space: {:.1}GB free", state.hardware.disk_free_gb)
                        }));
                    }

                    // Alert on high CPU sustained
                    if state.hardware.cpu_usage > 95.0 {
                        let _ = app_handle.emit("system-alert", serde_json::json!({
                            "type": "warning",
                            "message": format!("High CPU: {:.0}%", state.hardware.cpu_usage)
                        }));
                    }
                }
            });

            app.manage(state);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                // Save session summary on exit
                let state: tauri::State<AppState> = window.state();
                let brain = state.brain.clone();
                std::thread::spawn(move || {
                    // blocking_lock works from a sync context
                    let brain = brain.blocking_lock();
                    brain.save_session_summary();
                });
            }
        })
        .invoke_handler(tauri::generate_handler![
            send_message,
            clear_history,
            get_config,
            save_config,
            search_vault,
            reindex_vault,
            toggle_voice,
            get_voice_status,
            list_conversations,
            save_conversation,
            delete_conversation,
            start_voice_mode,
            stop_voice_mode,
            speak_response,
            stop_speaking,
            get_system_state,
        ])
        .run(tauri::generate_context!())
        .expect("error running Nebula");
}
