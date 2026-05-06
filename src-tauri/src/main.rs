#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod brain;
mod config;
mod tools;
mod vault;

use brain::{Brain, StreamEvent};
use config::Config;
use vault::VaultIndex;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

struct AppState {
    brain: Arc<Mutex<Brain>>,
    config: Arc<Mutex<Config>>,
    vault: Arc<VaultIndex>,
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
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Token(token) => {
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

    let state = AppState {
        brain: Arc::new(Mutex::new(brain)),
        config: Arc::new(Mutex::new(config)),
        vault: vault.clone(),
    };

    // Set vault reference on brain (after state is created)
    let brain_ref = state.brain.clone();
    let vault_ref = vault.clone();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            brain_ref.lock().await.set_vault(vault_ref);
        });

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            send_message,
            clear_history,
            get_config,
            save_config,
            search_vault,
            reindex_vault,
        ])
        .run(tauri::generate_context!())
        .expect("error running JARVIS");
}
