//! Getting a pairing file to the device by way of FlekStore's server, where the FlekDeck app
//! collects it with a tap on Refresh.
//!
//! The file goes up as it is, under the device's UDID, and the server hands it to whoever asks
//! for that UDID. A UDID is no secret and the file is a key to the device, so this trades safety
//! for having nothing to scan or type. Saving the file to this computer instead uploads nothing.

use std::time::Duration;

use serde::Deserialize;
use tauri::{AppHandle, State};

use crate::{
    device::KeepAwake,
    error::{AppError, chain},
    pairing::{PairingCancelToken, pairing_file, until_canceled},
};

/// Where pairing files are left for a device to collect, each under its UDID. The address is
/// given to the build in `FLEKPAIR_SERVER` rather than written here. A build without it uploads
/// nothing and only saves the file.
const SERVER: Option<&str> = option_env!("FLEKPAIR_SERVER");

const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether this build has somewhere to upload pairing files to.
#[tauri::command]
pub fn can_send() -> bool {
    given(SERVER).is_some()
}

/// The server a build was given, unless that was nothing.
fn given(server: Option<&'static str>) -> Option<&'static str> {
    server
        .map(|server| server.trim().trim_end_matches('/'))
        .filter(|server| !server.is_empty())
}

/// Pairs with the device and uploads its pairing file for FlekDeck to collect.
#[tauri::command]
pub async fn send_pairing_file(
    app: AppHandle,
    cancel_state: State<'_, PairingCancelToken>,
    awake: State<'_, KeepAwake>,
    udid: String,
) -> Result<(), AppError> {
    let server = given(SERVER).ok_or_else(|| failed("this build has nowhere to upload to"))?;
    awake.extend();

    let sent = until_canceled(&cancel_state, async {
        let file = pairing_file(&app, &udid).await?;
        upload(server, &device(&file)?, file).await
    })
    .await;
    // Answering the trust prompt can take a while, and the device should still be there after.
    awake.extend();

    sent
}

fn failed(detail: impl Into<String>) -> AppError {
    AppError::Upload("Failed to upload the pairing file".into(), detail.into())
}

/// The device a pairing file is for, which the file itself says.
fn device(file: &[u8]) -> Result<String, AppError> {
    let udid = plist::from_bytes::<plist::Dictionary>(file)
        .ok()
        .and_then(|file| file.get("UDID")?.as_string().map(str::to_owned))
        .ok_or_else(|| failed("the pairing file doesn't name its device"))?;

    // It becomes part of an address.
    if udid.is_empty() || !udid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(failed("the pairing file names its device oddly"));
    }
    Ok(udid)
}

/// What the server says when it turns an upload down.
#[derive(Deserialize)]
struct Refusal {
    message: String,
}

async fn upload(server: &str, udid: &str, file: Vec<u8>) -> Result<(), AppError> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("FlekPair/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .timeout(UPLOAD_TIMEOUT)
        .build()
        .map_err(|e| failed(chain(&e)))?;

    let response = client
        .post(format!("{server}/{udid}"))
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .body(file)
        .send()
        .await
        .map_err(|e| failed(chain(&e)))?;

    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let reason = response
        .json::<Refusal>()
        .await
        .map(|refusal| format!(": {}", refusal.message))
        .unwrap_or_default();
    Err(failed(format!("the server answered {status}{reason}")))
}

#[cfg(test)]
mod tests {
    use plist_macro::plist_to_xml_bytes;

    use super::*;

    #[test]
    fn uploads_only_where_the_build_was_told_to() {
        assert_eq!(given(None), None);
        assert_eq!(given(Some("")), None);
        assert_eq!(given(Some("  ")), None);
        assert_eq!(
            given(Some("https://example.com/files/")),
            Some("https://example.com/files")
        );
    }

    #[test]
    fn reads_the_device_from_the_file() {
        let mut file = plist::Dictionary::new();
        file.insert("UDID".into(), "00008110-000A1C2E3F90801E".into());
        assert_eq!(
            device(&plist_to_xml_bytes(&file)).unwrap(),
            "00008110-000A1C2E3F90801E"
        );

        assert!(device(b"not a pairing file").is_err());
        assert!(device(&plist_to_xml_bytes(&plist::Dictionary::new())).is_err());

        let mut odd = plist::Dictionary::new();
        odd.insert("UDID".into(), "../somewhere else".into());
        assert!(device(&plist_to_xml_bytes(&odd)).is_err());
    }
}
