mod apple_driver;
mod device;
mod error;
mod nearby;
mod pairing;
mod symbol;

use crate::{
    apple_driver::{
        DriverSetup, apple_driver_progress, apple_driver_state, cancel_apple_driver,
        install_apple_driver,
    },
    device::{DeviceCache, KeepAwake, list_devices},
    nearby::{Nearby, network_search},
    pairing::{
        LastExport, PairingCancelToken, RemotePairings, cancel_pairing, export_pairing_file,
        reveal_pairing_file,
    },
    symbol::system_symbol,
};

use tauri::{Manager, WindowEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(DeviceCache::default())
        .manage(KeepAwake::default())
        .manage(Nearby::default())
        .manage(PairingCancelToken::default())
        .manage(RemotePairings::default())
        .manage(LastExport::default())
        .manage(DriverSetup::default())
        .invoke_handler(tauri::generate_handler![
            list_devices,
            export_pairing_file,
            cancel_pairing,
            reveal_pairing_file,
            apple_driver_state,
            apple_driver_progress,
            install_apple_driver,
            cancel_apple_driver,
            system_symbol,
            network_search,
        ])
        .setup(|app| {
            if nearby::ENABLED {
                nearby::watch(app.handle().clone());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::Focused(true) = event {
                window.state::<KeepAwake>().extend();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Logs to stderr in debug builds, or at the level named by `FLEKPAIR_LOG` (e.g. `debug`).
fn init_logging() {
    let level = match std::env::var("FLEKPAIR_LOG") {
        Ok(level) => level.parse().unwrap_or(tracing::Level::DEBUG),
        Err(_) if cfg!(debug_assertions) => tracing::Level::INFO,
        Err(_) => return,
    };

    let _ = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(std::io::stderr)
        .try_init();
}
