use crate::protocol;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("HID error: {0}")]
    Hid(Box<dyn std::error::Error + Send + Sync>),

    #[error("no ZSA keyboard found (check udev rules / permissions)")]
    NotFound,

    #[error("read timed out")]
    Timeout,

    #[error("pairing failed")]
    PairingFailed,

    #[error("keyboard returned error: {0:?}")]
    Firmware(protocol::Error),

    #[error("keyboard returned unknown firmware error code: 0x{0:02X}")]
    FirmwareUnknown(u8),

    #[error("unexpected event byte 0x{got:02X} (expected 0x{expected:02X})")]
    UnexpectedEvent { expected: u8, got: u8 },
}

impl From<async_hid::HidError> for Error {
    fn from(e: async_hid::HidError) -> Self {
        Error::Hid(Box::new(e))
    }
}

impl From<hidapi::HidError> for Error {
    fn from(e: hidapi::HidError) -> Self {
        Error::Hid(Box::new(e))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
