// Core modules are declared at the crate root via `#[path]` so the ported
// code's existing `crate::message::…` / `crate::config::…` references keep
// working unchanged. The files physically live under `src/core/`.
#[path = "core/message.rs"]
pub mod message;
#[path = "core/api.rs"]
pub mod api;
#[path = "core/config.rs"]
pub mod config;
#[path = "core/slash.rs"]
pub mod slash;
#[path = "core/markdown.rs"]
pub mod markdown;
#[path = "core/tools.rs"]
pub mod tools;
#[path = "core/agents.rs"]
pub mod agents;

mod commands;
mod commands/chat_commands;
mod commands/config_commands;
mod commands/project_commands;
mod commands/usage_commands;
mod commands/mcp_commands;
mod commands/system_commands;
mod mcp;
mod metrics;
mod state;

use state::AppState;
use tauri::Manager;

/// One-time move of pre-rename data from the legacy `hchat` directories to
/// `fornax`, so anyone upgrading keeps their conversations, config, and tools.
/// Runs before any path is read; no-ops once the `fornax` dirs exist.
fn migrate_legacy_dirs() {
    use std::fs;
    // config + tools live under config_dir; the SQLite db under data_dir.
    // (these are the same directory on macOS, distinct on Linux.)
    for base in [dirs::config_dir(), dirs::data_dir()].into_iter().flatten() {
        let old = base.join("hchat");
        let new = base.join("fornax");
        if new.exists() || !old.exists() {
            continue;
        }
        if fs::rename(&old, &new).is_ok() {
            // Rename the db file (+ its WAL/SHM sidecars) to match the new name.
            for ext in ["", "-wal", "-shm"] {
                let from = new.join(format!("hchat.db{ext}"));
                if from.exists() {
                    let _ = fs::rename(&from, new.join(format!("fornax.db{ext}")));
                }
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    migrate_legacy_dirs();
    tauri::Builder::default()
        .manage(AppState::new())
        .setup(|app| {
            // Spawn the metrics poller + macmon thread; expose the active-target
            // handle so commands can retarget it.
            let target = metrics::start(app.handle().clone());
            app.manage(metrics::MetricsHandle(target));

            // Connect MCP servers in the background.
            let state = app.state::<AppState>();

            // Enforce the usage retention window once at startup.
            let retention = state.config.lock().unwrap().usage_retention_days;
            if retention > 0 {
                state.usage_storage.lock().unwrap().prune_usage(retention);
            }

            let mcp = state.mcp.clone();
            let servers = state.config.lock().unwrap().mcp_servers.clone();
            if !servers.is_empty() {
                tauri::async_runtime::spawn(async move {
                    mcp.connect_all(servers).await;
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Config commands
            commands::config_commands::get_config,
            commands::config_commands::save_config,
            commands::config_commands::fetch_models,
            
            // Chat commands
            commands::chat_commands::list_conversations,
            commands::chat_commands::load_conversation,
            commands::chat_commands::delete_conversation,
            commands::chat_commands::delete_all_conversations,
            commands::chat_commands::rename_conversation,
            commands::chat_commands::set_pinned,
            commands::chat_commands::search_conversations,
            commands::chat_commands::edit_message,
            commands::chat_commands::message_siblings,
            commands::chat_commands::walk_from,
            commands::chat_commands::cancel_stream,
            commands::chat_commands::resolve_tool,
            commands::chat_commands::save_draft,
            
            // Project commands
            commands::project_commands::list_projects,
            commands::project_commands::create_project,
            commands::project_commands::rename_project,
            commands::project_commands::delete_project,
            commands::project_commands::set_project_pinned,
            commands::project_commands::set_conversation_project,
            
            // Usage commands
            commands::usage_commands::usage_stats,
            commands::usage_commands::clear_usage,
            commands::usage_commands::run_benchmark,
            
            // MCP commands
            commands::mcp_commands::list_mcp_servers,
            commands::mcp_commands::reconnect_mcp,
            
            // System commands
            commands::system_commands::export_conversation,
            commands::system_commands::export_conversation_file,
            commands::system_commands::save_draft,
            commands::system_commands::list_agents,
            commands::system_commands::list_presets,
            commands::system_commands::create_preset,
            commands::system_commands::delete_preset,

        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}