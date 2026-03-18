use std::collections::HashMap;
use std::future::pending;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use oryx_hid::{asynchronous::Event as KbdEvent, asynchronous::OryxKeyboard};
use serde::Deserialize;
use tokio::sync::Notify;
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{debug, info, warn};
use zbus::{
    connection, interface,
    object_server::SignalEmitter,
    zvariant::{OwnedValue, Value},
};

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

/// Which animation to run on an LED.
#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
enum AnimKind {
    /// Brightness oscillates with a sine wave; color optionally shifts through
    /// a gradient in sync.
    Breathe,
    /// Color position bounces back and forth through a gradient (triangle wave),
    /// always at full brightness.
    Bounce,
}

/// Animation specification stored in the config and carried by `KbdCmd::Animate`.
#[derive(Deserialize, Clone, Debug)]
struct AnimSpec {
    animation: AnimKind,
    /// Single-color shorthand — equivalent to `colors = ["#RRGGBB"]`.
    #[serde(default)]
    color: Option<Rgb>,
    /// Two or more colors defining the gradient. Takes priority over `color`.
    #[serde(default)]
    colors: Vec<Rgb>,
    /// Period of one full animation cycle in milliseconds.
    #[serde(default)]
    period_ms: Option<f32>,
}

impl AnimSpec {
    /// Gradient to use: `colors` if non-empty, else `[color]`, else `[white]`.
    fn gradient(&self) -> Vec<Rgb> {
        if !self.colors.is_empty() {
            self.colors.clone()
        } else if let Some(c) = self.color {
            vec![c]
        } else {
            vec![Rgb(255, 255, 255)]
        }
    }

    fn period(&self) -> f32 {
        self.period_ms.unwrap_or(match self.animation {
            AnimKind::Breathe => 1500.0,
            AnimKind::Bounce => 2000.0,
        })
    }
}

/// Either a static color or an animation spec.
///
/// In TOML a static value is written as a hex string (`"#RRGGBB"`) while an
/// animated value is written as an inline table:
///   `{ animation = "breathe", color = "#0064FF" }`
///   `{ animation = "bounce",  colors = ["#FF0000", "#0000FF"], period_ms = 1000 }`
#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
enum ColorSpec {
    Static(Rgb),
    Animated(AnimSpec),
}

#[derive(Deserialize, Default, Clone)]
struct ProgressColors {
    /// Progress start color (0 % — lerped, always static).
    start: Option<Rgb>,
    /// Progress end color (100 % — lerped, always static).
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
    color: ColorSpec,
}

#[derive(Deserialize, Default, Clone)]
struct FinishedColors {
    #[serde(default)]
    matches: Vec<FinishedMatch>,
    default: Option<ColorSpec>,
}

#[derive(Deserialize, Clone)]
struct StageMatch {
    name: String,
    color: ColorSpec,
}

#[derive(Deserialize, Default, Clone)]
struct StageColors {
    #[serde(default)]
    matches: Vec<StageMatch>,
    default: Option<ColorSpec>,
}

#[derive(Deserialize, Default, Clone)]
struct PromptColors {
    /// Color/animation while waiting for a keyboard response.
    /// Defaults to a breathe animation at `#C800FF` for backward compat.
    waiting: Option<ColorSpec>,
    /// Pulse color on accept (tap). Static only — used for the flash animation.
    accept: Option<Rgb>,
    /// Pulse color on reject (hold). Static only — used for the flash animation.
    reject: Option<Rgb>,
}

#[derive(Deserialize, Default, Clone)]
struct Colors {
    idle: Option<ColorSpec>,
    started: Option<ColorSpec>,
    #[serde(default)]
    progress: ProgressColors,
    #[serde(default)]
    finished: FinishedColors,
    #[serde(default)]
    stage: StageColors,
    #[serde(default)]
    prompt: PromptColors,
}

#[derive(Deserialize, Default)]
struct Config {
    slots: Vec<u8>,
    /// Hold duration in ms to reject a prompt (default: 1000)
    hold_ms: Option<u64>,
    #[serde(default)]
    colors: Colors,
}

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
enum JobState {
    Created {
        metadata: HashMap<String, OwnedValue>,
    },
    Started,
    Progress {
        current: u32,
        total: u32,
    },
    Stage(String),
    Prompt(String),
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
    /// Metadata set at creation; persisted for the lifetime of the job so
    /// GetMetadata works regardless of the current state.
    metadata: HashMap<String, OwnedValue>,
}

struct JobManager {
    slots: Vec<Slot>,
    next_job_id: u32,
    job_to_slot: HashMap<u32, usize>,
    key_to_slot: HashMap<(u8, u8), usize>,
    colors: Colors,
    hold_ms: u64,
    notify: Arc<Notify>,
    prompt_responders: HashMap<u32, oneshot::Sender<bool>>,
    /// Notified whenever a prompt resolves so that blocked callers can retry.
    prompt_done: Arc<Notify>,
    /// Answer stored by `PromptResolve` when it races ahead of `Prompt()`.
    /// This happens when both DBus calls are issued in the same event-loop
    /// batch (e.g. `permission.asked` + `permission.replied` in the same
    /// 40 ms throttle window). `prompt()` drains this immediately after
    /// inserting the oneshot sender instead of blocking on it.
    pre_resolved: HashMap<u32, bool>,
}

impl JobManager {
    fn new(config: &Config) -> Result<Self> {
        let mut slots = Vec::with_capacity(config.slots.len());
        let mut key_to_slot = HashMap::new();

        for (i, &led) in config.slots.iter().enumerate() {
            // Firmware KeyDown/KeyUp events use the row-major scheme (led_to_pos),
            // not the column-major LED scheme (led_to_key). Use led_to_pos so
            // key_to_slot matches what recv_event() actually delivers.
            let (col, row) = oryx_hid::matrix::led_to_pos(led)
                .with_context(|| format!("LED index {led} has no known matrix position"))?;
            key_to_slot.insert((col, row), i);
            slots.push(Slot {
                led,
                col,
                row,
                job_id: None,
                state: None,
                metadata: HashMap::new(),
            });
        }

        Ok(Self {
            slots,
            next_job_id: 1,
            job_to_slot: HashMap::new(),
            key_to_slot,
            colors: config.colors.clone(),
            hold_ms: config.hold_ms.unwrap_or(1000),
            notify: Arc::new(Notify::new()),
            prompt_responders: HashMap::new(),
            prompt_done: Arc::new(Notify::new()),
            pre_resolved: HashMap::new(),
        })
    }

    fn alloc_slot(
        &mut self,
        metadata: HashMap<String, OwnedValue>,
        preferred_slot: Option<usize>,
    ) -> Option<(u32, u8)> {
        let slot_i = if let Some(ps) = preferred_slot {
            if ps >= self.slots.len() {
                return None;
            }
            if self.slots[ps].job_id.is_some() {
                return None;
            }
            ps
        } else {
            self.slots.iter().position(|s| s.job_id.is_none())?
        };
        let id = self.next_job_id;
        self.next_job_id += 1;
        self.slots[slot_i].job_id = Some(id);
        self.slots[slot_i].metadata = metadata.clone();
        self.slots[slot_i].state = Some(JobState::Created { metadata });
        self.job_to_slot.insert(id, slot_i);
        let led = self.slots[slot_i].led;
        Some((id, led))
    }

    fn set_state(&mut self, job_id: u32, state: JobState) -> Option<(u8, LedAction)> {
        let &slot_i = self.job_to_slot.get(&job_id)?;
        self.slots[slot_i].state = Some(state.clone());
        let led = self.slots[slot_i].led;
        Some((led, resolve_led_action(Some(&state), &self.colors)))
    }

    fn get_state(&self, job_id: u32) -> Option<&JobState> {
        let &slot_i = self.job_to_slot.get(&job_id)?;
        self.slots[slot_i].state.as_ref()
    }

    fn get_metadata(&self, job_id: u32) -> Option<&HashMap<String, OwnedValue>> {
        let &slot_i = self.job_to_slot.get(&job_id)?;
        Some(&self.slots[slot_i].metadata)
    }

    /// Unconditionally free a slot by job_id. Returns the LED index if the
    /// slot was found. Used for timeout-based auto-clear of Finished slots.
    fn force_clear(&mut self, job_id: u32) -> Option<u8> {
        let &slot_i = self.job_to_slot.get(&job_id)?;
        let led = self.slots[slot_i].led;
        self.slots[slot_i].job_id = None;
        self.slots[slot_i].state = None;
        self.slots[slot_i].metadata = HashMap::new();
        self.job_to_slot.remove(&job_id);
        self.notify.notify_waiters();
        Some(led)
    }

    /// Called on KeyUp for a slot. Returns the cleared job_id if it was Finished.
    fn try_clear(&mut self, col: u8, row: u8) -> Option<(u32, u8)> {
        let &slot_i = self.key_to_slot.get(&(col, row))?;
        if !matches!(self.slots[slot_i].state, Some(JobState::Finished(_))) {
            return None;
        }
        let job_id = self.slots[slot_i].job_id.take()?;
        self.slots[slot_i].state = None;
        self.slots[slot_i].metadata = HashMap::new();
        self.job_to_slot.remove(&job_id);
        let led = self.slots[slot_i].led;
        self.notify.notify_waiters();
        Some((job_id, led))
    }

    /// Check if a key position corresponds to a slot in Prompt state.
    /// Returns (job_id, led_index) if so.
    fn get_prompt_slot(&self, col: u8, row: u8) -> Option<(u32, u8)> {
        let &slot_i = self.key_to_slot.get(&(col, row))?;
        if !matches!(self.slots[slot_i].state, Some(JobState::Prompt(_))) {
            return None;
        }
        let job_id = self.slots[slot_i].job_id?;
        Some((job_id, self.slots[slot_i].led))
    }

    /// Resolve a prompt: take the oneshot sender and return it along with the accept/reject colors.
    fn take_prompt_responder(
        &mut self,
        job_id: u32,
    ) -> Option<oneshot::Sender<bool>> {
        self.prompt_responders.remove(&job_id)
    }
}

// ── Color resolution ──────────────────────────────────────────────────────────

fn match_finished_value(v: &OwnedValue, m: &FinishedMatchValue) -> bool {
    match (m, &**v) {
        (FinishedMatchValue::Int(n), Value::I32(x)) => *x as i64 == *n,
        (FinishedMatchValue::Int(n), Value::U32(x)) => *x as i64 == *n,
        (FinishedMatchValue::Int(n), Value::I64(x)) => *x == *n,
        (FinishedMatchValue::Int(n), Value::U64(x)) => *x as i64 == *n,
        (FinishedMatchValue::Int(n), Value::I16(x)) => *x as i64 == *n,
        (FinishedMatchValue::Int(n), Value::U16(x)) => *x as i64 == *n,
        (FinishedMatchValue::Int(n), Value::U8(x)) => *x as i64 == *n,
        (FinishedMatchValue::Str(s), Value::Str(x)) => x.as_str() == s,
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

/// Sample a multi-stop gradient at position `t` ∈ [0, 1].
fn sample_gradient(colors: &[Rgb], t: f32) -> (u8, u8, u8) {
    match colors.len() {
        0 => (255, 255, 255),
        1 => (colors[0].0, colors[0].1, colors[0].2),
        n => {
            let pos = t.clamp(0.0, 1.0) * (n - 1) as f32;
            let i = (pos.floor() as usize).min(n - 2);
            lerp_rgb(colors[i], colors[i + 1], pos.fract())
        }
    }
}

/// Compute the LED RGB output for an animation at a given elapsed time.
fn sample_anim(spec: &AnimSpec, elapsed_ms: f32) -> (u8, u8, u8) {
    let period = spec.period();
    // Normalised time 0..1, wrapping.
    let t = (elapsed_ms / period).fract();
    let gradient = spec.gradient();

    match spec.animation {
        AnimKind::Breathe => {
            let phase = t * std::f32::consts::TAU;
            let brightness = 0.15 + 0.85 * ((phase.sin() + 1.0) / 2.0);
            let (r, g, b) = sample_gradient(&gradient, t);
            (
                (r as f32 * brightness).round() as u8,
                (g as f32 * brightness).round() as u8,
                (b as f32 * brightness).round() as u8,
            )
        }
        AnimKind::Bounce => {
            // Triangle wave: 0→1 for the first half, 1→0 for the second half.
            let pos = if t < 0.5 { t * 2.0 } else { (1.0 - t) * 2.0 };
            sample_gradient(&gradient, pos)
        }
    }
}

/// What the LED should do after a state transition.
enum LedAction {
    /// Set a fixed color immediately.
    Static(u8, u8, u8),
    /// Start a continuous animation.
    Animate(AnimSpec),
}

/// Convert a `ColorSpec` (or `None`) into a `LedAction`, using `default` when
/// the spec is absent.
fn color_spec_to_action(cs: Option<&ColorSpec>, default: Rgb) -> LedAction {
    match cs {
        Some(ColorSpec::Static(rgb)) => LedAction::Static(rgb.0, rgb.1, rgb.2),
        Some(ColorSpec::Animated(spec)) => LedAction::Animate(spec.clone()),
        None => LedAction::Static(default.0, default.1, default.2),
    }
}

fn resolve_led_action(state: Option<&JobState>, colors: &Colors) -> LedAction {
    match state {
        None | Some(JobState::Created { .. }) => {
            color_spec_to_action(colors.idle.as_ref(), Rgb(0, 0, 0))
        }
        Some(JobState::Started) => {
            color_spec_to_action(colors.started.as_ref(), Rgb(0, 100, 255))
        }
        Some(JobState::Progress { current, total }) => {
            let t = if *total == 0 {
                0.0
            } else {
                (*current as f32 / *total as f32).clamp(0.0, 1.0)
            };
            let start = colors.progress.start.unwrap_or(Rgb(0, 100, 255));
            let end = colors.progress.end.unwrap_or(Rgb(0, 255, 100));
            let (r, g, b) = lerp_rgb(start, end, t);
            LedAction::Static(r, g, b)
        }
        Some(JobState::Stage(name)) => {
            let cs = colors
                .stage
                .matches
                .iter()
                .find(|m| m.name == *name)
                .map(|m| &m.color)
                .or_else(|| colors.stage.default.as_ref());
            color_spec_to_action(cs, Rgb(255, 200, 0))
        }
        Some(JobState::Prompt(_)) => {
            // Always animate for the prompt state; default to breathe at purple.
            let spec = match &colors.prompt.waiting {
                Some(ColorSpec::Animated(s)) => s.clone(),
                Some(ColorSpec::Static(rgb)) => AnimSpec {
                    animation: AnimKind::Breathe,
                    color: Some(*rgb),
                    colors: vec![],
                    period_ms: None,
                },
                None => AnimSpec {
                    animation: AnimKind::Breathe,
                    color: Some(Rgb(200, 0, 255)),
                    colors: vec![],
                    period_ms: None,
                },
            };
            LedAction::Animate(spec)
        }
        Some(JobState::Finished(v)) => {
            let cs = colors
                .finished
                .matches
                .iter()
                .find(|m| match_finished_value(v, &m.value))
                .map(|m| &m.color)
                .or_else(|| colors.finished.default.as_ref());
            color_spec_to_action(cs, Rgb(180, 180, 180))
        }
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
            [(
                "name".to_string(),
                OwnedValue::from(zbus::zvariant::Str::from(name.as_str())),
            )]
            .into(),
        ),
        Some(JobState::Prompt(text)) => (
            "prompt",
            [(
                "text".to_string(),
                OwnedValue::from(zbus::zvariant::Str::from(text.as_str())),
            )]
            .into(),
        ),
        Some(JobState::Finished(value)) => {
            ("finished", [("value".to_string(), value.clone())].into())
        }
    }
}

// ── Keyboard command channel ──────────────────────────────────────────────────

enum KbdCmd {
    /// Set a fixed color on the LED and stop any running animation.
    SetRgb { index: u8, r: u8, g: u8, b: u8 },
    /// Start a continuous animation on the LED (breathe or bounce).
    Animate { index: u8, spec: AnimSpec },
    /// Stop any running animation on the LED (does not change the current color).
    StopAnim { index: u8 },
    /// Auto-clear a finished slot (timeout elapsed).
    ClearFinished { job_id: u32 },
    /// Hold threshold reached on a prompt slot — reject immediately.
    HoldReject { col: u8, row: u8 },
    /// Trigger the accept/reject pulse animation externally (PromptResolve path).
    /// The oneshot responder has already been sent; this is purely for the LED.
    Pulse {
        index: u8,
        r: u8,
        g: u8,
        b: u8,
        accepted: bool,
        job_id: u32,
    },
}

// ── DBus interface ────────────────────────────────────────────────────────────

/// Dispatch a `LedAction` to `keyboard_task` via the command channel.
/// For a static color we stop any running animation first then set the color.
/// For an animation we just start it (it overwrites any previous animation).
async fn send_led_action(tx: &mpsc::Sender<KbdCmd>, index: u8, action: LedAction) {
    match action {
        LedAction::Static(r, g, b) => {
            // SetRgb handler in keyboard_task already removes from animating map.
            let _ = tx.send(KbdCmd::SetRgb { index, r, g, b }).await;
        }
        LedAction::Animate(spec) => {
            let _ = tx.send(KbdCmd::Animate { index, spec }).await;
        }
    }
}

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
        slot: i32,
        timeout_ms: i32,
    ) -> zbus::fdo::Result<u32> {
        let timeout = if timeout_ms < 0 {
            None
        } else {
            Some(Duration::from_millis(timeout_ms as u64))
        };
        let slot = if slot < 0 { None } else { Some(slot as usize) };

        loop {
            let result = {
                let mut st = self.state.lock().await;
                let preferred = slot;
                match st.alloc_slot(metadata.clone(), preferred) {
                    Some((id, led)) => Ok((id, led)),
                    None => {
                        if let Some(ps) = preferred
                            && ps >= st.slots.len()
                        {
                            return Err(zbus::fdo::Error::Failed(format!(
                                "slot {ps} does not exist"
                            )));
                        }
                        debug!("no free slot, waiting");
                        let notify = Arc::clone(&st.notify);
                        drop(st);
                        if let Some(dur) = timeout {
                            let notified = notify.notified();
                            tokio::select! {
                                _ = notified => continue,
                                _ = tokio::time::sleep(dur) => {
                                    warn!("timed out waiting for a free slot");
                                    Err(if let Some(ps) = slot {
                                        zbus::fdo::Error::Failed(format!("timeout waiting for slot {ps}"))
                                    } else {
                                        zbus::fdo::Error::Failed("timeout waiting for free slot".to_string())
                                    })
                                }
                            }
                        } else {
                            notify.notified().await;
                            continue;
                        }
                    }
                }
            };

            let (id, led) = result?;
            let st = self.state.lock().await;
            let action = resolve_led_action(
                st.slots[*st.job_to_slot.get(&id).unwrap()].state.as_ref(),
                &st.colors,
            );
            drop(st);
            info!(job_id = id, led, "job created");
            send_led_action(&self.cmd_tx, led, action).await;
            let _ = self
                .status_tx
                .send((id, Some(JobState::Created { metadata })))
                .await;
            return Ok(id);
        }
    }

    async fn start(&self, job_id: u32) -> zbus::fdo::Result<()> {
        self.update(job_id, JobState::Started).await
    }

    async fn progress(&self, job_id: u32, current: u32, total: u32) -> zbus::fdo::Result<()> {
        self.update(job_id, JobState::Progress { current, total })
            .await
    }

    async fn stage(&self, job_id: u32, name: String) -> zbus::fdo::Result<()> {
        self.update(job_id, JobState::Stage(name)).await
    }

    async fn finish(
        &self,
        job_id: u32,
        value: OwnedValue,
        timeout_ms: i32,
    ) -> zbus::fdo::Result<()> {
        self.update(job_id, JobState::Finished(value)).await?;
        if timeout_ms >= 0 {
            info!(job_id, timeout_ms, "finished: auto-clear scheduled");
            let cmd_tx = self.cmd_tx.clone();
            let dur = Duration::from_millis(timeout_ms as u64);
            tokio::spawn(async move {
                tokio::time::sleep(dur).await;
                let _ = cmd_tx.send(KbdCmd::ClearFinished { job_id }).await;
            });
        } else {
            info!(job_id, "finished: waiting for key press to clear");
        }
        Ok(())
    }

    /// Enter prompt state: LED animates (default: breathe) until the user taps
    /// (accept → true) or holds (reject → false) the slot key, or until
    /// `PromptResolve` is called externally.
    async fn prompt(&self, job_id: u32, text: String) -> zbus::fdo::Result<bool> {
        let (tx, rx) = oneshot::channel();

        let led = {
            let mut st = self.state.lock().await;
            let (led, action) = st
                .set_state(job_id, JobState::Prompt(text.clone()))
                .ok_or_else(|| zbus::fdo::Error::Failed(format!("unknown job {job_id}")))?;
            st.prompt_responders.insert(job_id, tx);

            // Drain any answer that PromptResolve() stored before we arrived.
            // This happens when permission.asked + permission.replied land in
            // the same Neovim event batch: PromptResolve races ahead of Prompt().
            if let Some(accepted) = st.pre_resolved.remove(&job_id) {
                let tx2 = st.prompt_responders.remove(&job_id).unwrap();
                let slot_i = *st.job_to_slot.get(&job_id).unwrap();
                st.slots[slot_i].state = Some(JobState::Started);
                let _ = tx2.send(accepted);
                debug!(job_id, accepted, "prompt: consumed pre-resolved answer (PromptResolve raced ahead)");
            }
            drop(st);

            info!(job_id, text, "prompt: waiting for keyboard or PromptResolve");
            send_led_action(&self.cmd_tx, led, action).await;
            let _ = self
                .status_tx
                .send((job_id, Some(JobState::Prompt(text))))
                .await;
            led
        };

        // Block until the user responds — returns immediately if pre-resolved above.
        let result = rx.await;

        // Always stop animation regardless of how the prompt resolved.
        let _ = self.cmd_tx.send(KbdCmd::StopAnim { index: led }).await;

        // Wake any callers that were blocked waiting for this prompt to finish.
        self.state.lock().await.prompt_done.notify_waiters();

        let accepted = result.map_err(|_| {
            zbus::fdo::Error::Failed("prompt cancelled (channel dropped)".to_string())
        })?;

        info!(job_id, accepted, "prompt: resolved");
        Ok(accepted)
    }

    /// Resolve a pending prompt externally (e.g. from Neovim's UI) without
    /// requiring physical keyboard input. Unblocks the waiting `Prompt()` call,
    /// stops the breathing animation, and triggers the accept/reject pulse.
    ///
    /// Returns `Ok(())` silently if the prompt was already resolved (race is fine).
    async fn prompt_resolve(&self, job_id: u32, accepted: bool) -> zbus::fdo::Result<()> {
        let (led, pulse_color) = {
            let mut st = self.state.lock().await;

            if !matches!(st.get_state(job_id), Some(JobState::Prompt(_))) {
                if st.job_to_slot.contains_key(&job_id) {
                    // Prompt() hasn't been called yet — store the answer so
                    // prompt() can consume it immediately without blocking.
                    debug!(job_id, accepted, "PromptResolve arrived before Prompt(); pre-resolving");
                    st.pre_resolved.insert(job_id, accepted);
                }
                // Either pre-stored or already fully resolved — either way done.
                return Ok(());
            }

            let tx = match st.take_prompt_responder(job_id) {
                Some(t) => t,
                None => return Ok(()), // responder already consumed by keyboard
            };

            let &slot_i = st.job_to_slot.get(&job_id).unwrap();
            let led = st.slots[slot_i].led;

            // Transition state back to Started (mirrors the keyboard path).
            st.slots[slot_i].state = Some(JobState::Started);

            let pulse_color = if accepted {
                st.colors.prompt.accept.unwrap_or(Rgb(0, 255, 0))
            } else {
                st.colors.prompt.reject.unwrap_or(Rgb(255, 0, 0))
            };

            // Unblock the waiting Prompt() DBus call.
            let _ = tx.send(accepted);

            (led, pulse_color)
        };

        info!(job_id, accepted, "prompt resolved via PromptResolve");

        // Stop animation and trigger the pulse in keyboard_task.
        let _ = self.cmd_tx.send(KbdCmd::StopAnim { index: led }).await;
        let _ = self
            .cmd_tx
            .send(KbdCmd::Pulse {
                index: led,
                r: pulse_color.0,
                g: pulse_color.1,
                b: pulse_color.2,
                accepted,
                job_id,
            })
            .await;

        // Wake any callers blocked waiting for the prompt to finish.
        self.state.lock().await.prompt_done.notify_waiters();

        Ok(())
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

    /// Return the metadata map that was supplied when this job was created.
    /// Available for the full lifetime of the job, regardless of current state.
    async fn get_metadata(
        &self,
        job_id: u32,
    ) -> zbus::fdo::Result<HashMap<String, OwnedValue>> {
        let st = self.state.lock().await;
        let meta = st
            .get_metadata(job_id)
            .ok_or_else(|| zbus::fdo::Error::Failed(format!("unknown job {job_id}")))?;
        Ok(meta.clone())
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
        // If the job is currently in Prompt state, wait for it to resolve first.
        let (led, action) = loop {
            let mut st = self.state.lock().await;
            if matches!(st.get_state(job_id), Some(JobState::Prompt(_))) {
                let prompt_done = Arc::clone(&st.prompt_done);
                drop(st);
                prompt_done.notified().await;
                continue;
            }
            break st
                .set_state(job_id, state.clone())
                .ok_or_else(|| zbus::fdo::Error::Failed(format!("unknown job {job_id}")))?;
        };
        match &state {
            JobState::Started => info!(job_id, "started"),
            JobState::Progress { current, total } => {
                info!(job_id, current, total, "progress")
            }
            JobState::Stage(name) => info!(job_id, stage = name, "stage"),
            JobState::Finished(v) => info!(job_id, value = ?v, "finished"),
            // Created is handled in create(); Prompt is handled in prompt().
            _ => {}
        }
        debug!(job_id, led, "LED updated");
        send_led_action(&self.cmd_tx, led, action).await;
        let _ = self.status_tx.send((job_id, Some(state))).await;
        Ok(())
    }
}

// ── Animation state ───────────────────────────────────────────────────────────

struct AnimState {
    spec: AnimSpec,
    start: Instant,
}

struct PulseState {
    r: u8,
    g: u8,
    b: u8,
    start: Instant,
    count: u8,
    accepted: bool,
    job_id: u32,
    responder: Option<oneshot::Sender<bool>>,
}

const PULSE_CYCLE_MS: f32 = 166.0; // ~83ms on, ~83ms off

// ── Keyboard task ─────────────────────────────────────────────────────────────

async fn keyboard_task(
    mut kbd: OryxKeyboard,
    mut cmd_rx: mpsc::Receiver<KbdCmd>,
    status_tx: mpsc::Sender<(u32, Option<JobState>)>,
    cmd_tx: mpsc::Sender<KbdCmd>,
    state: Arc<Mutex<JobManager>>,
) {
    let mut animating: HashMap<u8, AnimState> = HashMap::new();
    let mut pulsing: HashMap<u8, PulseState> = HashMap::new();
    let mut keydown_times: HashMap<(u8, u8), Instant> = HashMap::new();

    let mut anim_interval = tokio::time::interval(Duration::from_millis(30));
    anim_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    KbdCmd::SetRgb { index, r, g, b } => {
                        animating.remove(&index);
                        let _ = kbd.rgb(index, r, g, b).await;
                    }
                    KbdCmd::Animate { index, spec } => {
                        animating.insert(index, AnimState {
                            spec,
                            start: Instant::now(),
                        });
                    }
                    KbdCmd::StopAnim { index } => {
                        animating.remove(&index);
                    }
                    KbdCmd::ClearFinished { job_id } => {
                        let mut st = state.lock().await;
                        // Guard: a key press may have already cleared this slot.
                        if matches!(st.get_state(job_id), Some(JobState::Finished(_))) {
                            if let Some(led) = st.force_clear(job_id) {
                                drop(st);
                                info!(job_id, led, "job auto-cleared by timeout");
                                animating.remove(&led);
                                let _ = kbd.rgb(led, 0, 0, 0).await;
                                let _ = status_tx.send((job_id, None)).await;
                            }
                        }
                    }
                    KbdCmd::HoldReject { col, row } => {
                        // Unused: reserved for future hold-threshold rejection.
                        debug!(col, row, "HoldReject received (unimplemented)");
                    }
                    KbdCmd::Pulse { index, r, g, b, accepted, job_id } => {
                        // Triggered by PromptResolve — oneshot already sent, just animate.
                        animating.remove(&index);
                        pulsing.insert(index, PulseState {
                            r,
                            g,
                            b,
                            start: Instant::now(),
                            count: 3,
                            accepted,
                            job_id,
                            responder: None,
                        });
                    }
                }
            }
            result = kbd.recv_event() => {
                match result {
                    Ok(KbdEvent::KeyDown { col, row }) => {
                        let st = state.lock().await;
                        if let Some((job_id, _)) = st.get_prompt_slot(col, row) {
                            debug!(job_id, col, row, "key down on prompt slot");
                            keydown_times.insert((col, row), Instant::now());
                        }
                        drop(st);
                    }
                    Ok(KbdEvent::KeyUp { col, row }) => {
                        let mut st = state.lock().await;

                        // Check prompt slots first.
                        if let Some((job_id, led)) = st.get_prompt_slot(col, row) {
                            let hold_ms = st.hold_ms;
                            let accept_color = st.colors.prompt.accept.unwrap_or(Rgb(0, 255, 0));
                            let reject_color = st.colors.prompt.reject.unwrap_or(Rgb(255, 0, 0));

                            let down_time = keydown_times.remove(&(col, row));
                            let held_ms = down_time
                                .map(|t| t.elapsed().as_millis() as u64)
                                .unwrap_or(0);
                            let accepted = held_ms < hold_ms;

                            info!(job_id, accepted, held_ms, hold_ms, "prompt key released");

                            let responder = st.take_prompt_responder(job_id);

                            // Transition state back to Started (prompt is resolved).
                            let slot_i = *st.job_to_slot.get(&job_id).unwrap();
                            st.slots[slot_i].state = Some(JobState::Started);
                            drop(st);

                            // Stop animating, start pulse.
                            animating.remove(&led);
                            let pc = if accepted { accept_color } else { reject_color };
                            pulsing.insert(led, PulseState {
                                r: pc.0,
                                g: pc.1,
                                b: pc.2,
                                start: Instant::now(),
                                count: 3,
                                accepted,
                                job_id,
                                responder,
                            });
                        } else if let Some((job_id, led)) = st.try_clear(col, row) {
                            info!(job_id, led, "job cleared by key");
                            drop(st);
                            let _ = status_tx.send((job_id, None)).await;
                            let _ = cmd_tx.send(KbdCmd::SetRgb { index: led, r: 0, g: 0, b: 0 }).await;
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "keyboard HID error, keyboard_task exiting");
                        break;
                    }
                    _ => {}
                }
            }
            _ = anim_interval.tick() => {
                let now = Instant::now();

                // Update animated LEDs.
                for (&led, anim) in &animating {
                    let elapsed = now.duration_since(anim.start).as_millis() as f32;
                    let (r, g, b) = sample_anim(&anim.spec, elapsed);
                    let _ = kbd.rgb(led, r, g, b).await;
                }

                // Update pulsing LEDs, removing finished ones.
                let mut done = Vec::new();
                for (&led, ps) in &pulsing {
                    let elapsed = now.duration_since(ps.start).as_millis() as f32;
                    let total_ms = ps.count as f32 * PULSE_CYCLE_MS;
                    if elapsed >= total_ms {
                        done.push(led);
                        continue;
                    }
                    let phase = (elapsed / PULSE_CYCLE_MS) * std::f32::consts::TAU;
                    let brightness = ((phase.sin() + 1.0) / 2.0).powi(2);
                    let r = (ps.r as f32 * brightness).round() as u8;
                    let g = (ps.g as f32 * brightness).round() as u8;
                    let b = (ps.b as f32 * brightness).round() as u8;
                    let _ = kbd.rgb(led, r, g, b).await;
                }

                for led in done {
                    if let Some(mut ps) = pulsing.remove(&led) {
                        // Turn off the LED after pulse.
                        let _ = kbd.rgb(led, 0, 0, 0).await;

                        // Send response and emit state signal.
                        if let Some(responder) = ps.responder.take() {
                            let _ = responder.send(ps.accepted);
                        }
                        let _ = status_tx
                            .send((ps.job_id, Some(JobState::Started)))
                            .await;
                    }
                }
            }
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "oryx_jobs=info".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

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
        Err(e) => {
            return Err(e)
                .with_context(|| format!("reading config from {}", config_path.display()));
        }
    };

    if !cli.slot.is_empty() {
        config.slots = cli.slot;
    }

    if config.slots.is_empty() {
        bail!("no slots configured; use --slot <LED> or set slots in the config file");
    }

    info!(slots = ?config.slots, "configured");

    let state = Arc::new(Mutex::new(JobManager::new(&config)?));

    let mut kbd = OryxKeyboard::open().await.context("opening keyboard")?;
    kbd.rgb_control(true)
        .await
        .context("enabling RGB control")?;
    info!("keyboard opened");

    // Initialise every slot LED to the idle color/animation before any job
    // is created. This ensures a clean known state after (re)starting the
    // service rather than leaving whatever the keyboard had last.
    {
        let idle_action = resolve_led_action(None, &config.colors);
        for &led in &config.slots {
            match &idle_action {
                LedAction::Static(r, g, b) => {
                    kbd.rgb(led, *r, *g, *b).await.ok();
                }
                LedAction::Animate(_) => {
                    // Animation will start once keyboard_task is running and
                    // processes the Animate commands sent below via cmd_tx.
                    // For now just turn the LED off so it starts clean.
                    kbd.rgb(led, 0, 0, 0).await.ok();
                }
            }
        }
    }

    let (cmd_tx, cmd_rx) = mpsc::channel::<KbdCmd>(32);
    let (status_tx, mut status_rx) = mpsc::channel::<(u32, Option<JobState>)>(32);

    tokio::spawn(keyboard_task(
        kbd,
        cmd_rx,
        status_tx.clone(),
        cmd_tx.clone(),
        state.clone(),
    ));

    // If the idle spec is animated, start the animation on every slot now that
    // keyboard_task is running and can process Animate commands.
    if let LedAction::Animate(spec) = resolve_led_action(None, &config.colors) {
        for &led in &config.slots {
            let _ = cmd_tx.send(KbdCmd::Animate { index: led, spec: spec.clone() }).await;
        }
    }

    let jobs = Jobs {
        state,
        cmd_tx,
        status_tx,
    };

    let conn = connection::Builder::session()
        .context("creating session bus")?
        .name("zsa.oryx.Jobs")
        .context("claiming bus name")?
        .serve_at("/zsa/oryx/Jobs", jobs)
        .context("registering object")?
        .build()
        .await
        .context("building connection")?;
    info!("DBus service ready on zsa.oryx.Jobs");

    let iface_ref = conn
        .object_server()
        .interface::<_, Jobs>("/zsa/oryx/Jobs")
        .await
        .context("getting interface ref")?;

    tokio::spawn(async move {
        while let Some((job_id, job_state)) = status_rx.recv().await {
            let emitter = iface_ref.signal_emitter();
            let (state_str, meta) = state_to_signal(job_state.as_ref());
            debug!(job_id, state = state_str, "emitting State signal");
            let _ = Jobs::state(emitter, job_id, state_str, meta).await;
        }
    });

    pending::<()>().await;
    Ok(())
}
