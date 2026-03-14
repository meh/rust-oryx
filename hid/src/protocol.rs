/// ZSA Technology Labs USB Vendor ID.
pub const ZSA_VID: u16 = 0x3297;

/// Raw HID usage page (vendor-specific).
pub const RAW_HID_USAGE_PAGE: u16 = 0xFF60;

/// Raw HID usage ID.
pub const RAW_HID_USAGE: u16 = 0x0061;

/// HID packet size in bytes.
pub const PACKET_SIZE: usize = 32;

/// Protocol version this crate implements.
pub const PROTOCOL_VERSION: u8 = 0x04;

/// Byte value used to terminate variable-length fields in response packets.
pub const STOP_BIT: u8 = 0xFE;

/// Commands sent from host to keyboard (byte 0 of every outgoing packet).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    GetFwVersion = 0x00,
    PairingInit = 0x01,
    /// Legacy – accepted by firmware but has no effect.
    PairingValidate = 0x02,
    Disconnect = 0x03,
    SetLayer = 0x04,
    RgbControl = 0x05,
    SetRgbLed = 0x06,
    SetStatusLed = 0x07,
    UpdateBrightness = 0x08,
    SetRgbLedAll = 0x09,
    StatusLedControl = 0x0A,
    /// Also happens to share the byte value of STOP_BIT in response data.
    GetProtocolVersion = 0xFE,
}

/// Event codes returned by the keyboard (byte 0 of every incoming packet).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    GetFwVersion = 0x00,
    PairingInput = 0x01,
    PairingKeyInput = 0x02,
    PairingFailed = 0x03,
    PairingSuccess = 0x04,
    Layer = 0x05,
    KeyDown = 0x06,
    KeyUp = 0x07,
    RgbControl = 0x08,
    ToggleSmartLayer = 0x09,
    TriggerSmartLayer = 0x0A,
    StatusLedControl = 0x0B,
    GetProtocolVersion = 0xFE,
    Error = 0xFF,
}

impl TryFrom<u8> for Event {
    type Error = u8;

    fn try_from(v: u8) -> Result<Self, u8> {
        match v {
            0x00 => Ok(Self::GetFwVersion),
            0x01 => Ok(Self::PairingInput),
            0x02 => Ok(Self::PairingKeyInput),
            0x03 => Ok(Self::PairingFailed),
            0x04 => Ok(Self::PairingSuccess),
            0x05 => Ok(Self::Layer),
            0x06 => Ok(Self::KeyDown),
            0x07 => Ok(Self::KeyUp),
            0x08 => Ok(Self::RgbControl),
            0x09 => Ok(Self::ToggleSmartLayer),
            0x0A => Ok(Self::TriggerSmartLayer),
            0x0B => Ok(Self::StatusLedControl),
            0xFE => Ok(Self::GetProtocolVersion),
            0xFF => Ok(Self::Error),
            other => Err(other),
        }
    }
}

/// Error codes carried in [`Event::Error`] packets.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    PairingInitFailed = 0x00,
    PairingInputFailed = 0x01,
    PairingKeyInputFailed = 0x02,
    PairingFailed = 0x03,
    RgbMatrixNotEnabled = 0x04,
    StatusLedOutOfRange = 0x05,
    UnknownCommand = 0xFF,
}

impl TryFrom<u8> for Error {
    type Error = u8;

    fn try_from(v: u8) -> Result<Self, u8> {
        match v {
            0x00 => Ok(Self::PairingInitFailed),
            0x01 => Ok(Self::PairingInputFailed),
            0x02 => Ok(Self::PairingKeyInputFailed),
            0x03 => Ok(Self::PairingFailed),
            0x04 => Ok(Self::RgbMatrixNotEnabled),
            0x05 => Ok(Self::StatusLedOutOfRange),
            0xFF => Ok(Self::UnknownCommand),
            other => Err(other),
        }
    }
}
