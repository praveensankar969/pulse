pub mod domain;
pub mod ipc;
pub mod store;

use tauri::Manager;

use crate::ipc::AppState;
use crate::store::{ConfigStore, Paths};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();

    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("popover") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }));
    }

    // Plumbing only: never check or install on launch. Off unless `--features updater`.
    #[cfg(all(desktop, feature = "updater"))]
    {
        builder = builder
            .plugin(tauri_plugin_process::init())
            .plugin(tauri_plugin_updater::Builder::new().build());
    }

    builder
        .setup(|app| {
            // Accessory + Info.plist LSUIElement: no Dock icon.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let paths = Paths::from_app(app.handle())?;
            let store = ConfigStore::open(paths)?;
            app.manage(AppState::new(store));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::commands::begin_reveal,
            ipc::commands::reveal_secret,
            ipc::commands::end_reveal,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
