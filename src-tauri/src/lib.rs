pub mod domain;
pub mod ipc;
pub mod eval;
pub mod logging;
pub mod notify;
pub mod platform;
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
            let tray = crate::platform::tray::TrayHandle::new();
            let scheduler = Scheduler::new(SchedulerConfig {
                services,
                settings,
                history,
                secrets: Arc::clone(&secrets),
                events: Arc::new(TauriEvents(app.handle().clone())),
                notifier: Box::new(NoopNotifier),
                enable_jitter: true,
                on_poller_dead: tray.poller_dead_hook(),
            })?;
            let handle = scheduler.handle();
            let wake = crate::platform::wake::listen({
                let handle = handle.clone();
                move |event| match event {
                    crate::platform::PowerEvent::Sleep => handle.on_os_sleep(),
                    crate::platform::PowerEvent::Wake => handle.on_os_wake(),
                }
            });
            app.manage(wake);
            tray.apply_services(&handle.views());
            if handle.poller_dead() {
                tray.set_poller_dead(true);
            }
            tauri::async_runtime::spawn(async move {
                scheduler.run().await;
            });
            app.manage(tray.clone());
            app.manage(AppState::new(store, secrets, handle));
            crate::platform::tray::install(app.handle(), tray)?;
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
            platform::tray::should_suppress_blur,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
