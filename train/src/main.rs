use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use clap::{Parser, Subcommand};
use crossterm::{
    event::{Event as CEvent, EventStream, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_lite::StreamExt as _;
use llm::{
    builder::{LLMBackend, LLMBuilder},
    chat::ChatMessage,
};
use rand::Rng;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

// ── Language configs ──────────────────────────────────────────────────────────

struct LangConfig {
    name: &'static str,
    system_prompt: &'static str,
}

const LANGUAGES: &[LangConfig] = &[
    LangConfig {
        name: "English",
        system_prompt: "You are a typing practice text generator. Generate clean, readable English prose suitable for typing practice. Use only ASCII characters and common punctuation. Output only the typing practice text with no preamble.",
    },
    LangConfig {
        name: "Spanish",
        system_prompt: "Eres un generador de texto para práctica de escritura al teclado. Genera texto natural en español incluyendo caracteres con tilde: á é í ó ú ü ñ ¡ ¿ y puntuación española. Emite únicamente el texto de práctica sin introducción.",
    },
    LangConfig {
        name: "French",
        system_prompt: "Vous êtes un générateur de texte pour la pratique de la frappe clavier. Générez un texte naturel en français incluant les caractères accentués: é è ê ë à â ç ù û ô î ï æ œ. Produisez uniquement le texte de pratique sans introduction.",
    },
    LangConfig {
        name: "German",
        system_prompt: "Sie sind ein Generator für Tipp-Übungstexte. Erstellen Sie natürlichen deutschen Text einschließlich Umlaute und Sonderzeichen: ä ö ü ß Ä Ö Ü und korrekte deutsche Interpunktion. Geben Sie nur den Übungstext ohne Einleitung aus.",
    },
    LangConfig {
        name: "Portuguese",
        system_prompt: "Você é um gerador de texto para prática de digitação. Gere texto natural em português incluindo caracteres acentuados: ã â á à ç é ê í ó ô õ ú e pontuação portuguesa. Produza apenas o texto de prática sem introdução.",
    },
    LangConfig {
        name: "Italian",
        system_prompt: "Sei un generatore di testo per la pratica della digitazione. Genera testo naturale in italiano includendo i caratteri accentati: à è é ì í î ò ó ù ú e la corretta punteggiatura italiana. Produce solo il testo di pratica senza introduzione.",
    },
];

// ── Code language configs ─────────────────────────────────────────────────────

struct CodeLangConfig {
    name: &'static str,
}

const CODE_LANGS: &[CodeLangConfig] = &[
    CodeLangConfig { name: "Rust" },
    CodeLangConfig { name: "Python" },
    CodeLangConfig { name: "JavaScript" },
    CodeLangConfig { name: "Go" },
    CodeLangConfig { name: "C" },
    CodeLangConfig { name: "Bash" },
];

// ── Training modes ────────────────────────────────────────────────────────────

const NGRAM_KINDS: &[&str] = &["bigrams", "trigrams", "words"];

const SYMBOLS: &str = "!@#$%^&*()_+-=[]{}|;':\",./<>?`~";

// ── Config types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Config {
    #[serde(default)]
    llm: LlmConfig,
    #[serde(default)]
    train: TrainConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LlmConfig {
    provider: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ModeCache {
    last_mode: Option<String>,
    lang: Option<usize>,
    word_count: Option<usize>,
    prose_infinite: Option<bool>,
    prose_prompt: Option<String>,
    words_prompt: Option<String>,
    symbol_length: Option<usize>,
    ngram_kind: Option<usize>,
    ngram_count: Option<usize>,
    code_lang: Option<usize>,
    code_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TrainConfig {
    #[serde(default)]
    prompts: Vec<String>,
    stats_file: Option<String>,
    accent_color: Option<String>,
    highlight_color: Option<String>,
    stats_window: Option<usize>,
    #[serde(default)]
    mode_cache: ModeCache,
}

fn parse_rgb(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim().trim_start_matches('#');
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some((r, g, b))
    } else {
        None
    }
}

fn readable_fg(r: u8, g: u8, b: u8) -> Color {
    let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    if lum > 128.0 {
        Color::Black
    } else {
        Color::White
    }
}

fn load_config(path: &PathBuf) -> Config {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(path: &PathBuf, config: &Config) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, toml::to_string_pretty(config)?)?;
    Ok(())
}

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Subcommand)]
enum Commands {
    /// Delete the recorded stats file.
    ClearStats,
}

#[derive(Parser)]
#[command(about = "Typing trainer powered by LLM")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    #[arg(long, env = "ORYX_TRAIN_PROVIDER")]
    provider: Option<String>,
    #[arg(long, env = "ORYX_TRAIN_MODEL")]
    model: Option<String>,
    #[arg(long, env = "ORYX_TRAIN_API_KEY")]
    api_key: Option<String>,
    #[arg(long, env = "ORYX_TRAIN_BASE_URL")]
    base_url: Option<String>,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long = "prompt", value_name = "PROMPT")]
    extra_prompts: Vec<String>,
    #[arg(long, env = "ORYX_TRAIN_STATS_FILE")]
    stats_file: Option<PathBuf>,
    /// Accent color for active pane borders/titles, as a hex color. E.g. "00c8ff".
    #[arg(long, env = "ORYX_TRAIN_ACCENT_COLOR")]
    accent_color: Option<String>,
    /// Keyboard LED color for the next key to press, as a hex color. Defaults to accent color.
    #[arg(long, env = "ORYX_TRAIN_HIGHLIGHT_COLOR")]
    highlight_color: Option<String>,
    /// Number of recent sessions to average in the stats bar.
    #[arg(long, env = "ORYX_TRAIN_STATS_WINDOW")]
    stats_window: Option<usize>,
}

// ── Provider / model helpers ──────────────────────────────────────────────────

fn parse_backend(s: &str) -> Result<LLMBackend> {
    match s.to_lowercase().as_str() {
        "openai" => Ok(LLMBackend::OpenAI),
        "anthropic" | "claude" => Ok(LLMBackend::Anthropic),
        "ollama" => Ok(LLMBackend::Ollama),
        "groq" => Ok(LLMBackend::Groq),
        "deepseek" => Ok(LLMBackend::DeepSeek),
        "xai" | "x.ai" => Ok(LLMBackend::XAI),
        "google" | "gemini" => Ok(LLMBackend::Google),
        "mistral" => Ok(LLMBackend::Mistral),
        "openrouter" => Ok(LLMBackend::OpenRouter),
        "cohere" => Ok(LLMBackend::Cohere),
        other => Err(anyhow::anyhow!(
            "Unknown provider: '{other}'. Supported: openai, anthropic, ollama, groq, deepseek, google, mistral"
        )),
    }
}

fn default_model(provider: &str) -> String {
    match provider.to_lowercase().as_str() {
        "anthropic" | "claude" => "claude-3-5-haiku-20241022".into(),
        "ollama" => "llama3.2".into(),
        "groq" => "llama3-8b-8192".into(),
        "google" | "gemini" => "gemini-1.5-flash".into(),
        "mistral" => "mistral-small-latest".into(),
        "deepseek" => "deepseek-chat".into(),
        _ => "gpt-4o-mini".into(),
    }
}

// ── Stats ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
struct SessionStats {
    timestamp_unix: u64,
    mode: String,
    duration_secs: f64,
    wpm: f64,
    accuracy: f64,
    errors: usize,
    correct: usize,
    text_length: usize,
    prompt: String,
    #[serde(default)]
    avg_reaction_ms: Option<f64>,
    /// Flash mode: average reaction time in ms per symbol key.
    #[serde(default)]
    reaction_by_symbol: Option<HashMap<String, f64>>,
}

// ── Per-mode stats path ───────────────────────────────────────────────────────

fn stats_path(base: &PathBuf, mode: Mode) -> PathBuf {
    let id = match mode {
        Mode::Prose => "prose",
        Mode::Words => "words",
        Mode::Flash => "flash",
        Mode::Symbols => "symbols",
        Mode::Ngrams => "ngrams",
        Mode::Code => "code",
    };
    let stem = base.file_stem().unwrap_or_default().to_string_lossy();
    let ext = base.extension().unwrap_or_default().to_string_lossy();
    let name = format!("{stem}.{id}.{ext}");
    base.parent().unwrap_or_else(|| Path::new(".")).join(name)
}

fn save_stats(path: &PathBuf, stats: &SessionStats) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    let mut line = serde_json::to_string(stats)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

fn load_recent_stats(path: &PathBuf, n: usize) -> Vec<SessionStats> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let all: Vec<SessionStats> = content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let skip = all.len().saturating_sub(n);
    all.into_iter().skip(skip).collect()
}

// ── GohuFont BDF bitmap rendering ─────────────────────────────────────────────

static GLYPHS: OnceLock<HashMap<char, Vec<u8>>> = OnceLock::new();

fn glyphs() -> &'static HashMap<char, Vec<u8>> {
    GLYPHS.get_or_init(|| parse_bdf(include_str!("../assets/gohufont-uni-11.bdf")))
}

fn parse_bdf(bdf: &str) -> HashMap<char, Vec<u8>> {
    let mut map = HashMap::new();
    let mut encoding: Option<u32> = None;
    let mut in_bitmap = false;
    let mut rows: Vec<u8> = Vec::new();
    for line in bdf.lines() {
        if let Some(rest) = line.strip_prefix("ENCODING ") {
            encoding = rest
                .trim()
                .parse::<i32>()
                .ok()
                .and_then(|n| if n >= 0 { Some(n as u32) } else { None });
        } else if line == "BITMAP" {
            in_bitmap = true;
            rows.clear();
        } else if line == "ENDCHAR" {
            if let Some(enc) = encoding {
                if let Some(ch) = char::from_u32(enc) {
                    map.insert(ch, rows.clone());
                }
            }
            in_bitmap = false;
            encoding = None;
        } else if in_bitmap {
            if let Ok(byte) = u8::from_str_radix(line.trim(), 16) {
                rows.push(byte);
            }
        }
    }
    map
}

/// Render a character as bitmap lines using the GohuFont glyph (6×11, 2× horizontal scale).
fn render_glyph(ch: char, on_style: Style) -> Vec<Line<'static>> {
    let glyph_map = glyphs();
    let rows = match glyph_map.get(&ch) {
        Some(r) => r.clone(),
        None => return vec![Line::from(Span::styled(ch.to_string(), on_style))],
    };
    rows.iter()
        .map(|&byte| {
            let spans: Vec<Span<'static>> = (0..6)
                .flat_map(|bit| {
                    if byte & (0x80 >> bit) != 0 {
                        vec![Span::styled("██", on_style)]
                    } else {
                        vec![Span::raw("  ")]
                    }
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

// ── Syntax highlighting (tree-sitter) ─────────────────────────────────────────

const HIGHLIGHT_NAMES: &[&str] = &[
    "keyword",
    "string",
    "comment",
    "number",
    "operator",
    "function",
    "type",
    "variable",
    "constant",
    "punctuation",
    "attribute",
];

fn highlight_style(idx: usize) -> Style {
    match idx {
        0 => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD), // keyword
        1 => Style::default().fg(Color::Yellow),    // string
        2 => Style::default().fg(Color::DarkGray),  // comment
        3 => Style::default().fg(Color::LightBlue), // number
        4 => Style::default().fg(Color::White),     // operator
        5 => Style::default().fg(Color::LightGreen), // function
        6 => Style::default().fg(Color::LightMagenta), // type
        _ => Style::default().fg(Color::White),
    }
}

fn make_highlight_config(code_lang_idx: usize) -> Option<HighlightConfiguration> {
    let (language, highlights_query) = match code_lang_idx {
        0 => (
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY,
        ),
        1 => (
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::HIGHLIGHTS_QUERY,
        ),
        2 => (
            tree_sitter_javascript::LANGUAGE.into(),
            tree_sitter_javascript::HIGHLIGHT_QUERY,
        ),
        3 => (
            tree_sitter_go::LANGUAGE.into(),
            tree_sitter_go::HIGHLIGHTS_QUERY,
        ),
        4 => (
            tree_sitter_c::LANGUAGE.into(),
            tree_sitter_c::HIGHLIGHT_QUERY,
        ),
        5 => (
            tree_sitter_bash::LANGUAGE.into(),
            tree_sitter_bash::HIGHLIGHT_QUERY,
        ),
        _ => return None,
    };
    let mut config =
        HighlightConfiguration::new(language, "code", highlights_query, "", "").ok()?;
    config.configure(HIGHLIGHT_NAMES);
    Some(config)
}

fn syntax_highlight(code: &str, code_lang_idx: usize) -> Vec<Style> {
    let default_style = Style::default().fg(Color::White);
    let byte_len = code.len();

    let Some(config) = make_highlight_config(code_lang_idx) else {
        return code.chars().map(|_| default_style).collect();
    };

    let mut highlighter = Highlighter::new();
    let events = match highlighter.highlight(&config, code.as_bytes(), None, |_| None) {
        Ok(e) => e,
        Err(_) => return code.chars().map(|_| default_style).collect(),
    };

    // Build byte-indexed style array.
    let mut byte_styles = vec![default_style; byte_len];
    let mut current_style = default_style;

    for event in events.flatten() {
        match event {
            HighlightEvent::Source { start, end } => {
                for b in start..end.min(byte_len) {
                    byte_styles[b] = current_style;
                }
            }
            HighlightEvent::HighlightStart(h) => {
                current_style = highlight_style(h.0);
            }
            HighlightEvent::HighlightEnd => {
                current_style = default_style;
            }
        }
    }

    // Map byte-indexed → char-indexed.
    code.char_indices().map(|(b, _)| byte_styles[b]).collect()
}

// ── Local text generation ─────────────────────────────────────────────────────

fn generate_symbols(len: usize) -> String {
    let chars: Vec<char> = SYMBOLS.chars().collect();
    let mut rng = rand::rng();
    (0..len)
        .map(|_| chars[rng.random_range(0..chars.len())])
        .collect()
}

fn generate_flash_sequence(flash_chars: &[char], len: usize) -> String {
    if flash_chars.is_empty() {
        return String::new();
    }
    let mut rng = rand::rng();
    let mut result = Vec::with_capacity(len);
    let mut last = '\0';
    for _ in 0..len {
        let ch = if flash_chars.len() == 1 {
            flash_chars[0]
        } else {
            loop {
                let c = flash_chars[rng.random_range(0..flash_chars.len())];
                if c != last {
                    break c;
                }
            }
        };
        last = ch;
        result.push(ch);
    }
    result.into_iter().collect()
}

// ── Keymap (keyboard layout) ──────────────────────────────────────────────────

/// Per-layer char-to-LED-index map fetched from the Oryx API.
struct Keymap {
    /// `layers[layer_idx]` maps a character to its LED matrix index.
    layers: Vec<HashMap<char, u8>>,
    /// LED indices for shift modifier keys (LSFT / RSFT), used to highlight
    /// when the target character requires shift.
    shift_leds: Vec<u8>,
}

impl Keymap {
    /// Find the LED index for `ch` on `layer`, falling back to layer 0.
    fn find_key(&self, layer: u8, ch: char) -> Option<u8> {
        // Canonicalize: uppercase → lowercase for physical key lookup
        let canon = ch.to_lowercase().next().unwrap_or(ch);
        let layer = layer as usize;
        if let Some(map) = self.layers.get(layer) {
            if let Some(&idx) = map.get(&canon).or_else(|| map.get(&ch)) {
                return Some(idx);
            }
        }
        if layer != 0 {
            if let Some(map) = self.layers.get(0) {
                if let Some(&idx) = map.get(&canon).or_else(|| map.get(&ch)) {
                    return Some(idx);
                }
            }
        }
        None
    }

    /// All printable chars across every layer (union, deduped).
    fn all_printable_chars(&self) -> Vec<char> {
        let mut seen = HashMap::new();
        for layer in &self.layers {
            for &ch in layer.keys() {
                if !ch.is_control() {
                    seen.insert(ch, ());
                }
            }
        }
        seen.into_keys().collect()
    }
}

fn qmk_keycode_to_char(code: &str) -> Option<char> {
    let code = code.trim();
    if let Some(rest) = code.strip_prefix("KC_") {
        if rest.len() == 1 {
            let c = rest.chars().next().unwrap();
            if c.is_ascii_uppercase() {
                return Some(c.to_ascii_lowercase());
            }
        }
        return match rest {
            "0" => Some('0'),
            "1" => Some('1'),
            "2" => Some('2'),
            "3" => Some('3'),
            "4" => Some('4'),
            "5" => Some('5'),
            "6" => Some('6'),
            "7" => Some('7'),
            "8" => Some('8'),
            "9" => Some('9'),
            "SPACE" => Some(' '),
            "MINUS" | "MINS" => Some('-'),
            "EQUAL" | "EQL" => Some('='),
            "LBRC" => Some('['),
            "RBRC" => Some(']'),
            "BSLS" => Some('\\'),
            "SCLN" => Some(';'),
            "QUOT" => Some('\''),
            "GRV" => Some('`'),
            "COMM" => Some(','),
            "DOT" => Some('.'),
            "SLSH" => Some('/'),
            _ => None,
        };
    }
    None
}

fn build_keymap(layout: &oryx_hid::Layout) -> Keymap {
    let revision = match &layout.revision {
        Some(r) => r,
        None => {
            return Keymap {
                layers: Vec::new(),
                shift_leds: Vec::new(),
            };
        }
    };
    let mut shift_leds: Vec<u8> = Vec::new();
    let layers = revision
        .layers
        .iter()
        .enumerate()
        .map(|(layer_idx, layer)| {
            let mut map: HashMap<char, u8> = HashMap::new();
            for (idx, key) in layer.keys.iter().enumerate() {
                if let Some(tap) = &key.tap {
                    let code = tap.code.trim();
                    if layer_idx == 0
                        && (code == "KC_LSFT"
                            || code == "KC_RSFT"
                            || code == "KC_LSHIFT"
                            || code == "KC_RSHIFT")
                    {
                        shift_leds.push(idx as u8);
                    }
                    if let Some(ch) = qmk_keycode_to_char(code) {
                        map.entry(ch).or_insert(idx as u8);
                    }
                }
            }
            map
        })
        .collect();
    Keymap { layers, shift_leds }
}

// ── Typing session ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Prose,
    Words,
    Flash,
    Symbols,
    Ngrams,
    Code,
}

impl Mode {
    const ALL: &'static [Mode] = &[
        Mode::Prose,
        Mode::Words,
        Mode::Flash,
        Mode::Symbols,
        Mode::Ngrams,
        Mode::Code,
    ];

    fn name(self) -> &'static str {
        match self {
            Mode::Prose => "Prose",
            Mode::Words => "Words",
            Mode::Flash => "Flash",
            Mode::Symbols => "Symbols",
            Mode::Ngrams => "N-grams",
            Mode::Code => "Code",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Mode::Prose => "LLM-generated prose text",
            Mode::Words => "Plain words, no punctuation",
            Mode::Flash => "Press each displayed key",
            Mode::Symbols => "Practice typing symbols",
            Mode::Ngrams => "LLM-generated letter patterns",
            Mode::Code => "Type programming code",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|&m| m == self).unwrap()
    }
}

#[allow(dead_code)]
fn is_llm_mode(mode: Mode) -> bool {
    matches!(mode, Mode::Prose | Mode::Words | Mode::Code | Mode::Ngrams)
}

fn shows_prompt_input(mode: Mode) -> bool {
    matches!(mode, Mode::Prose | Mode::Words | Mode::Code)
}

struct TypingSession {
    target: Vec<char>,
    typed: Vec<char>,
    /// Per-char syntax highlight styles (code mode only).
    base_styles: Option<Vec<Style>>,
    start_time: Option<Instant>,
    finish_time: Option<Instant>,
    error_count: usize,
    /// Last keypress in flash mode was wrong.
    last_was_error: bool,
    mode: Mode,
    /// Streaming state for LLM modes.
    streaming: bool,
    stream_done: bool,
    word_limit: Option<usize>,
    /// Stored for refill requests (Infinite mode).
    gen_system: String,
    gen_user: String,
    /// For code mode re-highlighting.
    code_lang_idx: Option<usize>,
    prompt: String,
    /// Flash mode: when the current symbol was shown.
    char_shown_at: Option<Instant>,
    /// Flash mode: per-correct-press reaction times in ms (overall).
    reaction_times_ms: Vec<f64>,
    /// Flash mode: reaction times per symbol.
    reaction_by_symbol: HashMap<char, Vec<f64>>,
}

impl TypingSession {
    fn new_local(text: String, mode: Mode, prompt: String) -> Self {
        let target: Vec<char> = text.chars().collect();
        let char_shown_at = if mode == Mode::Flash && !target.is_empty() {
            Some(Instant::now())
        } else {
            None
        };
        Self {
            target,
            typed: Vec::new(),
            base_styles: None,
            start_time: None,
            finish_time: None,
            error_count: 0,
            last_was_error: false,
            mode,
            streaming: false,
            stream_done: true,
            word_limit: None,
            gen_system: String::new(),
            gen_user: String::new(),
            code_lang_idx: None,
            prompt,
            char_shown_at,
            reaction_times_ms: Vec::new(),
            reaction_by_symbol: HashMap::new(),
        }
    }

    fn new_streaming(
        mode: Mode,
        word_limit: Option<usize>,
        gen_system: String,
        gen_user: String,
        code_lang_idx: Option<usize>,
        prompt: String,
    ) -> Self {
        Self {
            target: Vec::new(),
            typed: Vec::new(),
            base_styles: None,
            start_time: None,
            finish_time: None,
            error_count: 0,
            last_was_error: false,
            mode,
            streaming: true,
            stream_done: false,
            word_limit,
            gen_system,
            gen_user,
            code_lang_idx,
            prompt,
            char_shown_at: None,
            reaction_times_ms: Vec::new(),
            reaction_by_symbol: HashMap::new(),
        }
    }

    fn append_text(&mut self, chunk: &str) {
        if let Some(limit) = self.word_limit {
            let current_words = self
                .target
                .iter()
                .filter(|&&c| c == ' ' || c == '\n')
                .count()
                + 1;
            if current_words >= limit {
                self.stream_done = true;
                self.streaming = false;
                return;
            }
            let remaining = limit.saturating_sub(current_words) + 1;
            let mut word_i = 0;
            let mut byte_cut = chunk.len();
            for (byte, ch) in chunk.char_indices() {
                if ch == ' ' || ch == '\n' {
                    word_i += 1;
                    if word_i >= remaining {
                        byte_cut = byte;
                        break;
                    }
                }
            }
            for ch in chunk[..byte_cut].chars() {
                self.target.push(ch);
            }
        } else {
            for ch in chunk.chars() {
                self.target.push(ch);
            }
        }
        if let Some(lang_idx) = self.code_lang_idx {
            let text: String = self.target.iter().collect();
            self.base_styles = Some(syntax_highlight(&text, lang_idx));
        }
    }

    fn needs_refill(&self) -> bool {
        const AHEAD: usize = 150;
        // Prose mode with no word limit streams indefinitely and refills when the buffer runs low.
        self.mode == Mode::Prose
            && self.word_limit.is_none()
            && self.stream_done
            && !self.streaming
            && self.target.len().saturating_sub(self.typed.len()) < AHEAD
    }

    fn is_complete(&self) -> bool {
        if self.mode == Mode::Prose && self.word_limit.is_none() {
            return false;
        }
        !self.target.is_empty() && self.typed.len() >= self.target.len() && self.stream_done
    }

    fn elapsed(&self) -> Duration {
        match (self.start_time, self.finish_time) {
            (Some(s), Some(e)) => e.duration_since(s),
            (Some(s), None) => s.elapsed(),
            _ => Duration::ZERO,
        }
    }

    fn correct_count(&self) -> usize {
        self.typed
            .iter()
            .zip(self.target.iter())
            .filter(|(a, b)| a == b)
            .count()
    }

    fn wpm(&self) -> f64 {
        let secs = self.elapsed().as_secs_f64();
        if secs < 0.1 {
            return 0.0;
        }
        (self.correct_count() as f64 / 5.0) / (secs / 60.0)
    }

    fn accuracy(&self) -> f64 {
        let n = self.typed.len();
        if n == 0 {
            return 1.0;
        }
        self.correct_count() as f64 / n as f64
    }

    fn type_char(&mut self, c: char) {
        if self.is_complete() {
            return;
        }
        let expected = self.target.get(self.typed.len()).copied();
        if let Some(exp) = expected {
            if self.start_time.is_none() {
                self.start_time = Some(Instant::now());
            }
            if c != exp {
                self.error_count += 1;
            }
            self.typed.push(c);
            if self.is_complete() {
                self.finish_time = Some(Instant::now());
            }
        }
    }

    fn avg_reaction_ms(&self) -> Option<f64> {
        if self.reaction_times_ms.is_empty() {
            return None;
        }
        Some(self.reaction_times_ms.iter().sum::<f64>() / self.reaction_times_ms.len() as f64)
    }

    fn backspace(&mut self) {
        self.typed.pop();
    }

    fn force_done(&mut self) {
        if self.finish_time.is_none() {
            self.finish_time = Some(Instant::now());
        }
    }

    fn to_stats(&self) -> SessionStats {
        SessionStats {
            timestamp_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            mode: self.mode.name().to_string(),
            duration_secs: self.elapsed().as_secs_f64(),
            wpm: self.wpm(),
            accuracy: self.accuracy(),
            errors: self.error_count,
            correct: self.correct_count(),
            text_length: self.typed.len(),
            prompt: self.prompt.clone(),
            avg_reaction_ms: self.avg_reaction_ms(),
            reaction_by_symbol: if self.reaction_by_symbol.is_empty() {
                None
            } else {
                Some(
                    self.reaction_by_symbol
                        .iter()
                        .map(|(&ch, times)| {
                            let avg = times.iter().sum::<f64>() / times.len() as f64;
                            (ch.to_string(), avg)
                        })
                        .collect(),
                )
            },
        }
    }
}

// ── Text rendering ────────────────────────────────────────────────────────────

fn char_display(
    i: usize,
    pos: usize,
    typed: &[char],
    target: &[char],
    base_styles: Option<&Vec<Style>>,
) -> (char, Style) {
    let tgt = target[i];
    if i < pos {
        let typed_ch = typed[i];
        if typed_ch == tgt {
            (tgt, Style::default().fg(Color::Green))
        } else {
            (
                typed_ch,
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::UNDERLINED),
            )
        }
    } else if i == pos {
        (tgt, Style::default().fg(Color::White).bg(Color::DarkGray))
    } else {
        let base = base_styles
            .and_then(|s| s.get(i))
            .copied()
            .unwrap_or_else(|| Style::default().fg(Color::DarkGray));
        (tgt, base)
    }
}

fn build_typed_lines(session: &TypingSession, width: u16) -> (Vec<Line<'static>>, u16) {
    let w = (width as usize).saturating_sub(2).max(20);
    let pos = session.typed.len();
    let base = session.base_styles.as_ref();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cursor_line: u16 = 0;
    let mut col = 0usize;
    let mut cur_text = String::new();
    let mut cur_style = Style::default();
    let mut spans: Vec<Span<'static>> = Vec::new();

    for (i, &tgt_ch) in session.target.iter().enumerate() {
        if i == pos {
            cursor_line = lines.len() as u16;
        }
        let needs_wrap = tgt_ch == '\n' || col >= w;
        if needs_wrap {
            if !cur_text.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut cur_text), cur_style));
            }
            lines.push(Line::from(std::mem::take(&mut spans)));
            col = 0;
            if tgt_ch == '\n' {
                continue;
            }
        }
        let (display_ch, style) = char_display(i, pos, &session.typed, &session.target, base);
        if style != cur_style && !cur_text.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut cur_text), cur_style));
        }
        cur_style = style;
        cur_text.push(display_ch);
        col += 1;
    }
    if !cur_text.is_empty() {
        spans.push(Span::styled(cur_text, cur_style));
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    if pos >= session.target.len() && !lines.is_empty() {
        cursor_line = (lines.len() - 1) as u16;
    }
    (lines, cursor_line)
}

// ── App state ──────────────────────────────────────────────────────────────────

#[derive(PartialEq)]
enum Screen {
    Config,
    Typing,
    Done,
}

enum AppMsg {
    TextChunk(String),
    StreamDone,
    LlmError(String),
    /// Keyboard layer changed.
    LayerChanged(u8),
    /// Keyboard connected and layout fetched.
    KbReady(Keymap),
    /// Keyboard disconnected.
    KbGone,
}

/// Commands from App to the keyboard task.
enum KbCmd {
    SetLed(u8, u8, u8, u8), // index, r, g, b
    ClearAll,
}

#[derive(PartialEq)]
enum ConfigFocus {
    ModeList,
    Options,
    Prompt,
}

struct ConfigState {
    mode: Mode,
    lang_idx: usize,
    code_lang_idx: usize,
    options_idx: usize,
    word_count: usize,
    /// When true, Prose mode streams indefinitely (no word limit).
    prose_infinite: bool,
    ngram_kind: usize,
    ngram_count: usize,
    symbol_len: usize,
    prompt_buf: String,
    prompt_cursor: usize,
    sample_idx: usize,
    focus: ConfigFocus,
    samples: Vec<String>,
    /// Saved prompt text per mode, to restore when switching back.
    prose_prompt: String,
    words_prompt: String,
    code_prompt: String,
}

impl ConfigState {
    fn option_count(&self) -> usize {
        match self.mode {
            Mode::Prose | Mode::Words => 2,
            Mode::Flash => 0,
            Mode::Symbols => 1,
            Mode::Ngrams => 2,
            Mode::Code => 1,
        }
    }

    fn option_items(&self) -> Vec<(&'static str, String)> {
        match self.mode {
            Mode::Prose => vec![
                ("Language", LANGUAGES[self.lang_idx].name.to_string()),
                (
                    "Words",
                    if self.prose_infinite {
                        "∞".into()
                    } else {
                        self.word_count.to_string()
                    },
                ),
            ],
            Mode::Words => vec![
                ("Language", LANGUAGES[self.lang_idx].name.to_string()),
                ("Words", self.word_count.to_string()),
            ],
            Mode::Flash => vec![],
            Mode::Symbols => vec![("Length", self.symbol_len.to_string())],
            Mode::Ngrams => vec![
                ("Type", NGRAM_KINDS[self.ngram_kind].to_string()),
                ("Count", self.ngram_count.to_string()),
            ],
            Mode::Code => vec![("Language", CODE_LANGS[self.code_lang_idx].name.to_string())],
        }
    }

    fn adjust_option(&mut self, delta: i32) {
        match (self.mode, self.options_idx) {
            (Mode::Prose, 0) | (Mode::Words, 0) => {
                let n = LANGUAGES.len() as i32;
                self.lang_idx = ((self.lang_idx as i32 + delta).rem_euclid(n)) as usize;
            }
            (Mode::Prose, 1) => {
                if self.prose_infinite && delta > 0 {
                    self.prose_infinite = false;
                    self.word_count = 10;
                } else if !self.prose_infinite && self.word_count <= 10 && delta < 0 {
                    self.prose_infinite = true;
                } else if !self.prose_infinite {
                    self.word_count = (self.word_count as i32 + delta * 10).clamp(10, 500) as usize;
                }
            }
            (Mode::Words, 1) => {
                self.word_count = (self.word_count as i32 + delta * 10).clamp(10, 500) as usize;
            }
            (Mode::Symbols, 0) => {
                self.symbol_len = (self.symbol_len as i32 + delta * 10).clamp(10, 300) as usize;
            }
            (Mode::Ngrams, 0) => {
                let n = NGRAM_KINDS.len() as i32;
                self.ngram_kind = ((self.ngram_kind as i32 + delta).rem_euclid(n)) as usize;
            }
            (Mode::Ngrams, 1) => {
                self.ngram_count = (self.ngram_count as i32 + delta * 5).clamp(5, 200) as usize;
            }
            (Mode::Code, 0) => {
                let n = CODE_LANGS.len() as i32;
                self.code_lang_idx = ((self.code_lang_idx as i32 + delta).rem_euclid(n)) as usize;
            }
            _ => {}
        }
    }

    fn load_sample(&mut self, idx: usize) {
        if idx < self.samples.len() {
            self.sample_idx = idx;
            self.prompt_buf = self.samples[idx].clone();
            self.prompt_cursor = self.prompt_buf.chars().count();
        }
    }

    fn insert_char(&mut self, c: char) {
        let byte = char_to_byte(&self.prompt_buf, self.prompt_cursor);
        self.prompt_buf.insert(byte, c);
        self.prompt_cursor += 1;
    }

    fn delete_before_cursor(&mut self) {
        if self.prompt_cursor > 0 {
            let end = char_to_byte(&self.prompt_buf, self.prompt_cursor);
            let start = char_to_byte(&self.prompt_buf, self.prompt_cursor - 1);
            self.prompt_buf.drain(start..end);
            self.prompt_cursor -= 1;
        }
    }
}

fn char_to_byte(s: &str, idx: usize) -> usize {
    s.char_indices().nth(idx).map(|(b, _)| b).unwrap_or(s.len())
}

struct App {
    screen: Screen,
    cfg: ConfigState,
    typing: Option<TypingSession>,
    status_msg: String,
    stats_file: PathBuf,
    stats_save_error: Option<String>,
    provider: String,
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
    /// Channel to the keyboard background task (None when no keyboard connected).
    kb_tx: Option<mpsc::UnboundedSender<KbCmd>>,
    /// Layout fetched from the Oryx API.
    keymap: Option<Keymap>,
    /// Currently active layer reported by the keyboard.
    current_layer: u8,
    /// Accent color used for active pane borders/titles and status bars.
    accent_color: (u8, u8, u8),
    /// Keyboard LED color for the next key to press.
    highlight_color: (u8, u8, u8),
    stats_window: usize,
    recent_stats: Vec<SessionStats>,
}

// ── Keyboard LED helpers ───────────────────────────────────────────────────────

/// Light up the LED for the current target character.
fn update_kbd_led(app: &App) {
    let Some(ref kb_tx) = app.kb_tx else { return };
    let Some(ref km) = app.keymap else { return };
    let Some(ref session) = app.typing else {
        let _ = kb_tx.send(KbCmd::ClearAll);
        return;
    };
    let cursor = session.typed.len();
    let Some(&ch) = session.target.get(cursor) else {
        return;
    };
    let Some(led_idx) = km.find_key(app.current_layer, ch) else {
        return;
    };
    let (r, g, b) = app.highlight_color;
    let _ = kb_tx.send(KbCmd::ClearAll);
    let _ = kb_tx.send(KbCmd::SetLed(led_idx, r, g, b));
    if ch.is_uppercase() {
        for &shift_idx in &km.shift_leds {
            let _ = kb_tx.send(KbCmd::SetLed(shift_idx, r, g, b));
        }
    }
}

// ── Keyboard background task ──────────────────────────────────────────────────

fn spawn_keyboard_task(
    app_tx: mpsc::UnboundedSender<AppMsg>,
) -> Option<mpsc::UnboundedSender<KbCmd>> {
    let (kb_tx, mut kb_rx) = mpsc::unbounded_channel::<KbCmd>();

    tokio::spawn(async move {
        let mut kb = match oryx_hid::asynchronous::OryxKeyboard::open().await {
            Ok(k) => k,
            Err(_) => return, // no keyboard; degrade gracefully
        };

        let firmware = match kb.firmware().await {
            Ok(f) => f,
            Err(_) => return,
        };

        if let Ok(resp) = oryx_hid::layout::fetch(&firmware.layout, &firmware.revision, "").await {
            let km = build_keymap(&resp.data.layout);
            let _ = app_tx.send(AppMsg::KbReady(km));
        }

        let _ = kb.rgb_control(true).await;

        loop {
            tokio::select! {
                cmd = kb_rx.recv() => {
                    match cmd {
                        Some(KbCmd::SetLed(idx, r, g, b)) => {
                            let _ = kb.rgb(idx, r, g, b).await;
                        }
                        Some(KbCmd::ClearAll) => {
                            let _ = kb.rgb_all(0, 0, 0).await;
                        }
                        None => break,
                    }
                }
                event = kb.recv_event() => {
                    match event {
                        Ok(oryx_hid::asynchronous::Event::Layer(n)) => {
                            let _ = app_tx.send(AppMsg::LayerChanged(n));
                        }
                        Err(_) => {
                            let _ = app_tx.send(AppMsg::KbGone);
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    Some(kb_tx)
}

// ── LLM task ─────────────────────────────────────────────────────────────────

fn spawn_llm_stream(
    tx: mpsc::UnboundedSender<AppMsg>,
    system_prompt: String,
    user_prompt: String,
    provider: String,
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
) {
    tokio::spawn(async move {
        let backend = match parse_backend(&provider) {
            Ok(b) => b,
            Err(e) => {
                let _ = tx.send(AppMsg::LlmError(e.to_string()));
                return;
            }
        };
        let mut builder = LLMBuilder::new()
            .backend(backend)
            .model(model)
            .system(system_prompt)
            .max_tokens(512);
        if let Some(key) = api_key {
            builder = builder.api_key(key);
        }
        if let Some(url) = base_url {
            builder = builder.base_url(url);
        }
        let llm = match builder.build() {
            Ok(l) => l,
            Err(e) => {
                let _ = tx.send(AppMsg::LlmError(e.to_string()));
                return;
            }
        };
        let messages = vec![ChatMessage::user().content(user_prompt).build()];
        match llm.chat(&messages).await {
            Ok(resp) => {
                let text = resp.text().unwrap_or_default();
                let _ = tx.send(AppMsg::TextChunk(text));
                let _ = tx.send(AppMsg::StreamDone);
            }
            Err(e) => {
                let _ = tx.send(AppMsg::LlmError(e.to_string()));
            }
        }
    });
}

// ── System prompt builders ────────────────────────────────────────────────────

fn build_system_prompt(app: &App) -> String {
    match app.cfg.mode {
        Mode::Code => format!(
            "You are a code snippet generator for typing practice. Generate a short, \
             clean, well-formatted {} code snippet suitable for typing. Use proper \
             indentation with spaces. Use only printable ASCII characters. Output only \
             the code with no explanations, no markdown, no code fences.",
            CODE_LANGS[app.cfg.code_lang_idx].name
        ),
        Mode::Prose => {
            let mut sys = if app.cfg.prose_infinite {
                LANGUAGES[app.cfg.lang_idx].system_prompt.to_string()
            } else {
                format!(
                    "{} Generate exactly {} words.",
                    LANGUAGES[app.cfg.lang_idx].system_prompt, app.cfg.word_count
                )
            };
            sys.push_str(
                " Avoid repeating words, phrases, or sentence structures. \
                 Vary your vocabulary. Do not start multiple sentences the same way.",
            );
            sys
        }
        Mode::Words => {
            let mut sys = format!(
                "You are a typing practice text generator. Generate exactly {} common English \
                 words for typing practice, separated by spaces. Use only lowercase letters. \
                 No punctuation, no symbols, no numbers. Output only the words.",
                app.cfg.word_count
            );
            if !app.cfg.prompt_buf.trim().is_empty() {
                sys.push_str(&format!(
                    " Topic or domain: {}.",
                    app.cfg.prompt_buf.trim()
                ));
            }
            sys
        }
        Mode::Ngrams => {
            let kind = NGRAM_KINDS[app.cfg.ngram_kind];
            format!(
                "You are a typing practice text generator. Generate exactly {} {kind} \
                 for typing practice, separated by spaces. Output only the {kind} with \
                 no punctuation, no numbers, no preamble, no explanation.",
                app.cfg.ngram_count
            )
        }
        _ => LANGUAGES[app.cfg.lang_idx].system_prompt.to_string(),
    }
}

// ── Drawing ───────────────────────────────────────────────────────────────────

fn draw(f: &mut Frame, app: &App) {
    match app.screen {
        Screen::Config => draw_config(f, app),
        Screen::Typing => draw_typing(f, app),
        Screen::Done => draw_done(f, app),
    }
}

fn draw_config(f: &mut Frame, app: &App) {
    let area = f.area();
    let cfg = &app.cfg;
    let show_prompt = shows_prompt_input(cfg.mode);
    let accent = {
        let (r, g, b) = app.accent_color;
        Color::Rgb(r, g, b)
    };

    let top_split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);
    let stats_row = top_split[0];
    let content_area = top_split[1];

    let error_h = if app.status_msg.is_empty() { 0 } else { 1 };
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(if show_prompt {
            vec![
                Constraint::Length(error_h), // error
                Constraint::Min(8),          // top (mode list + options)
                Constraint::Length(4),       // prompt input
                Constraint::Min(3),          // samples
                Constraint::Length(1),       // help
            ]
        } else {
            vec![
                Constraint::Length(error_h), // error
                Constraint::Min(8),          // top
                Constraint::Length(1),       // help
            ]
        })
        .split(content_area);

    let (err_row, top_row, prompt_row, samples_row, help_row) = if show_prompt {
        (outer[0], outer[1], outer[2], outer[3], outer[4])
    } else {
        (outer[0], outer[1], outer[1], outer[1], outer[2])
    };

    // Stats bar
    {
        let (ar, ag, ab) = app.accent_color;
        let bar_bg = Color::Rgb(ar, ag, ab);
        let bar_fg = readable_fg(ar, ag, ab);
        let bar_style = Style::default()
            .fg(bar_fg)
            .bg(bar_bg)
            .add_modifier(Modifier::BOLD);
        let mode_name = app.cfg.mode.name();
        let stats_text = if app.recent_stats.is_empty() {
            format!(" No recent sessions ({mode_name})")
        } else {
            let n = app.recent_stats.len();
            let avg_acc = app.recent_stats.iter().map(|s| s.accuracy).sum::<f64>() / n as f64;
            if app.cfg.mode == Mode::Flash {
                let reaction_vals: Vec<f64> = app
                    .recent_stats
                    .iter()
                    .filter_map(|s| s.avg_reaction_ms)
                    .collect();
                if reaction_vals.is_empty() {
                    format!(
                        " Last {n} ({mode_name}):  Accuracy: {:5.1}%  |  Avg Reaction: - ",
                        avg_acc * 100.0,
                    )
                } else {
                    let avg_react = reaction_vals.iter().sum::<f64>() / reaction_vals.len() as f64;
                    let best_react = reaction_vals.iter().cloned().fold(f64::INFINITY, f64::min);
                    format!(
                        " Last {n} ({mode_name}):  Accuracy: {:5.1}%  |  Avg Reaction: {:.0}ms  |  Best: {:.0}ms ",
                        avg_acc * 100.0,
                        avg_react,
                        best_react,
                    )
                }
            } else {
                let avg_wpm = app.recent_stats.iter().map(|s| s.wpm).sum::<f64>() / n as f64;
                let best_wpm = app
                    .recent_stats
                    .iter()
                    .map(|s| s.wpm)
                    .fold(f64::NEG_INFINITY, f64::max);
                format!(
                    " Last {n} ({mode_name}):  WPM: {:5.1}  |  Accuracy: {:5.1}%  |  Best: {:5.1} ",
                    avg_wpm,
                    avg_acc * 100.0,
                    best_wpm,
                )
            }
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(stats_text, bar_style)))
                .style(Style::default().bg(bar_bg)),
            stats_row,
        );
    }

    if !app.status_msg.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                app.status_msg.as_str(),
                Style::default().fg(Color::Red),
            )),
            err_row,
        );
    }

    let mode_col_w = Mode::ALL.iter().map(|m| m.name().len()).max().unwrap_or(8) as u16 + 4;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(mode_col_w), Constraint::Min(20)])
        .split(top_row);

    // Mode list
    {
        let mode_focused = cfg.focus == ConfigFocus::ModeList;
        let items: Vec<ListItem> = Mode::ALL
            .iter()
            .map(|&mode| {
                let sel = mode == cfg.mode;
                let style = match (mode_focused, sel) {
                    (true, true) => Style::default().fg(accent).add_modifier(Modifier::BOLD),
                    (_, true) => Style::default().fg(Color::White),
                    _ => Style::default().fg(Color::DarkGray),
                };
                ListItem::new(Line::from(Span::styled(mode.name().to_string(), style)))
            })
            .collect();
        f.render_widget(
            List::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Mode ")
                    .border_style(if mode_focused {
                        Style::default().fg(accent)
                    } else {
                        Style::default()
                    }),
            ),
            cols[0],
        );
    }

    // Options pane
    {
        let opt_focused = cfg.focus == ConfigFocus::Options;
        let items = cfg.option_items();
        let lines: Vec<Line> = items
            .iter()
            .enumerate()
            .map(|(i, (label, value))| {
                let sel = opt_focused && i == cfg.options_idx;
                let (label_style, val_style) = if sel {
                    (
                        Style::default().fg(accent).add_modifier(Modifier::BOLD),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    (
                        Style::default().fg(Color::DarkGray),
                        Style::default().fg(Color::Gray),
                    )
                };
                Line::from(vec![
                    Span::styled(format!("{label:<12}: "), label_style),
                    Span::styled(value.clone(), val_style),
                    if sel {
                        Span::styled("  [← →]", Style::default().fg(Color::DarkGray))
                    } else {
                        Span::raw("")
                    },
                ])
            })
            .collect();
        let options_title = format!(" {} ", cfg.mode.description());
        f.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(options_title)
                    .border_style(if opt_focused {
                        Style::default().fg(accent)
                    } else {
                        Style::default()
                    }),
            ),
            cols[1],
        );
    }

    if show_prompt {
        // Prompt input
        {
            let prompt_focused = cfg.focus == ConfigFocus::Prompt;
            let chars: Vec<char> = cfg.prompt_buf.chars().collect();
            let before: String = chars[..cfg.prompt_cursor].iter().collect();
            let cursor_ch: String = chars
                .get(cfg.prompt_cursor)
                .map(|c| c.to_string())
                .unwrap_or_else(|| " ".into());
            let after: String = chars[(cfg.prompt_cursor + 1).min(chars.len())..]
                .iter()
                .collect();
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::raw(before),
                    Span::styled(
                        cursor_ch,
                        Style::default().fg(Color::Black).bg(Color::White),
                    ),
                    Span::raw(after),
                ]))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Prompt ")
                        .border_style(if prompt_focused {
                            Style::default().fg(accent)
                        } else {
                            Style::default()
                        }),
                )
                .wrap(Wrap { trim: false }),
                prompt_row,
            );
        }

        // Samples list
        {
            let items: Vec<ListItem> = cfg
                .samples
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let max = 70usize;
                    let display: String = if s.chars().count() > max {
                        format!("{}…", s.chars().take(max).collect::<String>())
                    } else {
                        s.clone()
                    };
                    let style = if i == cfg.sample_idx && cfg.focus == ConfigFocus::Prompt {
                        Style::default().fg(accent).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    ListItem::new(display).style(style)
                })
                .collect();
            f.render_widget(
                List::new(items).block(Block::default().borders(Borders::ALL).title(" Samples ")),
                samples_row,
            );
        }
    }

    let hint_style = Style::default().fg(accent).add_modifier(Modifier::BOLD);
    let help = Line::from(vec![
        Span::styled("[Tab]", hint_style),
        Span::raw(" switch  "),
        Span::styled("[↑↓]", hint_style),
        Span::raw(" navigate  "),
        Span::styled("[←→]", hint_style),
        Span::raw(" adjust  "),
        Span::styled("[Enter]", hint_style),
        Span::raw(" start  "),
        Span::styled("[Esc]", hint_style),
        Span::raw(" quit"),
    ]);
    f.render_widget(Paragraph::new(help), help_row);
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn draw_typing(f: &mut Frame, app: &App) {
    let session = match app.typing.as_ref() {
        Some(s) => s,
        None => return,
    };

    let (ar, ag, ab) = app.accent_color;
    let bar_bg = Color::Rgb(ar, ag, ab);
    let bar_fg = readable_fg(ar, ag, ab);
    let bar_style = Style::default()
        .fg(bar_fg)
        .bg(bar_bg)
        .add_modifier(Modifier::BOLD);
    let accent = bar_bg;

    if session.mode == Mode::Flash {
        draw_flash_typing(f, session, bar_style, accent);
        return;
    }

    let area = f.area();
    let status = format!(
        " WPM: {:5.1}  |  Accuracy: {:5.1}%  |  Errors: {:3}  |  Time: {} ",
        session.wpm(),
        session.accuracy() * 100.0,
        session.error_count,
        fmt_duration(session.elapsed()),
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(status, bar_style)))
            .style(Style::default().bg(bar_bg)),
        chunks[0],
    );

    if session.target.is_empty() {
        let wait_lines = if app.status_msg.is_empty() {
            vec![Line::from(Span::styled(
                "Waiting for text…",
                Style::default().fg(Color::DarkGray),
            ))]
        } else {
            vec![
                Line::from(Span::styled(
                    app.status_msg.as_str(),
                    Style::default().fg(Color::Red),
                )),
                Line::from(Span::styled(
                    "Press [Esc] to go back",
                    Style::default().fg(Color::DarkGray),
                )),
            ]
        };
        f.render_widget(
            Paragraph::new(wait_lines)
                .block(Block::default().borders(Borders::ALL).title(" Type ")),
            chunks[1],
        );
    } else {
        let (lines, cursor_line) = build_typed_lines(session, chunks[1].width);
        let view_h = chunks[1].height.saturating_sub(2);
        let scroll = if view_h == 0 || cursor_line < view_h / 2 {
            0
        } else {
            cursor_line.saturating_sub(view_h / 2)
        };
        let title = if session.streaming {
            " Type  [generating…] "
        } else {
            " Type "
        };
        f.render_widget(
            Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title(title))
                .scroll((scroll, 0)),
            chunks[1],
        );
    }

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "[Backspace]",
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" correct  "),
            Span::styled(
                "[Esc]",
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" stop"),
        ])),
        chunks[2],
    );
}

fn draw_flash_typing(f: &mut Frame, session: &TypingSession, bar_style: Style, accent: Color) {
    let area = f.area();
    let pos = session.typed.len();
    let total = session.target.len();
    let bar_bg = match bar_style.bg {
        Some(c) => c,
        None => Color::White,
    };

    let reaction_str = match session.avg_reaction_ms() {
        Some(ms) => format!("{:.0}ms", ms),
        None => "-".into(),
    };
    let status = format!(
        " Progress: {pos}/{total}  |  Correct: {}  |  Errors: {}  |  Accuracy: {:.1}%  |  Avg: {} ",
        session.correct_count(),
        session.error_count,
        session.accuracy() * 100.0,
        reaction_str,
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(status, bar_style)))
            .style(Style::default().bg(bar_bg)),
        chunks[0],
    );

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Length(11),
            Constraint::Percentage(30),
            Constraint::Min(0),
        ])
        .split(chunks[1]);

    if pos < total {
        let ch = session.target[pos];
        let on_style = if session.last_was_error {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        };
        let lines = render_glyph(ch, on_style);
        f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner[1]);
    }

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "[Esc]",
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" stop"),
        ])),
        chunks[2],
    );
}

fn draw_done(f: &mut Frame, app: &App) {
    let area = f.area();
    let accent = {
        let (r, g, b) = app.accent_color;
        Color::Rgb(r, g, b)
    };
    let session = match app.typing.as_ref() {
        Some(s) => s,
        None => return,
    };

    let mode_name = session.mode.name();

    let mut content = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Exercise Complete!",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        if session.mode == Mode::Flash {
            let reaction_str = match session.avg_reaction_ms() {
                Some(ms) => format!("{:.0}ms", ms),
                None => "-".into(),
            };
            Line::from(format!(
                "Mode: {}   Avg Reaction: {}   Accuracy: {:.1}%   Errors: {}   Time: {}",
                mode_name,
                reaction_str,
                session.accuracy() * 100.0,
                session.error_count,
                fmt_duration(session.elapsed()),
            ))
        } else {
            Line::from(format!(
                "Mode: {}   WPM: {:.1}   Accuracy: {:.1}%   Errors: {}   Time: {}",
                mode_name,
                session.wpm(),
                session.accuracy() * 100.0,
                session.error_count,
                fmt_duration(session.elapsed()),
            ))
        },
        Line::from(""),
    ];

    match &app.stats_save_error {
        Some(err) => content.push(Line::from(Span::styled(
            format!("Warning: could not save stats — {err}"),
            Style::default().fg(Color::Red),
        ))),
        None => content.push(Line::from(Span::styled(
            format!("Stats saved to {}", app.stats_file.display()),
            Style::default().fg(Color::DarkGray),
        ))),
    }

    content.push(Line::from(""));
    let hint_style = Style::default().fg(accent).add_modifier(Modifier::BOLD);
    content.push(Line::from(vec![
        Span::styled("[Enter] / [Esc]", hint_style),
        Span::raw(" back  "),
        Span::styled("[q]", hint_style),
        Span::raw(" quit"),
    ]));

    f.render_widget(
        Paragraph::new(content)
            .block(Block::default().borders(Borders::ALL).title(" Results "))
            .alignment(Alignment::Center),
        area,
    );
}

// ── Session creation ──────────────────────────────────────────────────────────

fn start_session(app: &mut App, tx: &mpsc::UnboundedSender<AppMsg>) {
    app.status_msg.clear();
    match app.cfg.mode {
        Mode::Flash => {
            if app.keymap.is_none() {
                app.status_msg = "Flash mode requires a connected keyboard.".into();
                return;
            }
            let flash_chars: Vec<char> = app
                .keymap
                .as_ref()
                .map(|km| km.all_printable_chars())
                .filter(|chars| !chars.is_empty())
                .unwrap_or_default();
            if flash_chars.is_empty() {
                app.status_msg = "No printable keys found in keyboard layout.".into();
                return;
            }
            let text = generate_flash_sequence(&flash_chars, 100);
            app.typing = Some(TypingSession::new_local(text, Mode::Flash, "Flash".into()));
            app.screen = Screen::Typing;
            update_kbd_led(app);
        }
        Mode::Symbols => {
            let text = generate_symbols(app.cfg.symbol_len);
            app.typing = Some(TypingSession::new_local(
                text,
                Mode::Symbols,
                "Symbols".into(),
            ));
            app.screen = Screen::Typing;
            update_kbd_led(app);
        }
        Mode::Ngrams => {
            let sys = build_system_prompt(app);
            let user = format!(
                "Generate {} {}.",
                app.cfg.ngram_count, NGRAM_KINDS[app.cfg.ngram_kind]
            );
            let session = TypingSession::new_streaming(
                Mode::Ngrams,
                None,
                sys.clone(),
                user.clone(),
                None,
                format!("Ngrams/{}", NGRAM_KINDS[app.cfg.ngram_kind]),
            );
            app.typing = Some(session);
            app.screen = Screen::Typing;
            spawn_llm_stream(
                tx.clone(),
                sys,
                user,
                app.provider.clone(),
                app.model.clone(),
                app.api_key.clone(),
                app.base_url.clone(),
            );
        }
        Mode::Prose => {
            let word_limit = (!app.cfg.prose_infinite).then_some(app.cfg.word_count);
            let sys = build_system_prompt(app);
            let user = app.cfg.prompt_buf.trim().to_string();
            let session = TypingSession::new_streaming(
                Mode::Prose,
                word_limit,
                sys.clone(),
                user.clone(),
                None,
                user.clone(),
            );
            app.typing = Some(session);
            app.screen = Screen::Typing;
            spawn_llm_stream(
                tx.clone(),
                sys,
                user,
                app.provider.clone(),
                app.model.clone(),
                app.api_key.clone(),
                app.base_url.clone(),
            );
        }
        Mode::Words => {
            let sys = build_system_prompt(app);
            let prompt_text = app.cfg.prompt_buf.trim().to_string();
            let user = if prompt_text.is_empty() {
                format!("Generate {} words.", app.cfg.word_count)
            } else {
                prompt_text.clone()
            };
            let session = TypingSession::new_streaming(
                Mode::Words,
                Some(app.cfg.word_count),
                sys.clone(),
                user.clone(),
                None,
                prompt_text,
            );
            app.typing = Some(session);
            app.screen = Screen::Typing;
            spawn_llm_stream(
                tx.clone(),
                sys,
                user,
                app.provider.clone(),
                app.model.clone(),
                app.api_key.clone(),
                app.base_url.clone(),
            );
        }
        Mode::Code => {
            let sys = build_system_prompt(app);
            let user = app.cfg.prompt_buf.trim().to_string();
            let session = TypingSession::new_streaming(
                Mode::Code,
                None,
                sys.clone(),
                user.clone(),
                Some(app.cfg.code_lang_idx),
                user.clone(),
            );
            app.typing = Some(session);
            app.screen = Screen::Typing;
            spawn_llm_stream(
                tx.clone(),
                sys,
                user,
                app.provider.clone(),
                app.model.clone(),
                app.api_key.clone(),
                app.base_url.clone(),
            );
        }
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config_path = cli.config.unwrap_or_else(|| {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("oryx-train")
            .join("config.toml")
    });
    let config = load_config(&config_path);
    let mut persisted_config = config.clone(); // kept for mode_cache save on exit

    let provider = cli
        .provider
        .or(config.llm.provider)
        .unwrap_or_else(|| "openai".into());
    let model = cli
        .model
        .or(config.llm.model)
        .unwrap_or_else(|| default_model(&provider));
    let api_key = cli.api_key.or(config.llm.api_key);
    let base_url = cli.base_url.or(config.llm.base_url);
    let stats_file = cli
        .stats_file
        .or_else(|| config.train.stats_file.map(PathBuf::from))
        .unwrap_or_else(|| {
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("oryx-train")
                .join("stats.jsonl")
        });

    let accent_color = cli
        .accent_color
        .as_deref()
        .or(config.train.accent_color.as_deref())
        .and_then(parse_rgb)
        .unwrap_or((0, 200, 255));

    let highlight_color = cli
        .highlight_color
        .as_deref()
        .or(config.train.highlight_color.as_deref())
        .and_then(parse_rgb)
        .unwrap_or(accent_color);

    if let Some(Commands::ClearStats) = cli.command {
        let mut cleared = 0;
        for &mode in Mode::ALL {
            let path = stats_path(&stats_file, mode);
            if path.exists() {
                std::fs::remove_file(&path)?;
                println!("Cleared: {}", path.display());
                cleared += 1;
            }
        }
        if cleared == 0 {
            println!("No stats files found.");
        }
        return Ok(());
    }

    let stats_window = cli.stats_window.or(config.train.stats_window).unwrap_or(10);

    let mut samples: Vec<String> = config.train.prompts;
    samples.extend(cli.extra_prompts);
    if samples.is_empty() {
        samples = vec![
            "Generate a typing exercise using common English words.".into(),
            "Write a short passage about nature for typing practice.".into(),
            "Create a typing exercise about everyday activities.".into(),
            "Generate a passage about technology and computers.".into(),
            "Write a short motivational passage for typing practice.".into(),
        ];
    }

    // Restore cached settings
    let cache = &persisted_config.train.mode_cache;
    let initial_mode = cache
        .last_mode
        .as_deref()
        .and_then(|n| Mode::ALL.iter().find(|&&m| m.name() == n).copied())
        .unwrap_or(Mode::Prose);
    let prose_prompt = cache.prose_prompt.clone().unwrap_or_default();
    let words_prompt = cache.words_prompt.clone().unwrap_or_default();
    let code_prompt  = cache.code_prompt.clone().unwrap_or_default();
    let initial_prompt = match initial_mode {
        Mode::Prose => prose_prompt.clone(),
        Mode::Words => words_prompt.clone(),
        Mode::Code  => code_prompt.clone(),
        _ => String::new(),
    };
    let prompt_buf = if initial_prompt.is_empty() {
        samples.first().cloned().unwrap_or_default()
    } else {
        initial_prompt
    };
    let prompt_cursor = prompt_buf.chars().count();
    let recent_stats = load_recent_stats(&stats_path(&stats_file, initial_mode), stats_window);

    let mut app = App {
        screen: Screen::Config,
        cfg: ConfigState {
            mode: initial_mode,
            lang_idx: cache.lang.unwrap_or(0),
            code_lang_idx: cache.code_lang.unwrap_or(0),
            options_idx: 0,
            word_count: cache.word_count.unwrap_or(50),
            prose_infinite: cache.prose_infinite.unwrap_or(true),
            ngram_kind: cache.ngram_kind.unwrap_or(2),
            ngram_count: cache.ngram_count.unwrap_or(50),
            symbol_len: cache.symbol_length.unwrap_or(80),
            prompt_buf,
            prompt_cursor,
            sample_idx: 0,
            focus: ConfigFocus::ModeList,
            samples,
            prose_prompt,
            words_prompt,
            code_prompt,
        },
        typing: None,
        status_msg: String::new(),
        stats_file,
        stats_save_error: None,
        provider,
        model,
        api_key,
        base_url,
        kb_tx: None,
        keymap: None,
        current_layer: 0,
        accent_color,
        highlight_color,
        stats_window,
        recent_stats,
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<AppMsg>();

    // Try to connect to the keyboard in the background; app works without it.
    app.kb_tx = spawn_keyboard_task(tx.clone());

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));

    let result: Result<()> = async {
        loop {
            terminal.draw(|f| draw(f, &app))?;

            tokio::select! {
                _ = tick.tick() => {
                    // Refill buffer for infinite mode.
                    let needs = app.typing.as_ref().map(|s| s.needs_refill()).unwrap_or(false);
                    if needs {
                        if let Some(ref mut s) = app.typing {
                            s.streaming = true;
                            s.stream_done = false;
                            let sys = s.gen_system.clone();
                            let user = s.gen_user.clone();
                            spawn_llm_stream(
                                tx.clone(), sys, user,
                                app.provider.clone(), app.model.clone(),
                                app.api_key.clone(), app.base_url.clone(),
                            );
                        }
                    }
                }

                Some(msg) = rx.recv() => {
                    match msg {
                        AppMsg::TextChunk(chunk) => {
                            {
                                if let Some(ref mut s) = app.typing {
                                    s.append_text(&chunk);
                                }
                            }
                            update_kbd_led(&app);
                        }
                        AppMsg::StreamDone => {
                            if let Some(ref mut s) = app.typing {
                                s.streaming = false;
                                s.stream_done = true;
                            }
                        }
                        AppMsg::LlmError(e) => {
                            app.status_msg = format!("Error: {e}");
                            if app.screen == Screen::Typing {
                                if let Some(ref s) = app.typing {
                                    if s.target.is_empty() {
                                        app.screen = Screen::Config;
                                        app.typing = None;
                                    }
                                }
                            } else {
                                app.screen = Screen::Config;
                            }
                        }
                        AppMsg::LayerChanged(n) => {
                            app.current_layer = n;
                            update_kbd_led(&app);
                        }
                        AppMsg::KbReady(km) => {
                            app.keymap = Some(km);
                            update_kbd_led(&app);
                        }
                        AppMsg::KbGone => {
                            app.keymap = None;
                            app.kb_tx = None;
                        }
                    }
                }

                Some(Ok(CEvent::Key(key))) = events.next() => {
                    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

                    if ctrl && key.code == KeyCode::Char('c') {
                        break;
                    }

                    match app.screen {
                        Screen::Config => {
                            let cfg = &mut app.cfg;
                            match key.code {
                                KeyCode::Esc => break,
                                KeyCode::Tab => {
                                    cfg.focus = match cfg.focus {
                                        ConfigFocus::ModeList => ConfigFocus::Options,
                                        ConfigFocus::Options
                                            if shows_prompt_input(cfg.mode) =>
                                        {
                                            ConfigFocus::Prompt
                                        }
                                        ConfigFocus::Options | ConfigFocus::Prompt => {
                                            ConfigFocus::ModeList
                                        }
                                    };
                                }
                                KeyCode::Enter => {
                                    let prompt_ok = !shows_prompt_input(cfg.mode)
                                        || !cfg.prompt_buf.trim().is_empty();
                                    if prompt_ok {
                                        start_session(&mut app, &tx);
                                    }
                                }
                                _ => match cfg.focus {
                                    ConfigFocus::ModeList => match key.code {
                                        KeyCode::Up => {
                                            let idx = cfg.mode.index();
                                            if idx > 0 {
                                                // Save current prompt
                                                match cfg.mode {
                                                    Mode::Prose => cfg.prose_prompt = cfg.prompt_buf.clone(),
                                                    Mode::Words => cfg.words_prompt = cfg.prompt_buf.clone(),
                                                    Mode::Code  => cfg.code_prompt  = cfg.prompt_buf.clone(),
                                                    _ => {}
                                                }
                                                cfg.mode = Mode::ALL[idx - 1];
                                                cfg.options_idx = 0;
                                                // Restore prompt for new mode
                                                cfg.prompt_buf = match cfg.mode {
                                                    Mode::Prose => cfg.prose_prompt.clone(),
                                                    Mode::Words => cfg.words_prompt.clone(),
                                                    Mode::Code  => cfg.code_prompt.clone(),
                                                    _ => String::new(),
                                                };
                                                cfg.prompt_cursor = cfg.prompt_buf.chars().count();
                                                let mode = cfg.mode;
                                                app.recent_stats = load_recent_stats(&stats_path(&app.stats_file, mode), app.stats_window);
                                            }
                                        }
                                        KeyCode::Down => {
                                            let idx = cfg.mode.index();
                                            if idx + 1 < Mode::ALL.len() {
                                                // Save current prompt
                                                match cfg.mode {
                                                    Mode::Prose => cfg.prose_prompt = cfg.prompt_buf.clone(),
                                                    Mode::Words => cfg.words_prompt = cfg.prompt_buf.clone(),
                                                    Mode::Code  => cfg.code_prompt  = cfg.prompt_buf.clone(),
                                                    _ => {}
                                                }
                                                cfg.mode = Mode::ALL[idx + 1];
                                                cfg.options_idx = 0;
                                                // Restore prompt for new mode
                                                cfg.prompt_buf = match cfg.mode {
                                                    Mode::Prose => cfg.prose_prompt.clone(),
                                                    Mode::Words => cfg.words_prompt.clone(),
                                                    Mode::Code  => cfg.code_prompt.clone(),
                                                    _ => String::new(),
                                                };
                                                cfg.prompt_cursor = cfg.prompt_buf.chars().count();
                                                let mode = cfg.mode;
                                                app.recent_stats = load_recent_stats(&stats_path(&app.stats_file, mode), app.stats_window);
                                            }
                                        }
                                        _ => {}
                                    },
                                    ConfigFocus::Options => match key.code {
                                        KeyCode::Up => {
                                            if cfg.options_idx > 0 {
                                                cfg.options_idx -= 1;
                                            }
                                        }
                                        KeyCode::Down => {
                                            if cfg.options_idx + 1 < cfg.option_count() {
                                                cfg.options_idx += 1;
                                            }
                                        }
                                        KeyCode::Left  => cfg.adjust_option(-1),
                                        KeyCode::Right => cfg.adjust_option(1),
                                        _ => {}
                                    },
                                    ConfigFocus::Prompt => match key.code {
                                        KeyCode::Up => {
                                            if cfg.sample_idx > 0 {
                                                cfg.load_sample(cfg.sample_idx - 1);
                                            }
                                        }
                                        KeyCode::Down => {
                                            if cfg.sample_idx + 1 < cfg.samples.len() {
                                                cfg.load_sample(cfg.sample_idx + 1);
                                            }
                                        }
                                        KeyCode::Left => {
                                            if cfg.prompt_cursor > 0 {
                                                cfg.prompt_cursor -= 1;
                                            }
                                        }
                                        KeyCode::Right => {
                                            if cfg.prompt_cursor
                                                < cfg.prompt_buf.chars().count()
                                            {
                                                cfg.prompt_cursor += 1;
                                            }
                                        }
                                        KeyCode::Home | KeyCode::Char('a') if ctrl => {
                                            cfg.prompt_cursor = 0;
                                        }
                                        KeyCode::End | KeyCode::Char('e') if ctrl => {
                                            cfg.prompt_cursor = cfg.prompt_buf.chars().count();
                                        }
                                        KeyCode::Char(c) => cfg.insert_char(c),
                                        KeyCode::Backspace => cfg.delete_before_cursor(),
                                        _ => {}
                                    },
                                },
                            }
                        }

                        Screen::Typing => match key.code {
                            KeyCode::Esc => {
                                let waiting = app
                                    .typing
                                    .as_ref()
                                    .map(|s| s.target.is_empty())
                                    .unwrap_or(true);
                                if waiting {
                                    app.typing = None;
                                    app.screen = Screen::Config;
                                } else {
                                    if let Some(ref mut s) = app.typing {
                                        s.force_done();
                                        let mode = s.mode;
                                        let stats = s.to_stats();
                                        match save_stats(&stats_path(&app.stats_file, mode), &stats) {
                                            Ok(()) => app.stats_save_error = None,
                                            Err(e) => {
                                                app.stats_save_error = Some(e.to_string())
                                            }
                                        }
                                    }
                                    app.screen = Screen::Done;
                                }
                            }
                            KeyCode::Backspace => {
                                if let Some(ref mut s) = app.typing {
                                    if s.mode != Mode::Flash {
                                        s.backspace();
                                    }
                                }
                                update_kbd_led(&app);
                            }
                            KeyCode::Char(c) => {
                                let mut complete = false;
                                {
                                    if let Some(ref mut s) = app.typing {
                                        if s.mode == Mode::Flash {
                                            let pos = s.typed.len();
                                            if pos < s.target.len() {
                                                if c == s.target[pos] {
                                                    if let Some(t) = s.char_shown_at {
                                                        let ms = t.elapsed().as_secs_f64() * 1000.0;
                                                        s.reaction_times_ms.push(ms);
                                                        s.reaction_by_symbol.entry(c).or_default().push(ms);
                                                    }
                                                    s.last_was_error = false;
                                                    s.type_char(c);
                                                    s.char_shown_at = Some(Instant::now());
                                                    complete = s.is_complete();
                                                } else {
                                                    s.error_count += 1;
                                                    s.last_was_error = true;
                                                    if s.start_time.is_none() && !s.target.is_empty() {
                                                        s.start_time = Some(Instant::now());
                                                    }
                                                }
                                            }
                                        } else {
                                            s.type_char(c);
                                            complete = s.is_complete();
                                        }
                                    }
                                }
                                update_kbd_led(&app);
                                if complete {
                                    let mode = app.typing.as_ref().unwrap().mode;
                                    let stats = app.typing.as_ref().unwrap().to_stats();
                                    match save_stats(&stats_path(&app.stats_file, mode), &stats) {
                                        Ok(()) => app.stats_save_error = None,
                                        Err(e) => app.stats_save_error = Some(e.to_string()),
                                    }
                                    app.screen = Screen::Done;
                                }
                            }
                            _ => {}
                        },

                        Screen::Done => match key.code {
                            KeyCode::Char('q') => break,
                            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('r') => {
                                let mode = app.cfg.mode;
                                app.recent_stats = load_recent_stats(&stats_path(&app.stats_file, mode), app.stats_window);
                                app.typing = None;
                                app.stats_save_error = None;
                                app.screen = Screen::Config;
                            }
                            _ => {}
                        },
                    }
                }
            }
        }
        Ok(())
    }
    .await;

    // Restore terminal.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    // Return keyboard LEDs to firmware control.
    if let Some(ref kb_tx) = app.kb_tx {
        let _ = kb_tx.send(KbCmd::ClearAll);
    }

    // Save mode cache to config file.
    {
        let cfg = &app.cfg;
        // Make sure the current mode's prompt is saved.
        let (prose_p, words_p, code_p) = match cfg.mode {
            Mode::Prose => (cfg.prompt_buf.clone(), cfg.words_prompt.clone(), cfg.code_prompt.clone()),
            Mode::Words => (cfg.prose_prompt.clone(), cfg.prompt_buf.clone(), cfg.code_prompt.clone()),
            Mode::Code  => (cfg.prose_prompt.clone(), cfg.words_prompt.clone(), cfg.prompt_buf.clone()),
            _ => (cfg.prose_prompt.clone(), cfg.words_prompt.clone(), cfg.code_prompt.clone()),
        };
        persisted_config.train.mode_cache = ModeCache {
            last_mode: Some(cfg.mode.name().to_string()),
            lang: Some(cfg.lang_idx),
            word_count: Some(cfg.word_count),
            prose_infinite: Some(cfg.prose_infinite),
            prose_prompt: Some(prose_p),
            words_prompt: Some(words_p),
            symbol_length: Some(cfg.symbol_len),
            ngram_kind: Some(cfg.ngram_kind),
            ngram_count: Some(cfg.ngram_count),
            code_lang: Some(cfg.code_lang_idx),
            code_prompt: Some(code_p),
        };
        let _ = save_config(&config_path, &persisted_config);
    }

    result
}
