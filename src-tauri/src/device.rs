use std::{
    collections::HashMap,
    net::Ipv4Addr,
    sync::Mutex,
    time::{Duration, Instant},
};

use idevice::{
    IdeviceError, IdeviceService,
    lockdown::LockdownClient,
    pairing_file::PairingFile,
    provider::{IdeviceProvider, TcpProvider},
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection, UsbmuxdDevice},
};
use serde::Serialize;
use tauri::State;
use tracing::debug;

use crate::{
    error::{AppError, chain},
    nearby::Nearby,
};

/// How this app identifies itself to the device.
pub const LABEL: &str = "FlekPair";

const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(4);

/// macOS shows a device over Wi-Fi once it has trusted the computer by cable and Wi-Fi syncing
/// is on. Apple's service on Windows does too when Bonjour is installed, which comes with
/// iTunes and not with the driver alone.
const WIRELESS: bool = cfg!(any(target_os = "macos", windows));

/// How often a device held on Wi-Fi is asked something, so its session counts as in use.
const KEEP_AWAKE_EVERY: Duration = Duration::from_secs(4);

/// How long devices on Wi-Fi are kept awake after the app was last used. Past that it has most
/// likely been left open, and a phone shouldn't lose battery to it.
const KEEP_AWAKE_FOR: Duration = Duration::from_secs(10 * 60);

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

/// A device and the way to it.
pub struct Reachable {
    pub udid: String,
    pub link: Link,
    route: Route,
}

enum Route {
    /// Through usbmuxd, which lists the device.
    Mux(UsbmuxdDevice),
    /// Straight to where the device answered from on the network, with the record it trusts.
    /// See [`crate::nearby`].
    Direct(Ipv4Addr, Box<PairingFile>),
}

/// Tells one way to a device from another. A session doesn't outlast the way it was opened.
#[derive(Clone, Copy, PartialEq)]
enum Path {
    /// usbmuxd gives a device a new id each time it comes back.
    Mux(u32),
    Direct(Ipv4Addr),
}

impl Reachable {
    pub fn provider(&self) -> Result<Box<dyn IdeviceProvider>, AppError> {
        Ok(match &self.route {
            Route::Mux(device) => Box::new(device.to_provider(usbmuxd_addr()?, LABEL)),
            Route::Direct(address, record) => Box::new(TcpProvider {
                addr: (*address).into(),
                scope_id: None,
                pairing_file: (**record).clone(),
                label: LABEL.into(),
            }),
        })
    }

    fn path(&self) -> Path {
        match &self.route {
            Route::Mux(device) => Path::Mux(device.device_id),
            Route::Direct(address, _) => Path::Direct(*address),
        }
    }
}

/// Lockdown sessions kept open to devices on Wi-Fi.
///
/// Left alone, a phone drops off the network within a minute and comes back seconds or minutes
/// later, so it would come and go in the list and an export could find it gone. It stays for as
/// long as a session with it is in use.
pub struct KeepAwake {
    until: Mutex<Instant>,
    sessions: Mutex<HashMap<String, Session>>,
}

struct Session {
    path: Path,
    /// `None` while the device isn't answering.
    lockdown: Option<LockdownClient>,
    /// When to use the session next, or to try opening it again.
    due: Instant,
}

impl Default for KeepAwake {
    fn default() -> Self {
        Self {
            until: Mutex::new(Instant::now() + KEEP_AWAKE_FOR),
            sessions: Mutex::default(),
        }
    }
}

impl KeepAwake {
    /// Called when the app is used: brought to the front, or asked to export.
    pub fn extend(&self) {
        *self.until.lock().unwrap() = Instant::now() + KEEP_AWAKE_FOR;
    }

    fn wanted_at(&self, time: Instant) -> bool {
        time < *self.until.lock().unwrap()
    }
}

/// Lists devices on a cable and ones on the same network that trust this computer.
#[tauri::command]
pub async fn list_devices(
    cache: State<'_, DeviceCache>,
    awake: State<'_, KeepAwake>,
    nearby: State<'_, Nearby>,
) -> Result<Vec<DeviceInfo>, AppError> {
    let devices = devices(&nearby).await?;
    let known = cache.lock().unwrap().clone();
    let keep_awake = awake.wanted_at(Instant::now());
    // Sessions left in here are with devices that are no longer listed, and end with this call.
    let mut sessions = std::mem::take(&mut *awake.sessions.lock().unwrap());

    let looked = futures::future::join_all(devices.iter().map(|device| {
        let cached = known.get(&device.udid).cloned();
        let session = sessions.remove(&device.udid);
        async move {
            if device.link == Link::Network && keep_awake {
                let (info, session) = held(device, cached, session).await;
                (info, Some(session))
            } else {
                let info = match cached {
                    // The details hold whichever way the device is reached now.
                    Some(info) => DeviceInfo {
                        link: device.link,
                        ..info
                    },
                    None => describe(device).await.0,
                };
                (info, None)
            }
        }
    }))
    .await;

    for (info, session) in &looked {
        if session.as_ref().is_some_and(|held| held.lockdown.is_some()) {
            nearby.still_there(&info.udid);
        }
    }

    let (infos, sessions): (Vec<_>, Vec<_>) = looked.into_iter().unzip();
    *awake.sessions.lock().unwrap() = infos
        .iter()
        .zip(sessions)
        .filter_map(|(info, session)| Some((info.udid.clone(), session?)))
        .collect();

    *cache.lock().unwrap() = infos
        .iter()
        .filter(|info| !info.name.is_empty() && !info.version.is_empty())
        .map(|info| (info.udid.clone(), info.clone()))
        .collect();

    Ok(infos)
}

/// What is known about a device on Wi-Fi, and the session that keeps it there.
async fn held(
    device: &Reachable,
    cached: Option<DeviceInfo>,
    session: Option<Session>,
) -> (DeviceInfo, Session) {
    let mut session = session
        // A device that came back is reached another way, and connections made the old way
        // are dead.
        .filter(|session| session.path == device.path())
        .unwrap_or_else(|| Session {
            path: device.path(),
            lockdown: None,
            due: Instant::now(),
        });
    let known = cached.map(|info| DeviceInfo {
        link: Link::Network,
        ..info
    });
    let unnamed = || DeviceInfo {
        udid: device.udid.clone(),
        link: Link::Network,
        ..Default::default()
    };

    if Instant::now() < session.due {
        return (known.unwrap_or_else(unnamed), session);
    }

    let info = match session.lockdown.as_mut() {
        Some(lockdown) => {
            let asked =
                tokio::time::timeout(DESCRIBE_TIMEOUT, string_value(lockdown, "DeviceName")).await;
            if !matches!(asked, Ok(Ok(_))) {
                debug!("{} stopped answering over Wi-Fi", device.udid);
                session.lockdown = None;
            }
            known
        }
        None => {
            let (info, lockdown) = describe(device).await;
            session.lockdown = lockdown;
            // A device that isn't answering keeps the details it gave earlier.
            match session.lockdown {
                Some(_) => Some(info),
                None => known.or(Some(info)),
            }
        }
    };
    session.due = Instant::now() + KEEP_AWAKE_EVERY;

    (info.unwrap_or_else(unnamed), session)
}

/// The device's details, and the lockdown connection they were read over if it answered.
async fn describe(device: &Reachable) -> (DeviceInfo, Option<LockdownClient>) {
    let asked = async {
        let provider = device.provider().map_err(|e| e.to_string())?;
        tokio::time::timeout(DESCRIBE_TIMEOUT, ask(&*provider, device.link))
            .await
            .map_err(|_| "timed out".to_string())?
            .map_err(|e| chain(&e))
    };
    let (lockdown, (name, version, device_class)) = match asked.await {
        Ok((lockdown, described)) => (Some(lockdown), described),
        Err(e) => {
            debug!("No details for {}: {e}", device.udid);
            Default::default()
        }
    };

    let info = DeviceInfo {
        udid: device.udid.clone(),
        name,
        version,
        device_class,
        link: device.link,
    };
    (info, lockdown)
}

async fn ask(
    provider: &dyn IdeviceProvider,
    link: Link,
) -> Result<(LockdownClient, (String, String, String)), IdeviceError> {
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

    Ok((lockdown, (name, version, device_class)))
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

async fn devices(nearby: &Nearby) -> Result<Vec<Reachable>, AppError> {
    let listed =
        get_usbmuxd().await?.get_devices().await.map_err(|e| {
            AppError::Usbmuxd("Failed to list devices from usbmuxd".into(), chain(&e))
        })?;
    nearby.learn(listed.iter().map(|device| device.udid.as_str()));

    let mut devices: Vec<Reachable> = reachable(listed, WIRELESS)
        .into_iter()
        .map(|(device, link)| Reachable {
            udid: device.udid.clone(),
            link,
            route: Route::Mux(device),
        })
        .collect();
    // What usbmuxd lists comes first, and so does a cable.
    for (udid, address, record) in nearby.found() {
        if !devices.iter().any(|device| device.udid == udid) {
            devices.push(Reachable {
                udid,
                link: Link::Network,
                route: Route::Direct(address, Box::new(record)),
            });
        }
    }
    Ok(devices)
}

/// One entry per device, cabled ones first. usbmuxd lists a device once for each way it sees
/// it; the cable is preferred because pairing a device for the first time only works over it.
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
    // The app starts out with the first one selected.
    chosen.sort_by_key(|(_, link)| *link != Link::Usb);
    chosen
}

pub async fn find(nearby: &Nearby, udid: &str) -> Result<Reachable, AppError> {
    devices(nearby)
        .await?
        .into_iter()
        .find(|device| device.udid == udid)
        .ok_or(AppError::NoDevice)
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
    fn lists_each_device_once_with_cabled_ones_first() {
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
                ("old".to_string(), 5, Link::Usb),
                ("tablet".to_string(), 2, Link::Network),
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

    /// Needs a device that trusts this computer, awake and on the same network, and its UDID in
    /// `FLEKPAIR_UDID`.
    #[test]
    #[ignore]
    fn reaches_a_trusted_device_on_the_network_directly() {
        let udid = std::env::var("FLEKPAIR_UDID").expect("FLEKPAIR_UDID names the device");

        tauri::async_runtime::block_on(async {
            let nearby = Nearby::default();
            nearby.learn([udid.as_str()]);
            // A phone that nothing is talking to comes and goes; give it a couple of minutes.
            let mut found = None;
            for _ in 0..40 {
                nearby.look().await;
                found = nearby
                    .found()
                    .into_iter()
                    .find(|(found, ..)| *found == udid);
                if found.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            let (udid, address, record) = found.expect("the device didn't answer");
            let device = Reachable {
                udid,
                link: Link::Network,
                route: Route::Direct(address, Box::new(record)),
            };

            let (info, session) = held(&device, None, None).await;
            assert!(
                session.lockdown.is_some(),
                "no session straight to the device"
            );
            assert!(!info.name.is_empty() && !info.version.is_empty());
        });
    }

    #[test]
    fn keeps_devices_awake_only_for_a_while_after_the_app_was_used() {
        let awake = KeepAwake::default();
        let opened = Instant::now();
        let later = opened + KEEP_AWAKE_FOR + Duration::from_secs(1);

        assert!(awake.wanted_at(opened));
        assert!(!awake.wanted_at(later));

        awake.extend();
        assert!(awake.wanted_at(later - Duration::from_secs(2)));
        assert!(!awake.wanted_at(later + KEEP_AWAKE_FOR));
    }
}
