use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use hidapi::HidApi;
use oryx_hid::blocking::OryxKeyboard;

#[derive(Parser)]
#[command(about = "Control a ZSA keyboard via the Oryx HID protocol")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print firmware version (layout/revision).
    Firmware,
    /// Print protocol version.
    Protocol,
    /// Layer control.
    #[command(subcommand)]
    Layer(LayerCmd),
    /// RGB LED control.
    #[command(subcommand)]
    Rgb(RgbCmd),
    /// Adjust RGB matrix brightness.
    #[command(subcommand)]
    Brightness(BrightnessCmd),
    /// Status LED control.
    #[command(subcommand)]
    Led(LedCmd),
}

#[derive(Subcommand)]
enum LayerCmd {
    /// Activate a layer (layer_move).
    Set {
        /// Layer index to activate.
        layer: u8,
    },
    /// Deactivate a layer (layer_off).
    Unset {
        /// Layer index to deactivate.
        layer: u8,
    },
}

#[derive(Subcommand)]
enum RgbCmd {
    /// Enable or disable webhid RGB control mode.
    Control { state: Toggle },
    /// Set a single LED by matrix index to a colour.
    Set {
        /// LED matrix index.
        index: u8,
        /// Colour as #RRGGBB hex.
        #[arg(value_parser = parse_color)]
        color: (u8, u8, u8),
    },
    /// Set all LEDs to one colour.
    All {
        /// Colour as #RRGGBB hex.
        #[arg(value_parser = parse_color)]
        color: (u8, u8, u8),
    },
}

#[derive(Subcommand)]
enum BrightnessCmd {
    /// Increase RGB matrix brightness.
    Up,
    /// Decrease RGB matrix brightness.
    Down,
}

#[derive(Subcommand)]
enum LedCmd {
    /// Enable or disable firmware control of all status LEDs.
    Control { state: Toggle },
    /// Turn a single status LED on or off. Index is 0-based (0–5).
    Set { index: u8, state: Toggle },
}

#[derive(Clone, ValueEnum)]
enum Toggle {
    On,
    Off,
}

impl Toggle {
    fn as_bool(&self) -> bool {
        matches!(self, Toggle::On)
    }
}

fn parse_color(s: &str) -> Result<(u8, u8, u8), String> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return Err(format!("expected #RRGGBB hex colour, got {:?}", s));
    }
    let r = u8::from_str_radix(&s[0..2], 16).map_err(|e| e.to_string())?;
    let g = u8::from_str_radix(&s[2..4], 16).map_err(|e| e.to_string())?;
    let b = u8::from_str_radix(&s[4..6], 16).map_err(|e| e.to_string())?;
    Ok((r, g, b))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let api = HidApi::new().context("failed to initialise HID API")?;
    let mut kb = OryxKeyboard::open(&api).context("no ZSA keyboard found")?;
    kb.pair().context("pairing failed")?;

    match cli.cmd {
        Cmd::Firmware => {
            let fw = kb.firmware()?;
            println!("layout:   {}", fw.layout);
            println!("revision: {}", fw.revision);
        }

        Cmd::Protocol => {
            let v = kb.protocol()?;
            println!("protocol version: {v:#04x}");
        }

        Cmd::Layer(LayerCmd::Set { layer }) => {
            kb.layer(layer)?;
            println!("layer {layer} activated");
        }

        Cmd::Layer(LayerCmd::Unset { layer }) => {
            kb.unset_layer(layer)?;
            println!("layer {layer} deactivated");
        }

        Cmd::Rgb(RgbCmd::Control { state }) => {
            let new = kb.rgb_control(state.as_bool())?;
            println!("RGB control: {}", if new { "on" } else { "off" });
        }

        Cmd::Rgb(RgbCmd::Set {
            index,
            color: (r, g, b),
        }) => {
            kb.rgb(index, r, g, b)?;
            println!("LED {index} set to #{r:02x}{g:02x}{b:02x}");
        }

        Cmd::Rgb(RgbCmd::All { color: (r, g, b) }) => {
            kb.rgb_all(r, g, b)?;
            println!("all LEDs set to #{r:02x}{g:02x}{b:02x}");
        }

        Cmd::Brightness(BrightnessCmd::Up) => {
            kb.brightness(true)?;
            println!("brightness increased");
        }

        Cmd::Brightness(BrightnessCmd::Down) => {
            kb.brightness(false)?;
            println!("brightness decreased");
        }

        Cmd::Led(LedCmd::Control { state }) => {
            let new = kb.led_control(state.as_bool())?;
            println!("status LED control: {}", if new { "on" } else { "off" });
        }

        Cmd::Led(LedCmd::Set { index, state }) => {
            if index > 5 {
                bail!("status LED index must be 0–5, got {index}");
            }
            kb.led(index, state.as_bool())?;
            println!(
                "status LED {index}: {}",
                if state.as_bool() { "on" } else { "off" }
            );
        }
    }

    Ok(())
}
