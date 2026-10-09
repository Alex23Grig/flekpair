use std::{collections::HashMap, sync::Mutex, time::Duration};

use idevice::{
    IdeviceError, IdeviceService,
    lockdown::LockdownClient,
    provider::{IdeviceProvider, UsbmuxdProvider},
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection, UsbmuxdDevice},
};
use serde::Serialize;
use tauri::State;
use tracing::debug;

use crate::error::{AppError, chain};

/// How this app identifies itself to the device.
pub const LABEL: &str = "FlekPair";

const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(4);

/// macOS shows a device over Wi-Fi once it has trusted the computer by cable and Wi-Fi syncing
/// is on. Windows can too, but nothing here has been tried there.
const WIRELESS: bool = cfg!(target_os = "macos");

/// How a device is reached.
#[derive(Serialize, Clone, Copy, PartialEq, Default, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Link {
    #[default]
    Usb,
    Network,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub udid: String,
    /// Empty until the device answers lockdown, e.g. right after it is plugged in.
    pub name: String,
    pub version: String,
    pub device_class: String,
    pub link: Link,
}

/// Devices that have been fully described, so polling doesn't reopen lockdown for them.
pub type DeviceCache = Mutex<HashMap<String, DeviceInfo>>;

/// Lists devices on a cable and, on macOS, ones seen over Wi-Fi.
#[tauri::command]
pub async fn list_devices(cache: State<'_, DeviceCache>) -> Result<Vec<DeviceInfo>, AppError> {
    let addr = usbmuxd_addr()?;
    let devices = devices().await?;
    let known = cache.lock().unwrap().clone();

    let infos = futures::future::join_all(devices.iter().map(|(device, link)| {
        let cached = known.get(&device.udid).cloned();
        let addr = addr.clone();
        async move {
            match cached {
                // The details hold whichever way the device is reached now.
                Some(info) => DeviceInfo {
                    link: *link,
                    ..info
                },
                None => describe(device, *link, addr).await,
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

async fn describe(device: &UsbmuxdDevice, link: Link, addr: UsbmuxdAddr) -> DeviceInfo {
    let provider = device.to_provider(addr, LABEL);
    let (name, version, device_class) =
        match tokio::time::timeout(DESCRIBE_TIMEOUT, ask(&provider, link)).await {
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
        link,
    }
}

async fn ask(
    provider: &UsbmuxdProvider,
    link: Link,
) -> Result<(String, String, String), IdeviceError> {
    let mut lockdown = LockdownClient::connect(provider).await?;
    // Over Wi-Fi lockdown only talks inside a session, which the record from the earlier cable
    // pairing opens.
    if link == Link::Network {
        lockdown
            .start_session(&provider.get_pairing_file().await?)
            .await?;
    }
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

async fn devices() -> Result<Vec<(UsbmuxdDevice, Link)>, AppError> {
    let devices =
        get_usbmuxd().await?.get_devices().await.map_err(|e| {
            AppError::Usbmuxd("Failed to list devices from usbmuxd".into(), chain(&e))
        })?;

    Ok(reachable(devices, WIRELESS))
}

/// One entry per device. usbmuxd lists a device once for each way it sees it; the cable is
/// preferred because pairing a device for the first time only works over it.
fn reachable(devices: Vec<UsbmuxdDevice>, wireless: bool) -> Vec<(UsbmuxdDevice, Link)> {
    let mut chosen: Vec<(UsbmuxdDevice, Link)> = Vec::new();
    for device in devices {
        let link = match device.connection_type {
            Connection::Usb => Link::Usb,
            Connection::Network(_) if wireless => Link::Network,
            _ => continue,
        };
        match chosen
            .iter_mut()
            .find(|(known, _)| known.udid == device.udid)
        {
            Some(entry) if link == Link::Usb => *entry = (device, link),
            Some(_) => {}
            None => chosen.push((device, link)),
        }
    }
    chosen
}

pub async fn get_provider(udid: &str) -> Result<(UsbmuxdProvider, Link), AppError> {
    let (device, link) = devices()
        .await?
        .into_iter()
        .find(|(device, _)| device.udid == udid)
        .ok_or(AppError::NoDevice)?;

    Ok((device.to_provider(usbmuxd_addr()?, LABEL), link))
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    fn device(udid: &str, device_id: u32, connection_type: Connection) -> UsbmuxdDevice {
        UsbmuxdDevice {
            connection_type,
            udid: udid.into(),
            device_id,
        }
    }

    fn summary(devices: Vec<(UsbmuxdDevice, Link)>) -> Vec<(String, u32, Link)> {
        devices
            .into_iter()
            .map(|(device, link)| (device.udid, device.device_id, link))
            .collect()
    }

    #[test]
    fn lists_each_device_once_and_prefers_the_cable() {
        let wifi = || Connection::Network(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 20)));
        let seen = || {
            vec![
                device("phone", 1, wifi()),
                device("tablet", 2, wifi()),
                device("phone", 3, Connection::Usb),
                device("watch", 4, Connection::Unknown("Bluetooth".into())),
                device("old", 5, Connection::Usb),
                device("old", 6, wifi()),
            ]
        };

        assert_eq!(
            summary(reachable(seen(), true)),
            [
                ("phone".to_string(), 3, Link::Usb),
                ("tablet".to_string(), 2, Link::Network),
                ("old".to_string(), 5, Link::Usb),
            ]
        );
        assert_eq!(
            summary(reachable(seen(), false)),
            [
                ("phone".to_string(), 3, Link::Usb),
                ("old".to_string(), 5, Link::Usb),
            ]
        );
    }
}
