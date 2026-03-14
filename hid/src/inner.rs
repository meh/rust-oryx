use super::{bisync, only_async, only_sync};

#[only_async]
use async_hid::{AsyncHidRead, AsyncHidWrite, DeviceReader, DeviceWriter, HidBackend};
#[only_async]
use futures_lite::StreamExt;

#[only_sync]
use hidapi::{HidApi, HidDevice};

use crate::{
    error::{Error, Result},
    protocol::{self, PACKET_SIZE, RAW_HID_USAGE, RAW_HID_USAGE_PAGE, STOP_BIT, ZSA_VID},
};

/// A decoded event received from the keyboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Firmware serial number bytes (terminated by STOP_BIT in the raw packet).
    FirmwareVersion(Firmware),
    /// Keyboard is awaiting pairing input.
    PairingInput,
    /// Keyboard is awaiting a pairing key.
    PairingKeyInput,
    PairingFailed,
    PairingSuccess,
    /// Active layer changed.
    Layer(u8),
    /// Key at (col, row) was pressed.
    KeyDown {
        col: u8,
        row: u8,
    },
    /// Key at (col, row) was released.
    KeyUp {
        col: u8,
        row: u8,
    },
    /// RGB control mode is now enabled (`true`) or disabled (`false`).
    RgbControl(bool),
    ToggleSmartLayer,
    TriggerSmartLayer,
    /// Status LED control mode is now enabled (`true`) or disabled (`false`).
    StatusLedControl(bool),
    /// Protocol version reported by the firmware.
    ProtocolVersion(u8),
    /// Firmware error.
    FirmwareError(protocol::Error),
    /// Unknown firmware error code (not in the spec).
    UnknownFirmwareError(u8),
    /// Unknown event code.
    Unknown(u8),
}

impl<'a> From<&'a Event> for u8 {
    fn from(value: &'a Event) -> u8 {
        match value {
            Event::FirmwareVersion(_) => protocol::Event::GetFwVersion as u8,
            Event::PairingInput => protocol::Event::PairingInput as u8,
            Event::PairingKeyInput => protocol::Event::PairingKeyInput as u8,
            Event::PairingFailed => protocol::Event::PairingFailed as u8,
            Event::PairingSuccess => protocol::Event::PairingSuccess as u8,
            Event::Layer(_) => protocol::Event::Layer as u8,
            Event::KeyDown { .. } => protocol::Event::KeyDown as u8,
            Event::KeyUp { .. } => protocol::Event::KeyUp as u8,
            Event::RgbControl(_) => protocol::Event::RgbControl as u8,
            Event::ToggleSmartLayer => protocol::Event::ToggleSmartLayer as u8,
            Event::TriggerSmartLayer => protocol::Event::TriggerSmartLayer as u8,
            Event::StatusLedControl(_) => protocol::Event::StatusLedControl as u8,
            Event::ProtocolVersion(_) => protocol::Event::GetProtocolVersion as u8,
            Event::FirmwareError(_) | Event::UnknownFirmwareError(_) => {
                protocol::Event::Error as u8
            }
            Event::Unknown(c) => *c,
        }
    }
}

impl Event {
    pub fn decode(buf: &[u8; PACKET_SIZE]) -> Event {
        match protocol::Event::try_from(buf[0]) {
            Ok(protocol::Event::GetFwVersion) => {
                let data: Vec<u8> = buf[1..]
                    .iter()
                    .copied()
                    .take_while(|&b| b != STOP_BIT)
                    .collect();

                let string = String::from_utf8(data).expect("this is always a string");
                let split = string.split("/").collect::<Vec<_>>();

                Event::FirmwareVersion(Firmware {
                    layout: split[0].into(),
                    revision: split[1].into(),
                })
            }
            Ok(protocol::Event::PairingInput) => Event::PairingInput,
            Ok(protocol::Event::PairingKeyInput) => Event::PairingKeyInput,
            Ok(protocol::Event::PairingFailed) => Event::PairingFailed,
            Ok(protocol::Event::PairingSuccess) => Event::PairingSuccess,
            Ok(protocol::Event::Layer) => Event::Layer(buf[1]),
            Ok(protocol::Event::KeyDown) => Event::KeyDown {
                col: buf[1],
                row: buf[2],
            },
            Ok(protocol::Event::KeyUp) => Event::KeyUp {
                col: buf[1],
                row: buf[2],
            },
            Ok(protocol::Event::RgbControl) => Event::RgbControl(buf[1] != 0),
            Ok(protocol::Event::ToggleSmartLayer) => Event::ToggleSmartLayer,
            Ok(protocol::Event::TriggerSmartLayer) => Event::TriggerSmartLayer,
            Ok(protocol::Event::StatusLedControl) => Event::StatusLedControl(buf[1] != 0),
            Ok(protocol::Event::GetProtocolVersion) => Event::ProtocolVersion(buf[1]),
            Ok(protocol::Event::Error) => match protocol::Error::try_from(buf[1]) {
                Ok(code) => Event::FirmwareError(code),
                Err(byte) => Event::UnknownFirmwareError(byte),
            },
            Err(code) => Event::Unknown(code),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firmware {
    pub layout: String,
    pub revision: String,
}

/// An open connection to a ZSA keyboard (async variant).
#[only_async]
pub struct OryxKeyboard {
    reader: DeviceReader,
    writer: DeviceWriter,
}

/// An open connection to a ZSA keyboard (blocking variant).
#[only_sync]
pub struct OryxKeyboard {
    device: HidDevice,
}

impl OryxKeyboard {
    /// Open the first ZSA keyboard found on the system.
    #[only_async]
    pub async fn open() -> Result<Self> {
        let backend = HidBackend::default();
        let (reader, writer) = backend
            .enumerate()
            .await?
            .find(|d| {
                d.vendor_id == ZSA_VID
                    && d.usage_page == RAW_HID_USAGE_PAGE
                    && d.usage_id == RAW_HID_USAGE
            })
            .await
            .ok_or(Error::NotFound)?
            .open()
            .await?;
        Ok(Self { reader, writer })
    }

    /// Open a ZSA keyboard by product ID.
    #[only_async]
    pub async fn open_by_pid(pid: u16) -> Result<Self> {
        let backend = HidBackend::default();
        let (reader, writer) = backend
            .enumerate()
            .await?
            .find(|d| d.matches(RAW_HID_USAGE_PAGE, RAW_HID_USAGE, ZSA_VID, pid))
            .await
            .ok_or(Error::NotFound)?
            .open()
            .await?;
        Ok(Self { reader, writer })
    }

    /// Open the first ZSA keyboard found on the system.
    #[only_sync]
    pub fn open(api: &HidApi) -> Result<Self> {
        Self::open_by(api, |_| true)
    }

    /// Open a ZSA keyboard by product ID.
    #[only_sync]
    pub fn open_by_pid(api: &HidApi, pid: u16) -> Result<Self> {
        Self::open_by(api, |d| d.product_id() == pid)
    }

    #[only_sync]
    fn open_by(api: &HidApi, pred: impl Fn(&hidapi::DeviceInfo) -> bool) -> Result<Self> {
        let info = api
            .device_list()
            .find(|d| {
                d.vendor_id() == ZSA_VID
                    && d.usage_page() == RAW_HID_USAGE_PAGE
                    && d.usage() == RAW_HID_USAGE
                    && pred(d)
            })
            .ok_or(Error::NotFound)?;
        let device = info.open_device(api)?;
        device.set_blocking_mode(true)?;
        Ok(Self { device })
    }

    /// Request the firmware serial number. Returns the raw serial bytes.
    #[bisync]
    pub async fn firmware(&mut self) -> Result<Firmware> {
        self.send(&[protocol::Command::GetFwVersion as u8]).await?;

        match self.recv_event().await? {
            Event::FirmwareVersion(v) => Ok(v),
            other => Err(Error::UnexpectedEvent {
                expected: protocol::Event::GetFwVersion as u8,
                got: u8::from(&other),
            }),
        }
    }

    /// Request the protocol version from the firmware.
    #[bisync]
    pub async fn protocol(&mut self) -> Result<u8> {
        self.send(&[protocol::Command::GetProtocolVersion as u8])
            .await?;
        match self.recv_event().await? {
            Event::ProtocolVersion(v) => Ok(v),
            other => Err(Error::UnexpectedEvent {
                expected: protocol::Event::GetProtocolVersion as u8,
                got: u8::from(&other),
            }),
        }
    }

    /// Pair with the keyboard. Required before `set_layer`.
    #[only_async]
    pub async fn pair(&mut self) -> Result<()> {
        self.send(&[protocol::Command::PairingInit as u8]).await?;
        match self.recv_event().await? {
            Event::PairingSuccess => {}
            Event::PairingFailed => return Err(Error::PairingFailed),
            other => {
                return Err(Error::UnexpectedEvent {
                    expected: protocol::Event::PairingSuccess as u8,
                    got: u8::from(&other),
                })
            }
        }

        let _ = self.recv_event().await;
        Ok(())
    }

    /// Pair with the keyboard. Required before `set_layer`.
    #[only_sync]
    pub fn pair(&mut self) -> Result<()> {
        self.send(&[protocol::Command::PairingInit as u8])?;
        match self.recv_event()? {
            Event::PairingSuccess => {}
            Event::PairingFailed => return Err(Error::PairingFailed),
            other => {
                return Err(Error::UnexpectedEvent {
                    expected: protocol::Event::PairingSuccess as u8,
                    got: u8::from(&other),
                })
            }
        }

        while let Ok(Some(_)) = self.poll_event(50) {}
        Ok(())
    }

    /// Gracefully disconnect from the keyboard.
    #[bisync]
    pub async fn disconnect(&mut self) -> Result<()> {
        self.send(&[protocol::Command::Disconnect as u8]).await
    }

    /// Move to `layer` (equivalent to `layer_move()` in QMK).
    #[bisync]
    pub async fn set_layer(&mut self, layer: u8) -> Result<()> {
        self.send(&[protocol::Command::SetLayer as u8, 1, layer])
            .await
    }

    /// Deactivate `layer` (equivalent to `layer_off()` in QMK).
    #[bisync]
    pub async fn unset_layer(&mut self, layer: u8) -> Result<()> {
        self.send(&[protocol::Command::SetLayer as u8, 0, layer])
            .await
    }

    /// Enable or disable webhid RGB control mode.
    /// Returns the new state confirmed by the firmware.
    #[bisync]
    pub async fn rgb_control(&mut self, enable: bool) -> Result<bool> {
        self.send(&[protocol::Command::RgbControl as u8, enable as u8])
            .await?;
        match self.recv_event().await? {
            Event::RgbControl(state) => Ok(state),
            other => Err(Error::UnexpectedEvent {
                expected: protocol::Event::RgbControl as u8,
                got: u8::from(&other),
            }),
        }
    }

    /// Set the colour of a single RGB LED by its matrix index.
    #[bisync]
    pub async fn set_rgb(&mut self, index: u8, r: u8, g: u8, b: u8) -> Result<()> {
        self.send(&[protocol::Command::SetRgbLed as u8, index, r, g, b])
            .await
    }

    /// Set all RGB LEDs to the same colour in one packet.
    #[bisync]
    pub async fn set_rgb_all(&mut self, r: u8, g: u8, b: u8) -> Result<()> {
        self.send(&[protocol::Command::SetRgbLedAll as u8, r, g, b])
            .await
    }

    /// Increase (`true`) or decrease (`false`) the RGB matrix brightness.
    #[bisync]
    pub async fn update_brightness(&mut self, increase: bool) -> Result<()> {
        self.send(&[protocol::Command::UpdateBrightness as u8, increase as u8])
            .await
    }

    /// Turn an individual status LED on or off. `led` is 0-indexed (0–5).
    #[bisync]
    pub async fn set_status_led(&mut self, led: u8, on: bool) -> Result<()> {
        self.send(&[protocol::Command::SetStatusLed as u8, led, on as u8])
            .await
    }

    /// Enable or disable firmware control of all status LEDs.
    /// Returns the new state confirmed by the firmware.
    #[bisync]
    pub async fn set_status_led_control(&mut self, enable: bool) -> Result<bool> {
        self.send(&[protocol::Command::StatusLedControl as u8, enable as u8])
            .await?;
        match self.recv_event().await? {
            Event::StatusLedControl(state) => Ok(state),
            other => Err(Error::UnexpectedEvent {
                expected: protocol::Event::StatusLedControl as u8,
                got: u8::from(&other),
            }),
        }
    }

    /// Wait for the next event and decode it.
    #[bisync]
    pub async fn recv_event(&mut self) -> Result<Event> {
        let buf = self.recv().await?;
        Ok(Event::decode(&buf))
    }

    /// Poll for an event with a timeout (blocking only). Returns `None` on timeout.
    #[only_sync]
    pub fn poll_event(&self, timeout_ms: i32) -> Result<Option<Event>> {
        let mut buf = [0u8; PACKET_SIZE];
        let n = self.device.read_timeout(&mut buf, timeout_ms)?;
        if n == 0 {
            return Ok(None);
        }
        Ok(Some(Event::decode(&buf)))
    }

    #[only_async]
    async fn send(&mut self, payload: &[u8]) -> Result<()> {
        let mut buf = [0u8; PACKET_SIZE + 1];
        let len = payload.len().min(PACKET_SIZE);
        buf[1..=len].copy_from_slice(&payload[..len]);
        self.writer.write_output_report(&buf).await?;
        Ok(())
    }

    #[only_async]
    async fn recv(&mut self) -> Result<[u8; PACKET_SIZE]> {
        let mut buf = [0u8; PACKET_SIZE];
        let n = self.reader.read_input_report(&mut buf).await?;
        if n == 0 {
            return Err(Error::Timeout);
        }
        Ok(buf)
    }

    #[only_sync]
    fn send(&self, payload: &[u8]) -> Result<()> {
        let mut buf = [0u8; PACKET_SIZE + 1];
        let len = payload.len().min(PACKET_SIZE);
        buf[1..=len].copy_from_slice(&payload[..len]);
        self.device.write(&buf)?;
        Ok(())
    }

    #[only_sync]
    fn recv(&self) -> Result<[u8; PACKET_SIZE]> {
        let mut buf = [0u8; PACKET_SIZE];
        let n = self.device.read_timeout(&mut buf, 1000)?;
        if n == 0 {
            return Err(Error::Timeout);
        }
        Ok(buf)
    }
}
