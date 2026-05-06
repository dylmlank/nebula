#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod brain;
mod config;
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
    let voice = state.voice.clone();
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
                    // Speak the response (non-blocking)
                    voice.speak(&full);
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
        ])
        .run(tauri::generate_context!())
        .expect("error running JARVIS");
}
