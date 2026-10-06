use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

// used https://github.com/jkcoxson/idevice_pair/ as a guide
use idevice::{
    IdeviceError, IdeviceService,
    lockdown::LockdownClient,
    pairing_file::PairingFile,
    provider::UsbmuxdProvider,
    remote_pairing::{RemotePairingLockdownService, RpPairingFile},
};
use plist_macro::{plist, plist_to_xml_bytes};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::{
    device::{LABEL, get_provider, get_usbmuxd, string_value},
    error::{AppError, chain},
};

const FILE_STEM: &str = "pairingFile";
const FILE_EXTENSION: &str = "plist";

const PAIR_RETRY_DELAY: Duration = Duration::from_secs(1);

pub type PairingCancelToken = Mutex<Option<CancellationToken>>;

/// Remote pairings made since launch, by UDID, so exporting again doesn't ask for trust again.
pub type RemotePairings = Mutex<HashMap<String, RemotePairing>>;

/// The pairing file written last, and its contents.
pub type LastExport = Mutex<Option<(PathBuf, Vec<u8>)>>;

#[derive(Clone)]
pub struct RemotePairing {
    host_label: String,
    file: RpPairingFile,
}

impl RemotePairing {
    fn new() -> Self {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let host_label = format!("{LABEL}-{}", &id[..6]);
        let file = RpPairingFile::generate(&host_label);
        Self { host_label, file }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedPairing {
    pub path: String,
    pub file_name: String,
}

/// Pairs with the device, writes its pairing file to the Downloads folder and shows it there.
#[tauri::command]
pub async fn export_pairing_file(
    app: AppHandle,
    cancel_state: State<'_, PairingCancelToken>,
    udid: String,
) -> Result<ExportedPairing, AppError> {
    let token = CancellationToken::new();
    {
        let mut guard = cancel_state.lock().unwrap();
        if let Some(old) = guard.replace(token.clone()) {
            old.cancel();
        }
    }

    let pairing_result = tokio::select! {
        _ = token.cancelled() => Err(AppError::Canceled("Pairing".into())),
        res = pairing_file(&app, &udid) => res,
    };

    if !token.is_cancelled() {
        let mut guard = cancel_state.lock().unwrap();
        *guard = None;
    }

    let path = save(&app, pairing_result?)?;

    if let Err(e) = app.opener().reveal_item_in_dir(&path) {
        warn!("Failed to show {} in its folder: {}", path.display(), e);
    }

    Ok(ExportedPairing {
        file_name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        path: path.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
pub async fn cancel_pairing(cancel_state: State<'_, PairingCancelToken>) -> Result<(), AppError> {
    let mut guard = cancel_state.lock().unwrap();
    if let Some(token) = guard.take() {
        token.cancel();
    }
    Ok(())
}

#[tauri::command]
pub fn reveal_pairing_file(app: AppHandle, last: State<'_, LastExport>) -> Result<(), AppError> {
    let Some(path) = last.lock().unwrap().as_ref().map(|(path, _)| path.clone()) else {
        return Ok(());
    };

    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|e| AppError::Filesystem("Failed to show the pairing file".into(), e.to_string()))
}

async fn pairing_file(app: &AppHandle, udid: &str) -> Result<Vec<u8>, AppError> {
    let provider = get_provider(udid).await?;

    let (record, mut lockdown) = trusted_session(&provider, udid).await?;

    lockdown
        .set_value(
            "EnableWifiDebugging",
            true.into(),
            Some("com.apple.mobile.wireless_lockdown"),
        )
        .await
        .map_err(|e| {
            AppError::LockdownPairing("Failed to enable wifi debugging".into(), chain(&e))
        })?;

    let version = string_value(&mut lockdown, "ProductVersion")
        .await
        .map_err(|e| AppError::DeviceComs("Failed to fetch ProductVersion".into(), chain(&e)))?;
    drop(lockdown);

    let lockdown_plist = lockdown_plist(record, udid)?;

    // rppairing is 17.4+
    if is_ios_version_below(&version, 17, 4) {
        return Ok(plist_to_xml_bytes(&lockdown_plist));
    }

    let rppairing_plist = rppairing_plist(app, &provider, udid).await?;

    let pairing_plist = plist!(dict {
        :< lockdown_plist,
        :< rppairing_plist,
    });

    Ok(plist_to_xml_bytes(&pairing_plist))
}

/// Returns this computer's pairing record for the device and a lockdown session started with
/// it, pairing first if the computer isn't trusted or the device no longer accepts the record.
async fn trusted_session(
    provider: &UsbmuxdProvider,
    udid: &str,
) -> Result<(PairingFile, LockdownClient), AppError> {
    match get_usbmuxd().await?.get_pair_record(udid).await {
        Ok(record) => {
            let mut lockdown = connect_lockdown(provider).await?;
            match lockdown.start_session(&record).await {
                Ok(_) => return Ok((record, lockdown)),
                Err(IdeviceError::InvalidHostID) => {
                    info!("Device {udid} no longer accepts the stored pairing record");
                }
                Err(e) => {
                    return Err(AppError::DeviceComs(
                        "Failed to start lockdown session".into(),
                        chain(&e),
                    ));
                }
            }
        }
        Err(e) => info!("No pairing record for device {udid}: {}", chain(&e)),
    }

    let record = pair(provider, udid).await?;

    let mut lockdown = connect_lockdown(provider).await?;
    lockdown.start_session(&record).await.map_err(|e| {
        AppError::LockdownPairing("Failed to start lockdown session".into(), chain(&e))
    })?;

    Ok((record, lockdown))
}

/// Has the device trust this computer, like the prompt shown when it is first plugged in, and
/// stores the resulting record with usbmuxd so every other app on the computer can use it too.
async fn pair(provider: &UsbmuxdProvider, udid: &str) -> Result<PairingFile, AppError> {
    let system_buid = get_usbmuxd().await?.get_buid().await.map_err(|e| {
        AppError::Usbmuxd("Failed to get system BUID from usbmuxd".into(), chain(&e))
    })?;
    let host_id = uuid::Uuid::new_v4().to_string().to_uppercase();

    let record = loop {
        let mut lockdown = connect_lockdown(provider).await?;
        // Returns once the trust prompt has been answered.
        match lockdown
            .pair(host_id.as_str(), system_buid.as_str(), Some(LABEL))
            .await
        {
            Ok(record) => break record,
            // The prompt only shows once the device is unlocked.
            Err(IdeviceError::PasswordProtected) => tokio::time::sleep(PAIR_RETRY_DELAY).await,
            Err(IdeviceError::UserDeniedPairing) => return Err(AppError::TrustDenied),
            Err(e) => {
                return Err(AppError::LockdownPairing(
                    "Failed to pair with device".into(),
                    chain(&e),
                ));
            }
        }
    };

    let serialized = record.clone().serialize().map_err(|e| {
        AppError::LockdownPairing("Failed to serialize pairing file".into(), chain(&e))
    })?;
    get_usbmuxd()
        .await?
        .save_pair_record(udid, serialized)
        .await
        .map_err(|e| {
            AppError::LockdownPairing("Failed to save pairing record to usbmuxd".into(), chain(&e))
        })?;

    Ok(record)
}

async fn connect_lockdown(provider: &UsbmuxdProvider) -> Result<LockdownClient, AppError> {
    LockdownClient::connect(provider)
        .await
        .map_err(|e| AppError::DeviceComs("Failed to connect to lockdown".into(), chain(&e)))
}

fn lockdown_plist(mut record: PairingFile, udid: &str) -> Result<plist::Dictionary, AppError> {
    record.udid = Some(udid.to_string());

    let serialized = record.serialize().map_err(|e| {
        AppError::LockdownPairing("Failed to serialize pairing file".into(), chain(&e))
    })?;

    plist::from_bytes(&serialized).map_err(|e| {
        AppError::LockdownPairing(
            "Failed to parse pairing file as plist".into(),
            e.to_string(),
        )
    })
}

async fn rppairing_plist(
    app: &AppHandle,
    provider: &UsbmuxdProvider,
    udid: &str,
) -> Result<plist::Dictionary, AppError> {
    let pairings = app.state::<RemotePairings>();
    let mut pairing = pairings
        .lock()
        .unwrap()
        .get(udid)
        .cloned()
        .unwrap_or_else(RemotePairing::new);

    let service = RemotePairingLockdownService::connect(provider)
        .await
        .map_err(|e| {
            AppError::RemotePairing(
                "Failed to connect to the remote pairing service".into(),
                chain(&e),
            )
        })?;
    let mut client = service.into_client(&pairing.host_label).map_err(|e| {
        AppError::RemotePairing(
            "Failed to open the remote pairing channel".into(),
            chain(&e),
        )
    })?;

    // Verifies a pairing made earlier this session; otherwise the device asks to trust a new one.
    client
        .connect(&mut pairing.file, || async { "000000".to_string() })
        .await
        .map_err(|e| AppError::RemotePairing("Failed to pair with device".into(), chain(&e)))?;

    let rppairing_plist = plist::from_bytes(&pairing.file.to_bytes())
        .map_err(|e| AppError::RemotePairing("Invalid RPPairing plist".into(), e.to_string()))?;

    pairings.lock().unwrap().insert(udid.to_string(), pairing);

    Ok(rppairing_plist)
}

/// Writes the pairing file to the Downloads folder, falling back to the app's own data folder
/// when that isn't writable (macOS asks before letting an app into Downloads).
fn save(app: &AppHandle, pairing: Vec<u8>) -> Result<PathBuf, AppError> {
    let last = app.state::<LastExport>();
    let mut last = last.lock().unwrap();

    // Same pairing as last time, so point back at that file instead of writing a copy.
    if let Some((path, bytes)) = last.as_ref()
        && *bytes == pairing
        && path.exists()
    {
        return Ok(path.clone());
    }

    let folders = [app.path().download_dir(), app.path().app_data_dir()];
    let mut failure = "No folder to save the pairing file to".to_string();

    for folder in folders.into_iter().flatten() {
        match write_new(&folder, &pairing) {
            Ok(path) => {
                *last = Some((path.clone(), pairing));
                return Ok(path);
            }
            Err(e) => {
                warn!(
                    "Failed to write pairing file to {}: {}",
                    folder.display(),
                    e
                );
                failure = format!("{}: {}", folder.display(), e);
            }
        }
    }

    Err(AppError::Filesystem(
        "Failed to write pairing file".into(),
        failure,
    ))
}

/// Creates the file under the first free name, so an existing pairing file is never replaced.
fn write_new(folder: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(folder)?;

    let mut copy = 0;
    loop {
        let name = match copy {
            0 => format!("{FILE_STEM}.{FILE_EXTENSION}"),
            n => format!("{FILE_STEM} ({n}).{FILE_EXTENSION}"),
        };
        let path = folder.join(name);

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        // The file lets its holder into the device, so keep it to the current user.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);

        match options.open(&path) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(bytes) {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(e);
                }
                return Ok(path);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => copy += 1,
            Err(e) => return Err(e),
        }
    }
}

fn parse_version_component(segment: Option<&str>) -> u32 {
    segment
        .and_then(|s| {
            let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                None
            } else {
                digits.parse().ok()
            }
        })
        .unwrap_or(0)
}

fn is_ios_version_below(version: &str, target_major: u32, target_minor: u32) -> bool {
    let mut parts = version.split('.');
    let major = parse_version_component(parts.next());
    let minor = parse_version_component(parts.next());
    (major, minor) < (target_major, target_minor)
}
