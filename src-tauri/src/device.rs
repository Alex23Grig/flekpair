use std::{collections::HashMap, sync::Mutex, time::Duration};

use idevice::{
    IdeviceError, IdeviceService,
    lockdown::LockdownClient,
    provider::UsbmuxdProvider,
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection, UsbmuxdDevice},
};
use serde::Serialize;
use tauri::State;
use tracing::debug;

use crate::error::{AppError, chain};

/// How this app identifies itself to the device.
pub const LABEL: &str = "FlekPair";

const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub udid: String,
    /// Empty until the device answers lockdown, e.g. right after it is plugged in.
    pub name: String,
    pub version: String,
    pub device_class: String,
}

/// Devices that have been fully described, so polling doesn't reopen lockdown for them.
pub type DeviceCache = Mutex<HashMap<String, DeviceInfo>>;

/// Lists devices connected over USB. Pairing needs a cable, so wireless ones are left out.
#[tauri::command]
pub async fn list_devices(cache: State<'_, DeviceCache>) -> Result<Vec<DeviceInfo>, AppError> {
    let addr = usbmuxd_addr()?;
    let devices = usb_devices().await?;
    let known = cache.lock().unwrap().clone();

    let infos = futures::future::join_all(devices.iter().map(|device| {
        let cached = known.get(&device.udid).cloned();
        let addr = addr.clone();
        async move {
            match cached {
                Some(info) => info,
                None => describe(device, addr).await,
            }
        }
    }))
    .await;

    *cache.lock().unwrap() = infos
        .iter()
        .filter(|info| !info.name.is_empty() && !info.version.is_empty())
        .map(|info| (info.udid.clone(), info.clone()))
        .collect();

    Ok(infos)
}

async fn describe(device: &UsbmuxdDevice, addr: UsbmuxdAddr) -> DeviceInfo {
    let provider = device.to_provider(addr, LABEL);
    let (name, version, device_class) =
        match tokio::time::timeout(DESCRIBE_TIMEOUT, ask(&provider)).await {
            Ok(Ok(described)) => described,
            Ok(Err(e)) => {
                debug!("No details for {}: {}", device.udid, chain(&e));
                Default::default()
            }
            Err(_) => {
                debug!("Describing {} timed out", device.udid);
                Default::default()
            }
        };

    DeviceInfo {
        udid: device.udid.clone(),
        name,
        version,
        device_class,
    }
}

async fn ask(provider: &UsbmuxdProvider) -> Result<(String, String, String), IdeviceError> {
    let mut lockdown = LockdownClient::connect(provider).await?;
    let name = string_value(&mut lockdown, "DeviceName").await?;
    // A device that doesn't trust this computer yet may keep the rest to itself.
    let version = string_value(&mut lockdown, "ProductVersion")
        .await
        .unwrap_or_default();
    let device_class = string_value(&mut lockdown, "DeviceClass")
        .await
        .unwrap_or_default();

    Ok((name, version, device_class))
}

pub async fn string_value(
    lockdown: &mut LockdownClient,
    key: &str,
) -> Result<String, IdeviceError> {
    lockdown
        .get_value(Some(key), None)
        .await?
        .as_string()
        .map(str::to_owned)
        .ok_or_else(|| IdeviceError::UnexpectedResponse(format!("{key} was not a string")))
}

fn usbmuxd_addr() -> Result<UsbmuxdAddr, AppError> {
    UsbmuxdAddr::from_env_var().map_err(|e| {
        AppError::Usbmuxd(
            "Invalid usbmuxd address from environment".into(),
            e.to_string(),
        )
    })
}

pub async fn get_usbmuxd() -> Result<UsbmuxdConnection, AppError> {
    usbmuxd_addr()?
        .connect(0)
        .await
        .map_err(|e| AppError::Usbmuxd("Failed to connect to usbmuxd".into(), chain(&e)))
}

async fn usb_devices() -> Result<Vec<UsbmuxdDevice>, AppError> {
    let devices =
        get_usbmuxd().await?.get_devices().await.map_err(|e| {
            AppError::Usbmuxd("Failed to list devices from usbmuxd".into(), chain(&e))
        })?;

    Ok(devices
        .into_iter()
        .filter(|device| device.connection_type == Connection::Usb)
        .collect())
}

pub async fn get_provider(udid: &str) -> Result<UsbmuxdProvider, AppError> {
    let device = usb_devices()
        .await?
        .into_iter()
        .find(|device| device.udid == udid)
        .ok_or(AppError::NoDevice)?;

    Ok(device.to_provider(usbmuxd_addr()?, LABEL))
}
