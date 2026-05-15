pub mod asset_loader;
pub mod error;
pub mod file_watcher;
pub mod hooks_server;
pub mod hotkey;
pub mod jsonl_parser;
pub mod layout_persistence;
pub mod notifier;
pub mod pet_window;
pub mod session_map;
pub mod session_registry;
pub mod settings;
pub mod tray;

use error::MutexExt;
use file_watcher::start_watcher;
use layout_persistence::{load_layout, save_layout};
use session_registry::{list_sessions, new_registry, scan_projects};
use settings::{get_settings, set_settings};

use tauri::{AppHandle, Emitter, Listener, Manager};
use tauri_plugin_store::StoreExt;
use tauri_plugin_window_state::{AppHandleExt, StateFlags, WindowExt};
use tracing::error;

/// Tauri command — opens ~/.claude/projects/ in the system file explorer.
/// Silently no-ops if the directory doesn't exist yet.
#[tauri::command]
fn open_sessions_folder() -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Cannot resolve home directory")?;
    let projects_root = home.join(".claude").join("projects");

    // Create the directory if absent so the shell opener doesn't error.
    if !projects_root.exists() {
        std::fs::create_dir_all(&projects_root)
            .map_err(|e| format!("Failed to create sessions folder: {e}"))?;
    }

    // Use the opener plugin (or fall back to platform shell command).
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(projects_root.to_string_lossy().as_ref())
            .spawn()
            .map_err(|e| format!("Failed to open explorer: {e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(projects_root.to_string_lossy().as_ref())
            .spawn()
            .map_err(|e| format!("Failed to open finder: {e}"))?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(projects_root.to_string_lossy().as_ref())
            .spawn()
            .map_err(|e| format!("Failed to open file manager: {e}"))?;
    }

    Ok(())
}

/// Toggle the pet-mode window. Returns the new state: `true` if open, `false` if closed.
#[tauri::command]
fn toggle_pet_mode(app: AppHandle) -> Result<bool, String> {
    let new_state = if pet_window::is_pet_window_open(&app) {
        pet_window::close_pet_window(&app)?;
        false
    } else {
        pet_window::create_pet_window(&app)?;
        true
    };
    // Broadcast so the main window's toolbar can sync its label.
    let _ = app.emit("pet-mode-changed", new_state);
    Ok(new_state)
}

/// Bring the main window forward and focus it.
#[tauri::command]
fn focus_main_window(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        win.set_focus()
            .map_err(|e| format!("Failed to focus main window: {e}"))?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let registry = new_registry();
    let session_agent_map = file_watcher::new_session_agent_map();
    let active_agents = tray::new_active_agents();
    let shared_tray = tray::new_shared_tray_state();

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(registry.clone())
        .manage(session_agent_map.clone())
        .manage(active_agents.clone())
        .manage(shared_tray.clone())
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            get_settings,
            set_settings,
            save_layout,
            load_layout,
            asset_loader::scan_external_assets,
            asset_loader::list_sprite_packs,
            asset_loader::load_sprite_pack,
            open_sessions_folder,
            toggle_pet_mode,
            focus_main_window,
        ])
        .setup(move |app| {
            // Restore saved window size and position (tauri-plugin-window-state).
            // Must be called after the plugin is registered and the window exists.
            if let Some(window) = app.get_webview_window("main") {
                if let Err(e) = window.restore_state(StateFlags::SIZE | StateFlags::POSITION) {
                    tracing::warn!("Could not restore window state: {e}");
                }
            }

            // Initial project scan
            match scan_projects() {
                Ok(sessions) => {
                    let mut reg = registry.lock_or_recover();
                    *reg = sessions;
                }
                Err(e) => error!("Initial scan failed: {e}"),
            }

            // Persist window size/position on every resize or move so state is
            // saved even when the process is killed (e.g. `tauri dev` hot-reload).
            let save_handle = app.handle().clone();
            if let Some(window) = app.get_webview_window("main") {
                window.on_window_event(move |event| {
                    if matches!(
                        event,
                        tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Moved(_)
                    ) {
                        let _ = save_handle
                            .save_window_state(StateFlags::SIZE | StateFlags::POSITION);
                    }
                });
            }

            // Rich system tray: live agent count + Pet Mode + Refresh + Show/Quit.
            tray::build_tray(app.handle(), active_agents.clone(), shared_tray.clone())?;

            // Register the configured global shortcut (default Ctrl+Shift+P).
            let hotkey_str: Option<String> = app
                .store("settings.json")
                .ok()
                .and_then(|store| store.get("settings"))
                .and_then(|v| v.get("globalHotkey").cloned())
                .and_then(|v| match v {
                    serde_json::Value::String(s) => Some(s),
                    _ => None,
                })
                .or_else(|| Some("Ctrl+Shift+P".to_owned()));
            hotkey::register(app.handle(), hotkey_str.as_deref());

            // F2: Watch layout.json for external manual edits.
            layout_persistence::start_layout_watcher(app.handle().clone());

            // session_agent_map is created before setup() and managed as Tauri state
            // so list_sessions can resolve the correct agent IDs for the frontend.

            // Generate a per-session token for the hooks server.
            // Expose it as an env var so Claude Code hooks config can read it.
            let hook_token = uuid::Uuid::new_v4().to_string();
            std::env::set_var("PIXEL_AGENTS_HOOK_TOKEN", &hook_token);

            // F3: Start the Claude Code Hooks API HTTP server.
            {
                let hook_handle = app.handle().clone();
                let hook_map = std::sync::Arc::clone(&session_agent_map);
                let token = hook_token.clone();
                std::thread::spawn(move || {
                    hooks_server::start(hook_handle, hook_map, token);
                });
            }

            // Start filesystem watcher (passes shared session_agent_map).
            let handle = app.handle().clone();
            let reg = registry.clone();
            let watcher_map = std::sync::Arc::clone(&session_agent_map);
            std::thread::spawn(move || {
                if let Err(e) = start_watcher(handle, reg, watcher_map) {
                    error!("File watcher failed to start: {e}");
                }
            });

            // Listen to agent-event for native notifications + tray count updates.
            {
                let tray_handle = app.handle().clone();
                let shared_tray_ev = shared_tray.clone();
                app.listen("agent-event", move |event| {
                    let payload: serde_json::Value =
                        match serde_json::from_str(event.payload()) {
                            Ok(v) => v,
                            Err(_) => return,
                        };
                    let evt_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let id = payload
                        .get("id")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize);

                    match (evt_type, id) {
                        ("agentStatus", Some(aid)) => {
                            let status =
                                payload.get("status").and_then(|v| v.as_str()).unwrap_or("");
                            match status {
                                "active" => tray::mark_active(&shared_tray_ev, aid),
                                "waiting" => {
                                    notifier::notify_waiting(&tray_handle, aid);
                                }
                                _ => {}
                            }
                        }
                        ("agentToolPermission", Some(aid)) => {
                            notifier::notify_permission(&tray_handle, aid);
                        }
                        ("agentCreated", Some(aid)) => {
                            tray::mark_active(&shared_tray_ev, aid);
                        }
                        ("agentClosed", Some(aid)) => {
                            tray::mark_inactive(&shared_tray_ev, aid);
                        }
                        _ => {}
                    }
                });
            }

            // Tray "Refresh" menu item asks the frontend to re-bootstrap.
            // The frontend listens to `tray-refresh` via vscode-shim.
            // (The emit from the tray handler reaches the webview through Tauri's event bus.)

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}
