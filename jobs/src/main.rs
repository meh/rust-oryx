use std::collections::HashMap;
use std::future::pending;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Parser;
use oryx_hid::{asynchronous::Event as KbdEvent, asynchronous::OryxKeyboard};
use serde::Deserialize;
use tokio::sync::{Mutex, mpsc};
use zbus::{connection, interface, object_server::SignalEmitter, zvariant::{OwnedValue, Value}};

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Parser)]
struct Cli {
    /// Path to config file
    #[arg(long, short)]
    config: Option<String>,

    /// LED index to use as a job slot (repeatable; overrides config file slots)
    #[arg(long, short)]
    slot: Vec<u8>,
}

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Deserialize, Clone, Copy, Debug)]
#[serde(try_from = "String")]
struct Rgb(u8, u8, u8);

impl TryFrom<String> for Rgb {
    type Error = String;
    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        let s = s.trim_start_matches('#');
        if s.len() != 6 {
            return Err(format!("expected #RRGGBB, got #{s}"));
        }
        let r = u8::from_str_radix(&s[0..2], 16).map_err(|e| e.to_string())?;
        let g = u8::from_str_radix(&s[2..4], 16).map_err(|e| e.to_string())?;
        let b = u8::from_str_radix(&s[4..6], 16).map_err(|e| e.to_string())?;
        Ok(Rgb(r, g, b))
    }
}

#[derive(Deserialize, Default, Clone)]
struct ProgressColors {
    start: Option<Rgb>,
    end: Option<Rgb>,
}

#[derive(Deserialize, Clone)]
#[serde(untagged)]
enum FinishedMatchValue {
    Int(i64),
    Str(String),
}

#[derive(Deserialize, Clone)]
struct FinishedMatch {
    value: FinishedMatchValue,
    color: Rgb,
}

#[derive(Deserialize, Default, Clone)]
struct FinishedColors {
    #[serde(default)]
    matches: Vec<FinishedMatch>,
    default: Option<Rgb>,
}

#[derive(Deserialize, Clone)]
struct StageMatch {
    name: String,
    color: Rgb,
}

#[derive(Deserialize, Default, Clone)]
struct StageColors {
    #[serde(default)]
    matches: Vec<StageMatch>,
    default: Option<Rgb>,
}

#[derive(Deserialize, Default, Clone)]
struct Colors {
    idle: Option<Rgb>,
    started: Option<Rgb>,
    #[serde(default)]
    progress: ProgressColors,
    #[serde(default)]
    finished: FinishedColors,
    #[serde(default)]
    stage: StageColors,
}

#[derive(Deserialize, Default)]
struct Config {
    slots: Vec<u8>,
    #[serde(default)]
    colors: Colors,
}

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
enum JobState {
    Created { metadata: HashMap<String, OwnedValue> },
    Started,
    Progress { current: u32, total: u32 },
    Stage(String),
    Finished(OwnedValue),
}

struct Slot {
    led: u8,
    #[allow(dead_code)]
    col: u8,
    #[allow(dead_code)]
    row: u8,
    job_id: Option<u32>,
    state: Option<JobState>,
}

struct JobManager {
    slots: Vec<Slot>,
    next_job_id: u32,
    job_to_slot: HashMap<u32, usize>,
    key_to_slot: HashMap<(u8, u8), usize>,
    colors: Colors,
}

impl JobManager {
    fn new(config: &Config) -> Result<Self> {
        let mut slots = Vec::with_capacity(config.slots.len());
        let mut key_to_slot = HashMap::new();

        for (i, &led) in config.slots.iter().enumerate() {
            let (col, row) = oryx_hid::matrix::led_to_pos(led)
                .with_context(|| format!("LED index {led} has no known matrix position"))?;
            key_to_slot.insert((col, row), i);
            slots.push(Slot {
                led,
                col,
                row,
                job_id: None,
                state: None,
            });
        }

        Ok(Self {
            slots,
            next_job_id: 1,
            job_to_slot: HashMap::new(),
            key_to_slot,
            colors: config.colors.clone(),
        })
    }

    fn alloc_slot(&mut self, metadata: HashMap<String, OwnedValue>) -> Option<(u32, u8)> {
        let slot_i = self.slots.iter().position(|s| s.job_id.is_none())?;
        let id = self.next_job_id;
        self.next_job_id += 1;
        self.slots[slot_i].job_id = Some(id);
        self.slots[slot_i].state = Some(JobState::Created { metadata });
        self.job_to_slot.insert(id, slot_i);
        let led = self.slots[slot_i].led;
        Some((id, led))
    }

    fn set_state(&mut self, job_id: u32, state: JobState) -> Option<(u8, (u8, u8, u8))> {
        let &slot_i = self.job_to_slot.get(&job_id)?;
        self.slots[slot_i].state = Some(state.clone());
        let led = self.slots[slot_i].led;
        Some((led, resolve_color(Some(&state), &self.colors)))
    }

    fn get_state(&self, job_id: u32) -> Option<&JobState> {
        let &slot_i = self.job_to_slot.get(&job_id)?;
        self.slots[slot_i].state.as_ref()
    }

    /// Called on KeyUp for a slot. Returns the cleared job_id if it was Finished.
    fn try_clear(&mut self, col: u8, row: u8) -> Option<(u32, u8)> {
        let &slot_i = self.key_to_slot.get(&(col, row))?;
        if !matches!(self.slots[slot_i].state, Some(JobState::Finished(_))) {
            return None;
        }
        let job_id = self.slots[slot_i].job_id.take()?;
        self.slots[slot_i].state = None;
        self.job_to_slot.remove(&job_id);
        let led = self.slots[slot_i].led;
        Some((job_id, led))
    }
}

// ── Color resolution ──────────────────────────────────────────────────────────

fn match_finished_value(v: &OwnedValue, m: &FinishedMatchValue) -> bool {
    match (m, &**v) {
        (FinishedMatchValue::Int(n), Value::I32(x))  => *x as i64 == *n,
        (FinishedMatchValue::Int(n), Value::U32(x))  => *x as i64 == *n,
        (FinishedMatchValue::Int(n), Value::I64(x))  => *x == *n,
        (FinishedMatchValue::Int(n), Value::U64(x))  => *x as i64 == *n,
        (FinishedMatchValue::Int(n), Value::I16(x))  => *x as i64 == *n,
        (FinishedMatchValue::Int(n), Value::U16(x))  => *x as i64 == *n,
        (FinishedMatchValue::Int(n), Value::U8(x))   => *x as i64 == *n,
        (FinishedMatchValue::Str(s), Value::Str(x))  => x.as_str() == s,
        _ => false,
    }
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

fn lerp_rgb(a: Rgb, b: Rgb, t: f32) -> (u8, u8, u8) {
    (
        lerp_u8(a.0, b.0, t),
        lerp_u8(a.1, b.1, t),
        lerp_u8(a.2, b.2, t),
    )
}

fn resolve_color(state: Option<&JobState>, colors: &Colors) -> (u8, u8, u8) {
    match state {
        None | Some(JobState::Created { .. }) => {
            let c = colors.idle.unwrap_or(Rgb(0, 0, 0));
            (c.0, c.1, c.2)
        }
        Some(JobState::Started) => {
            let c = colors.started.unwrap_or(Rgb(0, 100, 255));
            (c.0, c.1, c.2)
        }
        Some(JobState::Progress { current, total }) => {
            let t = if *total == 0 {
                0.0
            } else {
                (*current as f32 / *total as f32).clamp(0.0, 1.0)
            };
            let start = colors.progress.start.unwrap_or(Rgb(0, 100, 255));
            let end = colors.progress.end.unwrap_or(Rgb(0, 255, 100));
            lerp_rgb(start, end, t)
        }
        Some(JobState::Stage(name)) => colors
            .stage
            .matches
            .iter()
            .find(|m| m.name == *name)
            .map(|m| (m.color.0, m.color.1, m.color.2))
            .or_else(|| colors.stage.default.map(|c| (c.0, c.1, c.2)))
            .unwrap_or((255, 200, 0)),
        Some(JobState::Finished(v)) => colors
            .finished
            .matches
            .iter()
            .find(|m| match_finished_value(v, &m.value))
            .map(|m| (m.color.0, m.color.1, m.color.2))
            .or_else(|| colors.finished.default.map(|c| (c.0, c.1, c.2)))
            .unwrap_or((180, 180, 180)),
    }
}

// ── State → signal conversion ─────────────────────────────────────────────────

fn state_to_signal(state: Option<&JobState>) -> (&'static str, HashMap<String, OwnedValue>) {
    match state {
        None => ("cleared", HashMap::new()),
        Some(JobState::Created { metadata }) => ("created", metadata.clone()),
        Some(JobState::Started) => ("started", HashMap::new()),
        Some(JobState::Progress { current, total }) => (
            "progress",
            [
                ("current".to_string(), OwnedValue::from(*current)),
                ("total".to_string(), OwnedValue::from(*total)),
            ]
            .into(),
        ),
        Some(JobState::Stage(name)) => (
            "stage",
            [("name".to_string(), OwnedValue::from(zbus::zvariant::Str::from(name.as_str())))].into(),
        ),
        Some(JobState::Finished(value)) => (
            "finished",
            [("value".to_string(), value.clone())].into(),
        ),
    }
}

// ── Keyboard command channel ──────────────────────────────────────────────────

enum KbdCmd {
    SetRgb { index: u8, r: u8, g: u8, b: u8 },
}

// ── DBus interface ────────────────────────────────────────────────────────────

struct Jobs {
    state: Arc<Mutex<JobManager>>,
    cmd_tx: mpsc::Sender<KbdCmd>,
    status_tx: mpsc::Sender<(u32, Option<JobState>)>,
}

#[interface(name = "zsa.oryx.Jobs")]
impl Jobs {
    async fn create(
        &self,
        metadata: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<u32> {
        let mut st = self.state.lock().await;
        let (id, led) = st
            .alloc_slot(metadata.clone())
            .ok_or_else(|| zbus::fdo::Error::Failed("no free slots".into()))?;
        let (r, g, b) = resolve_color(st.slots[*st.job_to_slot.get(&id).unwrap()].state.as_ref(), &st.colors);
        drop(st);
        let _ = self.cmd_tx.send(KbdCmd::SetRgb { index: led, r, g, b }).await;
        let _ = self.status_tx.send((id, Some(JobState::Created { metadata }))).await;
        Ok(id)
    }

    async fn start(&self, job_id: u32) -> zbus::fdo::Result<()> {
        self.update(job_id, JobState::Started).await
    }

    async fn progress(&self, job_id: u32, current: u32, total: u32) -> zbus::fdo::Result<()> {
        self.update(job_id, JobState::Progress { current, total }).await
    }

    async fn stage(&self, job_id: u32, name: String) -> zbus::fdo::Result<()> {
        self.update(job_id, JobState::Stage(name)).await
    }

    async fn finish(&self, job_id: u32, value: OwnedValue) -> zbus::fdo::Result<()> {
        self.update(job_id, JobState::Finished(value)).await
    }

    async fn get_state(
        &self,
        job_id: u32,
    ) -> zbus::fdo::Result<(String, HashMap<String, OwnedValue>)> {
        let st = self.state.lock().await;
        let job_state = st
            .get_state(job_id)
            .ok_or_else(|| zbus::fdo::Error::Failed(format!("unknown job {job_id}")))?;
        let (state_str, meta) = state_to_signal(Some(job_state));
        Ok((state_str.to_string(), meta))
    }

    #[zbus(signal)]
    async fn state(
        emitter: &SignalEmitter<'_>,
        job_id: u32,
        state: &str,
        metadata: HashMap<String, OwnedValue>,
    ) -> zbus::Result<()>;
}

impl Jobs {
    async fn update(&self, job_id: u32, state: JobState) -> zbus::fdo::Result<()> {
        let mut st = self.state.lock().await;
        let (led, (r, g, b)) = st
            .set_state(job_id, state.clone())
            .ok_or_else(|| zbus::fdo::Error::Failed(format!("unknown job {job_id}")))?;
        drop(st);
        let _ = self.cmd_tx.send(KbdCmd::SetRgb { index: led, r, g, b }).await;
        let _ = self.status_tx.send((job_id, Some(state))).await;
        Ok(())
    }
}

// ── Keyboard task ─────────────────────────────────────────────────────────────

async fn keyboard_task(
    mut kbd: OryxKeyboard,
    mut cmd_rx: mpsc::Receiver<KbdCmd>,
    status_tx: mpsc::Sender<(u32, Option<JobState>)>,
    cmd_tx: mpsc::Sender<KbdCmd>,
    state: Arc<Mutex<JobManager>>,
) {
    loop {
        tokio::select! {
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    KbdCmd::SetRgb { index, r, g, b } => {
                        let _ = kbd.rgb(index, r, g, b).await;
                    }
                }
            }
            result = kbd.recv_event() => {
                match result {
                    Ok(KbdEvent::KeyUp { col, row }) => {
                        let mut st = state.lock().await;
                        if let Some((job_id, led)) = st.try_clear(col, row) {
                            drop(st);
                            let _ = status_tx.send((job_id, None)).await;
                            let _ = cmd_tx.send(KbdCmd::SetRgb { index: led, r: 0, g: 0, b: 0 }).await;
                        }
                    }
                    Err(_) => break,
                    _ => {}
                }
            }
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config_path = match cli.config {
        Some(p) => std::path::PathBuf::from(p),
        None => dirs::config_dir()
            .context("could not determine config directory")?
            .join("oryx/jobs/config.toml"),
    };

    let mut config: Config = match std::fs::read_to_string(&config_path) {
        Ok(s) => toml::from_str(&s).context("parsing config")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => return Err(e).with_context(|| format!("reading config from {}", config_path.display())),
    };

    if !cli.slot.is_empty() {
        config.slots = cli.slot;
    }

    if config.slots.is_empty() {
        bail!("no slots configured; use --slot <LED> or set slots in the config file");
    }

    let state = Arc::new(Mutex::new(JobManager::new(&config)?));

    let mut kbd = OryxKeyboard::open().await.context("opening keyboard")?;
    kbd.rgb_control(true)
        .await
        .context("enabling RGB control")?;

    let (cmd_tx, cmd_rx) = mpsc::channel::<KbdCmd>(32);
    let (status_tx, mut status_rx) = mpsc::channel::<(u32, Option<JobState>)>(32);

    tokio::spawn(keyboard_task(
        kbd,
        cmd_rx,
        status_tx.clone(),
        cmd_tx.clone(),
        state.clone(),
    ));

    let jobs = Jobs { state, cmd_tx, status_tx };

    let conn = connection::Builder::session()
        .context("creating session bus")?
        .name("zsa.oryx.Jobs")
        .context("claiming bus name")?
        .serve_at("/zsa/oryx/Jobs", jobs)
        .context("registering object")?
        .build()
        .await
        .context("building connection")?;

    let iface_ref = conn
        .object_server()
        .interface::<_, Jobs>("/zsa/oryx/Jobs")
        .await
        .context("getting interface ref")?;

    tokio::spawn(async move {
        while let Some((job_id, job_state)) = status_rx.recv().await {
            let emitter = iface_ref.signal_emitter();
            let (state_str, meta) = state_to_signal(job_state.as_ref());
            let _ = Jobs::state(emitter, job_id, state_str, meta).await;
        }
    });

    pending::<()>().await;
    Ok(())
}
