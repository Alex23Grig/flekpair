mod device;
mod error;
mod pairing;

use crate::{
    device::{DeviceCache, list_devices},
    pairing::{
        LastExport, PairingCancelToken, RemotePairings, cancel_pairing, export_pairing_file,
        reveal_pairing_file,
    },
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(DeviceCache::default())
        .manage(PairingCancelToken::default())
        .manage(RemotePairings::default())
        .manage(LastExport::default())
        .invoke_handler(tauri::generate_handler![
            list_devices,
            export_pairing_file,
            cancel_pairing,
            reveal_pairing_file,
        ])
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
