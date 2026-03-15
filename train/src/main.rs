use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use clap::Parser;
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

// ── Constants ──────────────────────────────────────────────────────────────────

const DEFAULT_SAMPLES: &[&str] = &[
    "Generate a 100-word typing exercise using common English words and phrases.",
    "Write a short passage about nature suitable for typing practice, about 80 words.",
    "Create a typing exercise about everyday activities using simple sentences, about 100 words.",
    "Generate a fun typing passage about technology and computers, about 100 words.",
    "Write a short motivational passage for typing practice, about 80 words.",
];

const LLM_SYSTEM_PROMPT: &str =
    "You are a typing practice text generator. Generate clean, plain text suitable for \
     typing practice. Use only standard ASCII characters and common punctuation. Do not use \
     markdown, code blocks, bullet points, numbered lists, or special formatting. Do not \
     include any introduction, explanation, or preamble — output only the typing practice text.";

// ── Config ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Default)]
struct Config {
    #[serde(default)]
    llm: LlmConfig,
    #[serde(default)]
    train: TrainConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct LlmConfig {
    provider: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct TrainConfig {
    #[serde(default)]
    prompts: Vec<String>,
    stats_file: Option<String>,
}

fn default_stats_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("oryx-train")
        .join("stats.jsonl")
}

fn load_config(path: &PathBuf) -> Config {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

// ── CLI ────────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(about = "Typing trainer powered by LLM")]
struct Cli {
    /// LLM provider (openai, anthropic, ollama, groq, deepseek, google, mistral)
    #[arg(long, env = "ORYX_TRAIN_PROVIDER")]
    provider: Option<String>,

    /// Model name
    #[arg(long, env = "ORYX_TRAIN_MODEL")]
    model: Option<String>,

    /// API key
    #[arg(long, env = "ORYX_TRAIN_API_KEY")]
    api_key: Option<String>,

    /// API base URL (e.g. for Ollama: http://localhost:11434)
    #[arg(long, env = "ORYX_TRAIN_BASE_URL")]
    base_url: Option<String>,

    /// Config file path (default: ~/.config/oryx-train/config.toml)
    #[arg(long)]
    config: Option<PathBuf>,

    /// Sample prompt to display (can be repeated)
    #[arg(long = "prompt", value_name = "PROMPT")]
    extra_prompts: Vec<String>,

    /// File to append typing statistics to
    #[arg(long, env = "ORYX_TRAIN_STATS_FILE")]
    stats_file: Option<PathBuf>,
}

// ── Provider parsing ───────────────────────────────────────────────────────────

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
            "Unknown provider: '{}'. Supported: openai, anthropic, ollama, groq, deepseek, google, mistral, openrouter, cohere",
            other
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

// ── Stats ──────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct SessionStats {
    timestamp_unix: u64,
    duration_secs: f64,
    wpm: f64,
    accuracy: f64,
    errors: usize,
    correct: usize,
    text_length: usize,
    prompt: String,
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

// ── Typing session ─────────────────────────────────────────────────────────────

struct TypingSession {
    target: Vec<char>,
    typed: Vec<char>,
    start_time: Option<Instant>,
    finish_time: Option<Instant>,
    error_count: usize,
    prompt: String,
}

impl TypingSession {
    fn new(text: String, prompt: String) -> Self {
        let text = text.trim().replace("\r\n", "\n").replace('\r', "\n");
        Self {
            target: text.chars().collect(),
            typed: Vec::new(),
            start_time: None,
            finish_time: None,
            error_count: 0,
            prompt,
        }
    }

    fn is_complete(&self) -> bool {
        !self.target.is_empty() && self.typed.len() == self.target.len()
    }

    fn elapsed(&self) -> Duration {
        match (self.start_time, self.finish_time) {
            (Some(start), Some(end)) => end.duration_since(start),
            (Some(start), None) => start.elapsed(),
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
        if self.start_time.is_none() {
            self.start_time = Some(Instant::now());
        }
        let expected = self.target[self.typed.len()];
        if c != expected {
            self.error_count += 1;
        }
        self.typed.push(c);
        if self.is_complete() {
            self.finish_time = Some(Instant::now());
        }
    }

    fn backspace(&mut self) {
        self.typed.pop();
    }

    fn to_stats(&self) -> SessionStats {
        SessionStats {
            timestamp_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            duration_secs: self.elapsed().as_secs_f64(),
            wpm: self.wpm(),
            accuracy: self.accuracy(),
            errors: self.error_count,
            correct: self.correct_count(),
            text_length: self.target.len(),
            prompt: self.prompt.clone(),
        }
    }
}

// ── Text rendering ─────────────────────────────────────────────────────────────

fn char_style(i: usize, pos: usize, typed: &[char], target: &[char]) -> Style {
    if i < pos {
        if typed[i] == target[i] {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Red).add_modifier(Modifier::UNDERLINED)
        }
    } else if i == pos {
        Style::default().fg(Color::White).bg(Color::DarkGray)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

/// Build display lines for the typing area.
/// Returns (lines, cursor_line_index) where cursor_line_index is the line containing the cursor.
fn build_typed_lines(session: &TypingSession, width: u16) -> (Vec<Line<'static>>, u16) {
    let w = (width as usize).saturating_sub(2).max(20);
    let pos = session.typed.len();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cursor_line: u16 = 0;
    let mut col = 0usize;
    let mut cur_text = String::new();
    let mut cur_style = Style::default();
    let mut spans: Vec<Span<'static>> = Vec::new();

    for (i, &ch) in session.target.iter().enumerate() {
        if i == pos {
            cursor_line = lines.len() as u16;
        }

        let needs_wrap = ch == '\n' || col >= w;
        if needs_wrap {
            if !cur_text.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut cur_text), cur_style));
            }
            lines.push(Line::from(std::mem::take(&mut spans)));
            col = 0;
            if ch == '\n' {
                continue;
            }
        }

        let style = char_style(i, pos, &session.typed, &session.target);
        if style != cur_style && !cur_text.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut cur_text), cur_style));
        }
        cur_style = style;
        cur_text.push(ch);
        col += 1;
    }

    if !cur_text.is_empty() {
        spans.push(Span::styled(cur_text, cur_style));
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }

    if pos == session.target.len() && !lines.is_empty() {
        cursor_line = (lines.len() - 1) as u16;
    }

    (lines, cursor_line)
}

// ── App state ──────────────────────────────────────────────────────────────────

#[derive(PartialEq)]
enum Screen {
    PromptSelect,
    Generating,
    Typing,
    Done,
}

enum AppMsg {
    LlmResponse(String),
    LlmError(String),
}

struct App {
    screen: Screen,
    samples: Vec<String>,
    sample_index: usize,
    prompt_buf: String,
    prompt_cursor: usize,
    typing: Option<TypingSession>,
    status_msg: String,
    stats_file: PathBuf,
    stats_save_error: Option<String>,
    provider: String,
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
}

impl App {
    fn load_sample(&mut self, idx: usize) {
        if idx < self.samples.len() {
            self.sample_index = idx;
            self.prompt_buf = self.samples[idx].clone();
            self.prompt_cursor = self.prompt_buf.chars().count();
        }
    }

    fn insert_char(&mut self, c: char) {
        let byte_pos = char_to_byte(&self.prompt_buf, self.prompt_cursor);
        self.prompt_buf.insert(byte_pos, c);
        self.prompt_cursor += 1;
    }

    fn delete_before_cursor(&mut self) {
        if self.prompt_cursor > 0 {
            let byte_end = char_to_byte(&self.prompt_buf, self.prompt_cursor);
            let byte_start = char_to_byte(&self.prompt_buf, self.prompt_cursor - 1);
            self.prompt_buf.drain(byte_start..byte_end);
            self.prompt_cursor -= 1;
        }
    }
}

fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

// ── LLM call ──────────────────────────────────────────────────────────────────

fn spawn_llm_task(
    tx: mpsc::UnboundedSender<AppMsg>,
    prompt: String,
    provider: String,
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
) {
    tokio::spawn(async move {
        match call_llm(prompt, provider, model, api_key, base_url).await {
            Ok(text) => {
                let _ = tx.send(AppMsg::LlmResponse(text));
            }
            Err(e) => {
                let _ = tx.send(AppMsg::LlmError(e.to_string()));
            }
        }
    });
}

async fn call_llm(
    prompt: String,
    provider: String,
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
) -> Result<String> {
    let backend = parse_backend(&provider)?;
    let mut builder = LLMBuilder::new()
        .backend(backend)
        .model(model)
        .system(LLM_SYSTEM_PROMPT)
        .max_tokens(512);
    if let Some(key) = api_key {
        builder = builder.api_key(key);
    }
    if let Some(url) = base_url {
        builder = builder.base_url(url);
    }
    let llm = builder.build().map_err(|e| anyhow::anyhow!("{e}"))?;
    let messages = vec![ChatMessage::user().content(prompt).build()];
    let response = llm
        .chat(&messages)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    response
        .text()
        .ok_or_else(|| anyhow::anyhow!("LLM returned empty response"))
}

// ── Drawing ────────────────────────────────────────────────────────────────────

fn draw(f: &mut Frame, app: &App) {
    match app.screen {
        Screen::PromptSelect => draw_prompt(f, app),
        Screen::Generating => draw_generating(f, app),
        Screen::Typing => draw_typing(f, app),
        Screen::Done => draw_done(f, app),
    }
}

fn draw_prompt(f: &mut Frame, app: &App) {
    let area = f.area();
    let error_height = if app.status_msg.is_empty() { 0 } else { 1 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(error_height),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(area);

    // Build prompt display with cursor
    let chars: Vec<char> = app.prompt_buf.chars().collect();
    let before: String = chars[..app.prompt_cursor].iter().collect();
    let cursor_ch: String = chars
        .get(app.prompt_cursor)
        .map(|c| c.to_string())
        .unwrap_or_else(|| " ".into());
    let after_start = (app.prompt_cursor + 1).min(chars.len());
    let after: String = chars[after_start..].iter().collect();

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(before),
            Span::styled(cursor_ch, Style::default().fg(Color::Black).bg(Color::White)),
            Span::raw(after),
        ]))
        .block(Block::default().borders(Borders::ALL).title(" Prompt "))
        .wrap(Wrap { trim: false }),
        chunks[0],
    );

    // Sample list
    let items: Vec<ListItem> = app
        .samples
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let max = 72usize;
            let display: String = if s.chars().count() > max {
                format!("{}…", s.chars().take(max).collect::<String>())
            } else {
                s.clone()
            };
            let style = if i == app.sample_index {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            ListItem::new(display).style(style)
        })
        .collect();

    // Error message (only rendered if non-empty)
    if !app.status_msg.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                app.status_msg.as_str(),
                Style::default().fg(Color::Red),
            ))),
            chunks[1],
        );
    }

    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(" Samples ")),
        chunks[2],
    );

    // Help bar
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("[↑↓]", Style::default().fg(Color::Yellow)),
            Span::raw(" cycle samples  "),
            Span::styled("[←→]", Style::default().fg(Color::Yellow)),
            Span::raw(" move cursor  "),
            Span::styled("[Enter]", Style::default().fg(Color::Yellow)),
            Span::raw(" generate  "),
            Span::styled("[Esc]", Style::default().fg(Color::Yellow)),
            Span::raw(" quit"),
        ])),
        chunks[3],
    );
}

fn draw_generating(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "Generating typing text…",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                app.status_msg.as_str(),
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "[Esc] cancel",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .block(Block::default().borders(Borders::ALL).title(" oryx-train "))
        .alignment(Alignment::Center),
        area,
    );
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn draw_typing(f: &mut Frame, app: &App) {
    let area = f.area();
    let session = match app.typing.as_ref() {
        Some(s) => s,
        None => return,
    };

    let status_text = format!(
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

    // Status bar
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            status_text,
            Style::default().fg(Color::Black).bg(Color::White),
        ))),
        chunks[0],
    );

    // Text area
    let (lines, cursor_line) = build_typed_lines(session, chunks[1].width);
    let view_h = chunks[1].height.saturating_sub(2);
    let scroll = if view_h == 0 || cursor_line < view_h / 2 {
        0
    } else {
        cursor_line.saturating_sub(view_h / 2)
    };

    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" Type "))
            .scroll((scroll, 0)),
        chunks[1],
    );

    // Help
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("[Backspace]", Style::default().fg(Color::Yellow)),
            Span::raw(" correct  "),
            Span::styled("[Esc]", Style::default().fg(Color::Yellow)),
            Span::raw(" quit"),
        ])),
        chunks[2],
    );
}

fn draw_done(f: &mut Frame, app: &App) {
    let area = f.area();
    let session = match app.typing.as_ref() {
        Some(s) => s,
        None => return,
    };

    let mut content = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Exercise Complete!",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!(
            "WPM: {:.1}   Accuracy: {:.1}%   Errors: {}   Time: {}",
            session.wpm(),
            session.accuracy() * 100.0,
            session.error_count,
            fmt_duration(session.elapsed()),
        )),
        Line::from(""),
    ];

    if let Some(ref err) = app.stats_save_error {
        content.push(Line::from(Span::styled(
            format!("Warning: could not save stats — {}", err),
            Style::default().fg(Color::Red),
        )));
    } else {
        content.push(Line::from(Span::styled(
            format!("Stats saved to {}", app.stats_file.display()),
            Style::default().fg(Color::DarkGray),
        )));
    }

    content.push(Line::from(""));
    content.push(Line::from(vec![
        Span::styled("[Enter]", Style::default().fg(Color::Yellow)),
        Span::raw(" try again  "),
        Span::styled("[q / Esc]", Style::default().fg(Color::Yellow)),
        Span::raw(" quit"),
    ]));

    f.render_widget(
        Paragraph::new(content)
            .block(Block::default().borders(Borders::ALL).title(" Results "))
            .alignment(Alignment::Center),
        area,
    );
}

// ── Main ───────────────────────────────────────────────────────────────────────

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
        .unwrap_or_else(default_stats_path);

    let mut samples: Vec<String> = config.train.prompts;
    samples.extend(cli.extra_prompts);
    if samples.is_empty() {
        samples.extend(DEFAULT_SAMPLES.iter().map(|s| s.to_string()));
    }

    let mut app = App {
        screen: Screen::PromptSelect,
        sample_index: 0,
        prompt_buf: samples.first().cloned().unwrap_or_default(),
        prompt_cursor: samples.first().map(|s| s.chars().count()).unwrap_or(0),
        samples,
        typing: None,
        status_msg: String::new(),
        stats_file,
        stats_save_error: None,
        provider,
        model,
        api_key,
        base_url,
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<AppMsg>();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));

    let result = async {
        loop {
            terminal.draw(|f| draw(f, &app))?;
            tokio::select! {
                _ = tick.tick() => {}

                Some(msg) = rx.recv() => {
                    match msg {
                        AppMsg::LlmResponse(text) => {
                            let prompt = app.prompt_buf.clone();
                            app.typing = Some(TypingSession::new(text, prompt));
                            app.screen = Screen::Typing;
                        }
                        AppMsg::LlmError(e) => {
                            app.status_msg = format!("Error: {e}");
                            app.screen = Screen::PromptSelect;
                        }
                    }
                }

                Some(Ok(CEvent::Key(key))) = events.next() => {
                    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

                    if ctrl && key.code == KeyCode::Char('c') {
                        break;
                    }

                    match app.screen {
                        Screen::PromptSelect => match key.code {
                            KeyCode::Esc => break,
                            KeyCode::Enter => {
                                let prompt = app.prompt_buf.trim().to_string();
                                if !prompt.is_empty() {
                                    app.status_msg = String::new();
                                    app.screen = Screen::Generating;
                                    spawn_llm_task(
                                        tx.clone(),
                                        prompt,
                                        app.provider.clone(),
                                        app.model.clone(),
                                        app.api_key.clone(),
                                        app.base_url.clone(),
                                    );
                                }
                            }
                            KeyCode::Up => {
                                if app.sample_index > 0 {
                                    app.load_sample(app.sample_index - 1);
                                }
                            }
                            KeyCode::Down => {
                                if app.sample_index + 1 < app.samples.len() {
                                    app.load_sample(app.sample_index + 1);
                                }
                            }
                            KeyCode::Left => {
                                if app.prompt_cursor > 0 {
                                    app.prompt_cursor -= 1;
                                }
                            }
                            KeyCode::Right => {
                                if app.prompt_cursor < app.prompt_buf.chars().count() {
                                    app.prompt_cursor += 1;
                                }
                            }
                            KeyCode::Home | KeyCode::Char('a') if ctrl => {
                                app.prompt_cursor = 0;
                            }
                            KeyCode::End | KeyCode::Char('e') if ctrl => {
                                app.prompt_cursor = app.prompt_buf.chars().count();
                            }
                            KeyCode::Char(c) => {
                                app.insert_char(c);
                            }
                            KeyCode::Backspace => {
                                app.delete_before_cursor();
                            }
                            _ => {}
                        },

                        Screen::Generating => {
                            if key.code == KeyCode::Esc {
                                app.screen = Screen::PromptSelect;
                            }
                        }

                        Screen::Typing => match key.code {
                            KeyCode::Esc => break,
                            KeyCode::Backspace => {
                                if let Some(ref mut s) = app.typing {
                                    s.backspace();
                                }
                            }
                            KeyCode::Char(c) => {
                                let mut done = false;
                                if let Some(ref mut s) = app.typing {
                                    s.type_char(c);
                                    done = s.is_complete();
                                }
                                if done {
                                    let stats = app.typing.as_ref().unwrap().to_stats();
                                    match save_stats(&app.stats_file, &stats) {
                                        Ok(()) => app.stats_save_error = None,
                                        Err(e) => app.stats_save_error = Some(e.to_string()),
                                    }
                                    app.screen = Screen::Done;
                                }
                            }
                            _ => {}
                        },

                        Screen::Done => match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Enter | KeyCode::Char('r') => {
                                app.typing = None;
                                app.stats_save_error = None;
                                app.screen = Screen::PromptSelect;
                            }
                            _ => {}
                        },
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
