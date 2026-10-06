use serde::Serialize;
use serde::ser::{SerializeStruct, Serializer};

#[derive(Debug, thiserror::Error, Clone)]
pub enum AppError {
    #[error("{0}: {1}")]
    Usbmuxd(String, String),
    #[error("Device is no longer connected")]
    NoDevice,
    #[error("{0}: {1}")]
    DeviceComs(String, String),
    #[error("{0}: {1}")]
    LockdownPairing(String, String),
    #[error("{0}: {1}")]
    RemotePairing(String, String),
    #[error("The trust prompt was declined on the device")]
    TrustDenied,
    #[error("{0} canceled")]
    Canceled(String),
    #[error("{0}: {1}")]
    Filesystem(String, String),
    #[error("{0}: {1}")]
    Driver(String, String),
}

impl AppError {
    /// Stable identifier the frontend uses to pick troubleshooting suggestions.
    fn kind(&self) -> &'static str {
        match self {
            AppError::Usbmuxd(..) => "usbmuxd",
            AppError::NoDevice => "no_device",
            AppError::DeviceComs(..) => "device_coms",
            AppError::LockdownPairing(..) => "lockdown_pairing",
            AppError::RemotePairing(..) => "remote_pairing",
            AppError::TrustDenied => "trust_denied",
            AppError::Canceled(_) => "canceled",
            AppError::Filesystem(..) => "filesystem",
            AppError::Driver(..) => "driver",
        }
    }
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("AppError", 2)?;
        state.serialize_field("type", self.kind())?;
        state.serialize_field("message", &self.to_string())?;
        state.end()
    }
}

/// Flattens an error and its `source()` chain into one line, since most idevice errors only
/// describe the failing layer and leave the cause in the chain.
pub fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text = format!("{text}: {cause}");
        source = cause.source();
    }
    text
}
