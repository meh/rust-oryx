use std::collections::HashSet;
use std::io;

use crossterm::{
    event::{Event as CEvent, EventStream, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_lite::StreamExt as _;
use oryx_hid::{asynchronous::OryxKeyboard, layout};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout as TLayout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Tabs},
    Frame, Terminal,
};
use tokio::sync::mpsc;

const DARK_RED: Color = Color::Rgb(160, 20, 20);

// ── Key label helpers ─────────────────────────────────────────────────────────

fn mode_label(m: &layout::Mode) -> String {
    // Transparent / no-op – checked before any stripping
    match m.code.as_str() {
        "KC_TRANSPARENT" | "KC_TRNS" => return "    ".into(),
        "KC_NO" | "XXXXXXX" => return "    ".into(),
        _ => {}
    }

    // Layer-indexed codes (bare names; layer stored in a separate field)
    let layer = m.layer.unwrap_or(0);
    match m.code.as_str() {
        "MO" => return format!("MO{:<2}", layer),
        "LT" => return format!("LT{:<2}", layer),
        "TG" => return format!("TG{:<2}", layer),
        "TO" => return format!("TO{:<2}", layer),
        "TT" => return format!("TT{:<2}", layer),
        "DF" => return format!("DF{:<2}", layer),
        "OSL" => return format!("OSL{}", layer),
        _ => {}
    }

    // Strip the leading 2-3 char all-uppercase prefix (KC_, UK_, DE_, FR_, QK_, RGB_, …).
    // This makes the table below country/locale-independent: KC_MINS, UK_MINS, DE_MINS
    // all strip to MINS and match the same arm.
    let code = m.code.as_str();
    let bare = code
        .find('_')
        .filter(|&i| matches!(i, 2 | 3) && code[..i].bytes().all(|b| b.is_ascii_uppercase()))
        .map_or(code, |i| &code[i + 1..]);
    // Strip mod-tap _T suffix (LSFT_T → LSFT, LGUI_T → LGUI, …)
    let bare = bare.strip_suffix("_T").unwrap_or(bare);

    let label: &str = match bare {
        // ── Whitespace / editing ───────────────────────────────────────────────
        "SPACE" | "SPC" => "SPC",
        "ENTER" | "ENT" | "KP_ENTER" => "ENT",
        "BSPC" | "BSPACE" | "BACKSPACE" => "BSP",
        "DELETE" | "DEL" => "DEL",
        "ESCAPE" | "ESC" | "GESC" | "GRAVE_ESCAPE" => "ESC",
        "TAB" => "TAB",
        "CAPS" | "CAPS_LOCK" | "CLCK" => "CAP",
        "INSERT" | "INS" => "INS",
        // ── Modifiers ─────────────────────────────────────────────────────────
        "LSFT" | "RSFT" | "LSHIFT" | "RSHIFT" | "LEFT_SHIFT" | "RIGHT_SHIFT" | "LSPO" | "RSPC" => {
            "SFT"
        }
        "LCTL" | "RCTL" | "LCTRL" | "RCTRL" | "LEFT_CTRL" | "RIGHT_CTRL" => "CTL",
        "LALT" | "RALT" | "LEFT_ALT" | "RIGHT_ALT" | "ALGR" | "RALTGR" => "ALT",
        "LGUI" | "RGUI" | "LEFT_GUI" | "RIGHT_GUI" | "LWIN" | "RWIN" | "LCMD" | "RCMD" => "GUI",
        "MEH" => "MEH",
        "HYPR" => "HYP",
        // ── Navigation ────────────────────────────────────────────────────────
        "LEFT" => "◄",
        "RIGHT" => "►",
        "UP" => "▲",
        "DOWN" => "▼",
        "HOME" => "HOM",
        "END" => "END",
        "PAGE_UP" | "PGUP" => "PGU",
        "PAGE_DOWN" | "PGDN" | "PGDOWN" => "PGD",
        "PRINT_SCREEN" | "PSCR" | "PRSC" => "PSC",
        "SCROLL_LOCK" | "SLCK" | "SCRL" => "SCL",
        "PAUSE" | "PAUS" | "BRK" | "PAUSE_BREAK" => "PAU",
        "NUM_LOCK" | "NLCK" | "NUMLOCK" => "NUM",
        // ── Punctuation – suffix shared across locales ────────────────────────
        "MINUS" | "MINS" => "-",
        "EQUAL" | "EQL" => "=",
        "LEFT_BRACKET" | "LBRC" | "LBRACKET" => "[",
        "RIGHT_BRACKET" | "RBRC" | "RBRACKET" => "]",
        "BACKSLASH" | "BSLS" | "NONUS_BACKSLASH" | "NUBS" => "\\",
        "SEMICOLON" | "SCLN" | "SCOLON" => ";",
        "QUOTE" | "QUOT" => "'",
        "DQUO" | "DOUBLE_QUOTE" => "\"",
        "GRAVE" | "GRV" => "`",
        "COMMA" | "COMM" => ",",
        "DOT" => ".",
        "SLASH" | "SLSH" => "/",
        // ── Shifted symbols ────────────────────────────────────────────────────
        "TILD" | "TILDE" => "~",
        "EXLM" | "EXCLAIM" => "!",
        "AT" => "@",
        "HASH" | "NUHS" => "#",
        "DOLLAR" | "DLR" => "$",
        "PERCENT" | "PERC" => "%",
        "CIRC" | "CIRCUMFLEX" => "^",
        "AMPR" | "AMPERSAND" => "&",
        "ASTR" | "ASTERISK" => "*",
        "LPRN" | "LEFT_PAREN" => "(",
        "RPRN" | "RIGHT_PAREN" => ")",
        "UNDS" | "UNDERSCORE" => "_",
        "PLUS" => "+",
        "LCBR" | "LEFT_CURLY_BRACE" => "{",
        "RCBR" | "RIGHT_CURLY_BRACE" => "}",
        "PIPE" | "PIPE2" => "|",
        "COLN" | "COLON" => ":",
        "LABK" | "LESS_THAN" => "<",
        "RABK" | "GREATER_THAN" => ">",
        "QUES" | "QUESTION" => "?",
        // ── Keypad ────────────────────────────────────────────────────────────
        "KP_0" => "K0",
        "KP_1" => "K1",
        "KP_2" => "K2",
        "KP_3" => "K3",
        "KP_4" => "K4",
        "KP_5" => "K5",
        "KP_6" => "K6",
        "KP_7" => "K7",
        "KP_8" => "K8",
        "KP_9" => "K9",
        "KP_DOT" | "KP_COMMA" => "K.",
        "KP_SLASH" => "K/",
        "KP_MINUS" => "K-",
        "KP_PLUS" => "K+",
        "KP_ASTERISK" => "K*",
        "KP_EQUAL" => "K=",
        // ── Media / system ────────────────────────────────────────────────────
        "MUTE" | "KB_MUTE" | "AUDIO_MUTE" => "MUT",
        "VOLU" | "KB_VOLUME_UP" | "AUDIO_VOL_UP" => "VOL+",
        "VOLD" | "KB_VOLUME_DOWN" | "AUDIO_VOL_DOWN" => "VOL-",
        "MPLY" | "MEDIA_PLAY_PAUSE" => "PLY",
        "MNXT" | "MEDIA_NEXT_TRACK" => "NXT",
        "MPRV" | "MEDIA_PREV_TRACK" => "PRV",
        "MSTP" | "MEDIA_STOP" => "STP",
        "MRWD" | "MEDIA_REWIND" => "RWD",
        "MFFD" | "MEDIA_FAST_FORWARD" => "FFD",
        "EJCT" | "MEDIA_EJECT" => "EJT",
        "BRIU" | "BRIGHTNESS_UP" => "BRI+",
        "BRID" | "BRIGHTNESS_DOWN" => "BRI-",
        "PWR" | "SYSTEM_POWER" => "PWR",
        "SLEP" | "SYSTEM_SLEEP" => "ZZZ",
        "WAKE" | "SYSTEM_WAKE" => "WKE",
        // ── RGB underglow (RGB_ prefix stripped → bare suffixes) ──────────────
        "UNDERGLOW_TOGGLE" | "TOGG" => "RGB",
        "UNDERGLOW_MODE_NEXT" | "MOD" => "RGB+",
        "UNDERGLOW_MODE_PREVIOUS" | "RMOD" => "RGB-",
        "UNDERGLOW_HUE_UP" | "HUI" => "HUE+",
        "UNDERGLOW_HUE_DOWN" | "HUD" => "HUE-",
        "UNDERGLOW_SATURATION_UP" | "SAI" => "SAT+",
        "UNDERGLOW_SATURATION_DOWN" | "SAD" => "SAT-",
        "UNDERGLOW_VALUE_UP" | "VAI" => "VAL+",
        "UNDERGLOW_VALUE_DOWN" | "VAD" => "VAL-",
        "UNDERGLOW_SPEED_UP" | "SPI" => "SPD+",
        "UNDERGLOW_SPEED_DOWN" | "SPD" => "SPD-",
        // ── Backlight (BL_ prefix stripped) ──────────────────────────────────
        "BACKLIGHT_TOGGLE" | "BL_TOGG" => "BL",
        "BACKLIGHT_UP" | "INC" | "BL_INC" => "BL+",
        "BACKLIGHT_DOWN" | "DEC" | "BL_DEC" => "BL-",
        "BACKLIGHT_STEP" | "STEP" | "BL_STEP" => "BL+",
        // ── QMK specials ──────────────────────────────────────────────────────
        "BOOTLOADER" | "BOOT" => "BOOT",
        "RESET" | "RST" => "RST",
        "CLEAR_EEPROM" => "EECL",
        "DEBUG_TOGGLE" => "DBG",
        "CAPS_WORD_TOGGLE" => "CWRD",
        "LAYER_LOCK" => "LLCK",
        "LOCK" => "LOCK",
        "REPEAT_KEY" | "REP" => "REP",
        "ALT_REPEAT_KEY" | "AREP" => "AREP",
        // ── Dynamic macros ────────────────────────────────────────────────────
        "DYNAMIC_MACRO_RECORD_START_1" => "DR1",
        "DYNAMIC_MACRO_RECORD_START_2" => "DR2",
        "DYNAMIC_MACRO_PLAY_1" => "DP1",
        "DYNAMIC_MACRO_PLAY_2" => "DP2",
        "DYNAMIC_MACRO_RECORD_STOP" => "DRS",
        // ── Fall-through: first 4 chars of the bare suffix (lowercased) ───────
        _ => {
            let lower = bare.to_lowercase();
            let chars: String = lower.chars().take(4).collect();
            return format!("{:^4}", chars);
        }
    };
    format!("{:^4}", label)
}

fn is_transparent(key: &layout::Key) -> bool {
    key.custom_label.is_none()
        && key.hold.is_none()
        && key
            .tap
            .as_ref()
            .map_or(true, |m| matches!(m.code.as_str(), "KC_TRANSPARENT" | "KC_TRNS"))
}

fn key_label(key: &layout::Key, base_key: Option<&layout::Key>) -> String {
    if is_transparent(key) {
        return base_key.map_or_else(|| "    ".into(), |bk| key_label(bk, None));
    }
    if let Some(ref s) = key.custom_label {
        let s: String = s.chars().take(4).collect();
        return format!("{:^4}", s);
    }
    if let Some(ref m) = key.tap {
        let label = mode_label(m);
        if label.trim() != "" {
            return label;
        }
    }
    if let Some(ref m) = key.hold {
        let label = mode_label(m);
        if label.trim() != "" {
            return format!("[{:^2}]", label.trim().chars().take(2).collect::<String>());
        }
    }
    "    ".into()
}

fn parse_hex_color(s: &str) -> Option<Color> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

fn key_style(key: &layout::Key, base_key: Option<&layout::Key>, pressed: bool) -> Style {
    if pressed {
        return Style::default()
            .fg(Color::Black)
            .bg(DARK_RED)
            .add_modifier(Modifier::BOLD);
    }
    let effective = if is_transparent(key) {
        base_key.unwrap_or(key)
    } else {
        key
    };
    let fg = if is_transparent(effective) {
        Color::DarkGray
    } else {
        Color::White
    };
    let style = Style::default().fg(fg);
    match effective.glow_color.as_deref().and_then(parse_hex_color) {
        Some(bg) => style.bg(bg),
        None => style,
    }
}

// ── Matrix → key index ────────────────────────────────────────────────────────
//
// Best-guess mapping for Moonlander MK1. Rows 0-5 = left half, 6-11 = right half.
// Row 3: 6 keys (col 6 = missing/tower key on left, col 0 missing on right).
// Row 4: 5 keys. Row 5/11: thumb cluster (4 keys).
// Adjust if keys don't match physical positions.

fn matrix_to_index(row: u8, col: u8) -> Option<usize> {
    match (row, col) {
        (r @ 0..=2, c @ 0..=6) => Some(r as usize * 7 + c as usize),
        (3, c @ 0..=5) => Some(21 + c as usize),
        (4, c @ 0..=4) => Some(27 + c as usize),
        (5, c @ 0..=2) => Some(33 + c as usize), // left thumb: col0=outer, col1=mid, col2=inner
        (5, 3) => Some(32),                        // left thumb: col3=wide key
        (r @ 6..=8, c @ 0..=6) => Some(36 + (r as usize - 6) * 7 + c as usize),
        (9, c @ 1..=6) => Some(56 + c as usize),   // right 1×6: col0 missing (tower), col1-6 → 57-62
        (10, c @ 2..=6) => Some(61 + c as usize),  // right 1×5: col0-1 missing, col2-6 → 63-67
        (11, 3) => Some(68),                         // right thumb: col3=wide key (inner)
        (11, c @ 4..=6) => Some(65 + c as usize),  // right thumb: col4-6=small → 69,70,71
        _ => None,
    }
}

// ── Keyboard layout rendering ─────────────────────────────────────────────────
//
// Moonlander key indices (72 total per side, 36 per half):
//   Left  – 3×7: [0-6],[7-13],[14-20]  6-key: [21-26]  5-key: [27-31]  thumb: [32-35]
//   Right – 3×7: [36-42],[43-49],[50-56]  6-key: [57-62]  5-key: [63-67]  thumb: [68-71]
//
// Horizontal char positions (key cell = 5 chars, row of n = 5n+1):
//   Row of 7 = 36 chars, row of 6 = 31 chars, row of 5 = 26 chars, thumb = 16 chars
//   Main (7+7):  left [0..35]   gap=7   right [43..78]  → 79 chars
//   Row3 (6+6):  left [0..30]   gap=17  right [48..78]  → 79 chars
//   Row4 (5+5):  left [0..25]   gap=27  right [53..78]  → 79 chars
//   Thumb:       left [20..35]  gap=7   right [43..58]  → 59 chars

fn border_row(n: usize, l: char, m: char, r: char) -> String {
    let mut s = String::with_capacity(5 * n + 1);
    s.push(l);
    for i in 0..n {
        for _ in 0..4 {
            s.push('─');
        }
        s.push(if i + 1 < n { m } else { r });
    }
    s
}

fn row_top(n: usize) -> String {
    border_row(n, '┌', '┬', '┐')
}
fn row_mid(n: usize) -> String {
    border_row(n, '├', '┼', '┤')
}
fn row_bot(n: usize) -> String {
    border_row(n, '└', '┴', '┘')
}

fn content_line(
    all_keys: &[layout::Key],
    base_keys: Option<&[layout::Key]>,
    indices: &[usize],
    pressed: &HashSet<usize>,
) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw("│")];
    for &i in indices {
        let k = &all_keys[i];
        let bk = base_keys.and_then(|bks| bks.get(i));
        spans.push(Span::styled(
            key_label(k, bk),
            key_style(k, bk, pressed.contains(&i)),
        ));
        spans.push(Span::raw("│"));
    }
    spans
}

fn render_keyboard(
    keys: &[layout::Key],
    base_keys: Option<&[layout::Key]>,
    pressed: &HashSet<usize>,
) -> Vec<Line<'static>> {
    if keys.len() < 72 {
        return vec![Line::from("Not enough keys in layout")];
    }

    // Gap constants (see coordinate derivation in header comment above)
    const MAIN_GAP: &str = "       "; //  7 chars
    const ROW3_GAP: &str = "                 "; // 17 chars
    const ROW4_GAP: &str = "                           "; // 27 chars
    const THM_L_PAD: &str = "                    "; // 20 chars
    const THM_M_GAP: &str = "       "; //  7 chars

    let left_main: [&[usize]; 3] = [
        &[0, 1, 2, 3, 4, 5, 6],
        &[7, 8, 9, 10, 11, 12, 13],
        &[14, 15, 16, 17, 18, 19, 20],
    ];
    let right_main: [&[usize]; 3] = [
        &[36, 37, 38, 39, 40, 41, 42],
        &[43, 44, 45, 46, 47, 48, 49],
        &[50, 51, 52, 53, 54, 55, 56],
    ];

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Merge main 3×7 rows into one grid
    for row in 0..3 {
        let lb = if row == 0 { row_top(7) } else { row_mid(7) };
        let rb = if row == 0 { row_top(7) } else { row_mid(7) };
        lines.push(Line::from(format!("{}{}{}", lb, MAIN_GAP, rb)));
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.extend(content_line(keys, base_keys, left_main[row], pressed));
        spans.push(Span::raw(MAIN_GAP));
        spans.extend(content_line(keys, base_keys, right_main[row], pressed));
        lines.push(Line::from(spans));
    }
    // Transition: 3×7 bottom / 1×6 top merged
    // Left cols 0-5 shared → ├/┼; col 6 ends → ┘. Right col 0 ends → └; cols 1-6 shared → ┼/┤
    lines.push(Line::from(format!(
        "{}{}{}",
        border_row(7, '├', '┼', '┘'),
        MAIN_GAP,
        border_row(7, '└', '┼', '┤')
    )));
    {
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.extend(content_line(keys, base_keys, &[21, 22, 23, 24, 25, 26], pressed));
        spans.push(Span::raw(ROW3_GAP));
        spans.extend(content_line(keys, base_keys, &[57, 58, 59, 60, 61, 62], pressed));
        lines.push(Line::from(spans));
    }
    // Transition: 1×6 bottom / 1×5 top merged
    // Left cols 0-4 shared → ├/┼; col 5 ends → ┘. Right col 0 ends → └; cols 1-5 shared → ┼/┤
    lines.push(Line::from(format!(
        "{}{}{}",
        border_row(6, '├', '┼', '┘'),
        ROW3_GAP,
        border_row(6, '└', '┼', '┤')
    )));
    {
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.extend(content_line(keys, base_keys, &[27, 28, 29, 30, 31], pressed));
        spans.push(Span::raw(ROW4_GAP));
        spans.extend(content_line(keys, base_keys, &[63, 64, 65, 66, 67], pressed));
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(format!(
        "{}{}{}",
        row_bot(5),
        ROW4_GAP,
        row_bot(5)
    )));

    // Thumb clusters: wide key (2 cells) above the inner 2 small keys; 1 small key sticks out
    // on the outer side. Total thumb width = 3 keys = 16 chars.
    //
    // Left (32=wide above 34,35; 33=outer):   Right (68=wide above 69,70; 71=outer):
    //      ┌─────────┐                         ┌─────────┐
    // ┌────┼────┬────┤                         ├────┬────┼────┐
    // │    │    │    │                         │    │    │    │
    // └────┴────┴────┘                         └────┴────┴────┘
    let wide_inner = 9usize; // 5*2 - 1
    let wide_inner_s = "─".repeat(wide_inner);
    // top: 5 spaces + wide border = 16 chars (left); wide border + 5 spaces = 16 chars (right)
    let l_wide_top = format!("     ┌{}┐", wide_inner_s);
    let r_wide_top = format!("┌{}┐     ", wide_inner_s);
    // transition lines: combines outer-small top corner with wide bottom
    let l_wide_mid = "┌────┼────┬────┤"; // outer small ┌, cross at wide-left ┼, small mid ┬, cap ┤
    let r_wide_mid = "├────┬────┼────┐"; // cap ├, small mid ┬, cross at wide-right ┼, outer small ┐
    let small_bot = row_bot(3);

    // top of wide keys
    lines.push(Line::from(format!(
        "{}{}{}{}",
        THM_L_PAD, l_wide_top, THM_M_GAP, r_wide_top
    )));
    // content of wide keys (label centered in wide_inner chars)
    {
        let wl = 32usize;
        let wr = 68usize;
        let wl_bk = base_keys.and_then(|bks| bks.get(wl));
        let wr_bk = base_keys.and_then(|bks| bks.get(wr));
        let l_label = format!("{:^9}", key_label(&keys[wl], wl_bk).trim());
        let r_label = format!("{:^9}", key_label(&keys[wr], wr_bk).trim());
        lines.push(Line::from(vec![
            Span::raw(THM_L_PAD),
            Span::raw("     │"),
            Span::styled(l_label, key_style(&keys[wl], wl_bk, pressed.contains(&wl))),
            Span::raw("│"),
            Span::raw(THM_M_GAP),
            Span::raw("│"),
            Span::styled(r_label, key_style(&keys[wr], wr_bk, pressed.contains(&wr))),
            Span::raw("│     "),
        ]));
    }
    // transition: wide bottom merged with small keys top
    lines.push(Line::from(format!(
        "{}{}{}{}",
        THM_L_PAD, l_wide_mid, THM_M_GAP, r_wide_mid
    )));
    // small keys content: left [33,34,35], right [69,70,71]
    {
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::raw(THM_L_PAD));
        spans.extend(content_line(keys, base_keys, &[33, 34, 35], pressed));
        spans.push(Span::raw(THM_M_GAP));
        spans.extend(content_line(keys, base_keys, &[69, 70, 71], pressed));
        lines.push(Line::from(spans));
    }
    // bottom of small keys
    lines.push(Line::from(format!(
        "{}{}{}{}",
        THM_L_PAD, small_bot, THM_M_GAP, small_bot
    )));

    lines
}

// ── App state ─────────────────────────────────────────────────────────────────

enum Msg {
    SetActiveLayer(usize),
    KeyDown(usize),
    KeyUp(usize),
    LayoutLoaded(Box<layout::Response>),
    KbConnected,
    KbDisconnected,
    StatusUpdate(String),
}

struct App {
    layout_data: Option<layout::Response>,
    view_layer: usize,
    active_layer: usize,
    pressed: HashSet<usize>,
    status: String,
    keyboard_connected: bool,
}

impl App {
    fn new() -> Self {
        Self {
            layout_data: None,
            view_layer: 0,
            active_layer: 0,
            pressed: HashSet::new(),
            status: "Connecting to keyboard…".into(),
            keyboard_connected: false,
        }
    }

    fn layer_count(&self) -> usize {
        self.layout_data
            .as_ref()
            .and_then(|r| r.data.layout.revision.as_ref())
            .map(|rev| rev.layers.len())
            .unwrap_or(0)
    }

    fn apply(&mut self, msg: Msg) {
        match msg {
            Msg::SetActiveLayer(l) => {
                self.active_layer = l;
                self.view_layer = l;
            }
            Msg::KeyDown(i) => {
                self.pressed.insert(i);
            }
            Msg::KeyUp(i) => {
                self.pressed.remove(&i);
            }
            Msg::LayoutLoaded(r) => {
                self.layout_data = Some(*r);
                self.status = String::new();
            }
            Msg::KbConnected => {
                self.keyboard_connected = true;
                self.status = "Fetching layout…".into();
            }
            Msg::KbDisconnected => {
                self.keyboard_connected = false;
                self.pressed.clear();
                self.status = "Disconnected".into();
            }
            Msg::StatusUpdate(s) => {
                self.status = s;
            }
        }
    }
}

fn draw(f: &mut Frame, app: &App) {
    let chunks = TLayout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(f.area());

    let accent = Style::default().fg(DARK_RED);
    let highlight = Style::default()
        .fg(Color::Black)
        .bg(DARK_RED)
        .add_modifier(Modifier::BOLD);

    if let Some(ref response) = app.layout_data {
        if let Some(ref rev) = response.data.layout.revision {
            let tabs: Vec<Line> = rev
                .layers
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let name = l.title.as_deref().unwrap_or("?");
                    if i == app.active_layer {
                        Line::from(Span::styled(
                            format!(" ● {} ", name),
                            Style::default().fg(DARK_RED).add_modifier(Modifier::BOLD),
                        ))
                    } else {
                        Line::from(format!("  {}  ", name))
                    }
                })
                .collect();

            f.render_widget(
                Tabs::new(tabs)
                    .block(Block::default().borders(Borders::ALL).title(format!(
                        " {} – {} ",
                        response.data.layout.title.as_deref().unwrap_or("Layout"),
                        rev.model
                    )))
                    .select(app.view_layer)
                    .style(accent)
                    .highlight_style(highlight)
                    .divider(Span::styled("|", Style::default().fg(Color::DarkGray))),
                chunks[0],
            );

            let base_keys = if app.view_layer == 0 {
                None
            } else {
                rev.layers.first().map(|l| l.keys.as_slice())
            };
            f.render_widget(
                Paragraph::new(render_keyboard(
                    &rev.layers[app.view_layer].keys,
                    base_keys,
                    &app.pressed,
                )),
                chunks[1],
            );
            return;
        }
    }

    // Loading / error state
    f.render_widget(
        Paragraph::new(app.status.as_str())
            .block(Block::default().borders(Borders::ALL).title(" oryx_hid "))
            .style(Style::default().fg(DARK_RED)),
        chunks[1],
    );
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();

    tokio::spawn({
        let tx = tx.clone();
        async move {
            let mut kb = match OryxKeyboard::open().await {
                Ok(k) => k,
                Err(_) => {
                    let _ = tx.send(Msg::StatusUpdate("No ZSA keyboard found".into()));
                    return;
                }
            };
            let _ = tx.send(Msg::KbConnected);

            let firmware = match kb.firmware().await {
                Ok(f) => f,
                Err(e) => {
                    let _ = tx.send(Msg::StatusUpdate(format!("Firmware error: {e}")));
                    return;
                }
            };

            let _ = tx.send(Msg::StatusUpdate(format!(
                "Fetching {}/{}…",
                firmware.layout, firmware.revision
            )));

            match layout::fetch(&firmware.layout, &firmware.revision, "moonlander").await {
                Ok(response) => {
                    let _ = tx.send(Msg::LayoutLoaded(Box::new(response)));
                }
                Err(e) => {
                    let _ = tx.send(Msg::StatusUpdate(format!("Fetch failed: {e}")));
                }
            }

            if kb.pair().await.is_err() {
                let _ = tx.send(Msg::StatusUpdate("Pairing failed".into()));
                return;
            }

            loop {
                match kb.recv_event().await {
                    Ok(oryx_hid::asynchronous::Event::Layer(l)) => {
                        let _ = tx.send(Msg::SetActiveLayer(l as usize));
                    }
                    Ok(oryx_hid::asynchronous::Event::KeyDown { col, row }) => {
                        if let Some(idx) = matrix_to_index(row, col) {
                            let _ = tx.send(Msg::KeyDown(idx));
                        }
                    }
                    Ok(oryx_hid::asynchronous::Event::KeyUp { col, row }) => {
                        if let Some(idx) = matrix_to_index(row, col) {
                            let _ = tx.send(Msg::KeyUp(idx));
                        }
                    }
                    Ok(_) => {}
                    Err(_) => {
                        let _ = tx.send(Msg::KbDisconnected);
                        break;
                    }
                }
            }
        }
    });

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut app = App::new();
    let mut events = EventStream::new();

    let result = async {
        loop {
            terminal.draw(|f| draw(f, &app))?;
            tokio::select! {
                Some(msg) = rx.recv() => {
                    app.apply(msg);
                }
                Some(Ok(CEvent::Key(key))) = events.next() => {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        break;
                    }
                }
            }
        }
        anyhow::Ok(())
    }
    .await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    result
}
