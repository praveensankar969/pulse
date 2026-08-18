pub mod domain;
pub mod ipc;
pub mod eval;
pub mod launch;
pub mod logging;
pub mod notify;
pub mod platform;
pub mod poller;
pub mod store;

use std::sync::Arc;

use tauri::{Manager, RunEvent};

use crate::ipc::AppState;
use crate::launch::{
    first_run_pending, harbor_services, mark_first_run_shown, merge_demo, pause_all, LaunchFlags,
};
use crate::notify::{handle_activation, NotifyHub, OsNotifier};
use crate::poller::scheduler::{Scheduler, SchedulerConfig, TauriEvents};
use crate::store::{ConfigStore, History, Paths, SecretStore};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();

    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            handle_activation(app, &args);
        }));
        builder = builder
            .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
                if let Some(window) = app.get_webview_window("popover") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }))
            .plugin(tauri_plugin_autostart::Builder::new().build())
            .plugin(tauri_plugin_global_shortcut::Builder::new().build())
            .plugin(tauri_plugin_dialog::init());
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
        .plugin(tauri_plugin_notification::init())
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
            let flags = LaunchFlags::from_args(std::env::args());
            let mut services = store.load_services()?;
            if flags.demo {
                services = merge_demo(services, harbor_services(chrono::Utc::now()));
            }
            if flags.paused {
                // Persist so a bad poller build stays dark until the operator unpauses.
                pause_all(&mut services);
            }
            if flags.demo || flags.paused {
                store.save_services(&services)?;
            }
            let settings = store.load_settings()?;
            let tray = crate::platform::tray::TrayHandle::new();
            let hub = NotifyHub::new(settings.sound);
            let scheduler = Scheduler::new(SchedulerConfig {
                services,
                settings: settings.clone(),
                history,
                secrets: Arc::clone(&secrets),
                events: Arc::new(TauriEvents(app.handle().clone())),
                notifier: Box::new(OsNotifier::new(app.handle().clone(), hub.clone())),
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
            app.manage(hub);
            app.manage(tray.clone());
            app.manage(AppState::new(store, secrets, handle));
            crate::platform::tray::install(app.handle(), tray)?;
            crate::platform::detail::install(app.handle());
            crate::platform::settings::install(app.handle());
            if first_run_pending(&paths) && crate::platform::tray::show_first_run(app.handle()) {
                mark_first_run_shown(&paths);
            }
            // Installed Windows toast may relaunch with `pulse:focus?id=`.
            let args: Vec<String> = std::env::args().collect();
            if crate::notify::parse_focus_args(&args).is_some() {
                handle_activation(app.handle(), &args);
            }
            crate::platform::autostart::install(app.handle(), &settings);
            Ok(())
        })
        .on_window_event(ipc::windows::on_window_event)
        .invoke_handler(tauri::generate_handler![
            ipc::commands::begin_reveal,
            ipc::commands::reveal_secret,
            ipc::commands::end_reveal,
            ipc::commands::list_services,
            ipc::commands::get_settings,
            ipc::commands::save_service,
            ipc::commands::test_draft,
            ipc::commands::open_editor,
            ipc::commands::close_editor,
            ipc::commands::save_service,
            ipc::commands::set_paused,
            ipc::commands::check_now,
            ipc::commands::check_all,
            ipc::commands::delete_service,
            ipc::commands::poller_dead,
            ipc::commands::quit,
            ipc::commands::open_action,
            ipc::commands::get_detail,
            ipc::commands::get_settings,
            ipc::commands::update_settings,
            ipc::commands::open_settings,
            ipc::commands::snooze,
            ipc::commands::open_action,
            ipc::commands::open_detail,
            ipc::commands::get_settings,
            ipc::commands::update_settings,
            ipc::commands::open_settings,
            ipc::commands::maybe_ask_launch_at_login,
            ipc::commands::pending_launch_prompt,
            ipc::commands::answer_launch_prompt,
            ipc::commands::export_config,
            ipc::commands::import_config,
            ipc::commands::reset_all,
            platform::tray::should_suppress_blur,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Dock relaunch only. Banner click is wait_for_response in notify/os.rs;
            // this accessory app has no Dock icon, so Reopen is not the click path.
            if matches!(event, RunEvent::Reopen { .. }) {
                handle_activation(app, &[]);
            }
        });
}
