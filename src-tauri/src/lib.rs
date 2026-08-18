pub mod domain;
pub mod ipc;
<<<<<<< HEAD
pub mod eval;
=======
pub mod logging;
>>>>>>> 6bae09b (Scheduler, stagger, pause + logging + watchdog)
pub mod notify;
pub mod poller;
pub mod store;

use std::sync::Arc;

use tauri::Manager;

use crate::ipc::AppState;
use crate::notify::NoopNotifier;
use crate::poller::scheduler::{Scheduler, SchedulerConfig, TauriEvents};
use crate::store::{ConfigStore, History, Paths, SecretStore};

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
        .plugin(crate::logging::tauri_log_plugin().build())
        .setup(|app| {
            // Accessory + Info.plist LSUIElement: no Dock icon.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let paths = Paths::from_app(app.handle())?;
            if let Err(error) = crate::logging::init(&paths.log_file()) {
                eprintln!("pulse log init failed: {error}");
            }
            let store = ConfigStore::open(paths.clone())?;
            let history = History::open_in(&paths)?;
            let secrets = Arc::new(SecretStore::new());
            let services = store.load_services()?;
            let settings = store.load_settings()?;
            let scheduler = Scheduler::new(SchedulerConfig {
                services,
                settings,
                history,
                secrets: Arc::clone(&secrets),
                events: Arc::new(TauriEvents(app.handle().clone())),
                notifier: Box::new(NoopNotifier),
                enable_jitter: true,
                // Tray painter in PR 10 reads poller_dead() / this hook.
                on_poller_dead: Arc::new(|_| {}),
            })?;
            let handle = scheduler.handle();
            tauri::async_runtime::spawn(async move {
                scheduler.run().await;
            });
            app.manage(AppState::new(store, secrets, handle));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::commands::begin_reveal,
            ipc::commands::reveal_secret,
            ipc::commands::end_reveal,
            ipc::commands::list_services,
            ipc::commands::set_paused,
            ipc::commands::check_now,
            ipc::commands::check_all,
            ipc::commands::delete_service,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
