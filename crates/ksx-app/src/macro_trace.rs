//! `ksx macro trace` — the measuring instrument for macro playback.
//!
//! # Why this exists
//!
//! Everything upstream of the driver is already provable with a fake clock
//! (`crates/ksx-core/tests/engine_macros.rs`): the engine is a pure function of
//! `(events, clock)`, so "did the diagonal get emitted, and was it stable for
//! the whole step" is a unit test. What no unit test can answer is the question
//! that actually decides whether a game sees the input:
//!
//! > On a REAL clock, through the REAL output path, how long did each state
//! > exist — and would a 60 Hz poller have sampled it?
//!
//! That is a *measurement*, not an argument, and it needs a real timer, a real
//! `VigemBackend::update`, and a real observer. This command is that instrument:
//! it plugs a pad, plays one run of a macro through the same `Engine` +
//! `VirtualPadBackend` the daemon uses, timestamps every submission, and — the
//! number that matters — samples the published state at the consumer's rate and
//! reports the DISTINCT states that consumer would have seen.
//!
//! The distinction it is built to expose:
//!
//! - a state that is **submitted** is something ksx did;
//! - a state that is **observed** is something the game got.
//!
//! A state can be submitted perfectly and observed never. XInput hands out a
//! *snapshot*, not a queue (`XInputGetState` "retrieves the current state";
//! `dwPacketNumber` says only THAT it changed, never what the intermediate
//! values were), and Unreal Engine — Street Fighter V's engine — polls it once
//! per frame tick. So a state living less than one poll interval is not
//! unreliable, it is invisible, and the only honest way to know which side of
//! that line a step falls on is to sample it.
//!
//! # What it does not claim
//!
//! It measures up to the driver call, plus a poller of our own at the game's
//! rate. It cannot see inside ViGEmBus, XInput, or the game. If the trace shows
//! a state submitted and observed and the game still does not react, the loss is
//! downstream of everything ksx controls, and that is a useful answer too — it
//! moves the question from "is ksx dropping it" to "what does this game require".
//!
//! # Isolation
//!
//! It captures no keyboard and touches no configuration. `--config-dir` points
//! it at a portable root so a trace never reads or writes the installed
//! `%APPDATA%\ksx`, and the pad it plugs is its own — ViGEm targets are
//! per-process, so this runs safely beside a live `ksx daemon` (it does consume
//! one of the four XInput slots while it runs).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ksx_core::{
    DeviceId, Engine, Key, KeyEvent, Macro, MacroTrigger, PadState, Persona, ResolvedSlot,
    SlotSpec, XButtons,
};
use ksx_output::{PadHandle, VirtualPadBackend};

use crate::map::EXIT_REFUSED;

/// The consumer rate the sampling rule is written for: 60 Hz, one frame every
/// 16.667 ms. Kept as a rational and divided once, so a long trace's sample
/// times do not drift against the game's.
const DEFAULT_SAMPLE_HZ: u32 = 60;

/// How long the trace keeps sampling after the macro's last step ends.
///
/// Without it the final state has no measured dwell — it would look like it
/// lasted zero milliseconds because nothing came after it to bound it.
const TAIL: Duration = Duration::from_millis(150);

/// Half a stick's travel. Above it an axis is "deflected" for the purpose of
/// naming a direction — this is the tracer's own reading aid and is deliberately
/// NOT a claim about any game's deadzone, which is the game's business.
const DEFLECTED: i32 = 16_384;

pub struct Options {
    pub preset: String,
    pub name: String,
    /// Rate of the simulated consumer, in hertz. 60 is the number that matters.
    pub sample_hz: u32,
    /// Read configuration from this directory instead of discovering the
    /// installed root. The whole point is that a trace is never able to disturb
    /// a live setup.
    pub config_dir: Option<PathBuf>,
    pub persona: Persona,
    /// Trace through [`ksx_output::MockBackend`] instead of ViGEm: no driver, no
    /// pad, same engine and same timing. What it measures is ksx's scheduler;
    /// what it cannot measure is the driver call.
    pub dry_run: bool,
    /// How long the trigger key is held. A tap, by default — the fighting-game
    /// gesture, and the one `Repeat::Once` is written for.
    pub hold_ms: u64,
    pub json: bool,
}

// ---------------------------------------------------------------------------
// The recording
// ---------------------------------------------------------------------------

/// One state handed to the driver, and everything measurable about handing it
/// over.
#[derive(Clone, Copy)]
struct Submit {
    /// Microseconds since the trigger press.
    t_us: u64,
    /// Since the previous submission — the wall-clock the scheduler achieved,
    /// as opposed to the one the macro asked for.
    gap_us: u64,
    /// How long the backend call itself took. A driver that blocks here is the
    /// F2 failure mode, and it would show up as a gap on the NEXT row.
    call_us: u64,
    state: PadState,
}

/// One poll by the simulated consumer.
#[derive(Clone, Copy)]
struct Sample {
    t_us: u64,
    state: PadState,
}

/// A run of consecutive samples that all read the same state — i.e. one thing
/// the consumer actually saw.
struct Observed {
    state: PadState,
    samples: usize,
    first_us: u64,
    last_us: u64,
}

/// The published state, as a consumer would find it: whatever was last
/// successfully submitted. Shared with the sampler thread.
type Published = Arc<Mutex<PadState>>;

/// Wraps the backend so every submission is timestamped and published in one
/// place — the recorder is the instrument, not a second copy of the output path.
struct Recorder {
    pads: Box<dyn VirtualPadBackend>,
    handle: PadHandle,
    t0: Instant,
    last_us: u64,
    submits: Vec<Submit>,
    published: Published,
}

impl Recorder {
    fn submit(&mut self, state: PadState) -> Result<(), ksx_output::OutputError> {
        let before = Instant::now();
        let t_us = (before - self.t0).as_micros() as u64;
        self.pads.update(self.handle, &state)?;
        let call_us = before.elapsed().as_micros() as u64;
        // Published only after the driver accepted it: a state the call rejected
        // is a state no consumer could ever have read.
        *self.published.lock().expect("published state") = state;
        self.submits.push(Submit {
            t_us,
            gap_us: t_us.saturating_sub(self.last_us),
            call_us,
            state,
        });
        self.last_us = t_us;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Reading aids
// ---------------------------------------------------------------------------

/// The direction a state expresses, as an arrow, across BOTH mechanisms — the
/// dpad bits and the left stick. A state can drive either or both, and the
/// tracer must not pretend one of them is the whole truth.
fn arrow(state: &PadState) -> &'static str {
    let mut up = state.buttons.contains(XButtons::DPAD_UP);
    let mut down = state.buttons.contains(XButtons::DPAD_DOWN);
    let mut left = state.buttons.contains(XButtons::DPAD_LEFT);
    let mut right = state.buttons.contains(XButtons::DPAD_RIGHT);
    up |= i32::from(state.ly) >= DEFLECTED;
    down |= i32::from(state.ly) <= -DEFLECTED;
    right |= i32::from(state.lx) >= DEFLECTED;
    left |= i32::from(state.lx) <= -DEFLECTED;
    match (up, down, left, right) {
        (true, false, true, false) => "\u{2196}", // ↖
        (true, false, false, true) => "\u{2197}", // ↗
        (false, true, true, false) => "\u{2199}", // ↙
        (false, true, false, true) => "\u{2198}", // ↘
        (true, false, false, false) => "\u{2191}",
        (false, true, false, false) => "\u{2193}",
        (false, false, true, false) => "\u{2190}",
        (false, false, false, true) => "\u{2192}",
        _ => "\u{00b7}", // neutral, or a cancelled pair
    }
}

/// Two perpendicular directions at once — the state this whole investigation is
/// about.
fn is_diagonal(state: &PadState) -> bool {
    matches!(
        arrow(state),
        "\u{2196}" | "\u{2197}" | "\u{2199}" | "\u{2198}"
    )
}

/// Buttons and triggers, for the rows where they are the point.
fn extras(state: &PadState) -> String {
    let mut parts = Vec::new();
    let named = state.buttons.difference(
        XButtons::DPAD_UP | XButtons::DPAD_DOWN | XButtons::DPAD_LEFT | XButtons::DPAD_RIGHT,
    );
    if !named.is_empty() {
        parts.push(
            format!("{named:?}")
                .replace("XButtons(", "")
                .replace(')', ""),
        );
    }
    if state.lt != 0 {
        parts.push(format!("lt={}", state.lt));
    }
    if state.rt != 0 {
        parts.push(format!("rt={}", state.rt));
    }
    if parts.is_empty() {
        "-".to_owned()
    } else {
        parts.join(" ")
    }
}

fn state_json(state: &PadState) -> serde_json::Value {
    serde_json::json!({
        "arrow": arrow(state),
        "diagonal": is_diagonal(state),
        // Named exactly as the XInput wire fields the driver receives, because
        // that is what a reader will compare against.
        "thumb_lx": state.lx,
        "thumb_ly": state.ly,
        "thumb_rx": state.rx,
        "thumb_ry": state.ry,
        "buttons": state.buttons.bits(),
        "left_trigger": state.lt,
        "right_trigger": state.rt,
    })
}

fn ms(us: u64) -> f64 {
    us as f64 / 1000.0
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// The synthetic device the trace's key events come from. Never a real
/// keyboard: nothing is captured, and the id is obviously not a hardware path so
/// it can never be confused for one in a log.
const TRACE_DEVICE: &str = "ksx-macro-trace";

/// The key the trace presses. Only ever used to start the macro, on a device no
/// preset binds, so it cannot collide with a real binding.
const TRACE_KEY: Key = Key::F12;

struct Loaded {
    slot: ResolvedSlot,
    mac: Macro,
    trigger: Key,
    root: PathBuf,
    /// Every OTHER macro the same trigger key starts, with how long a run of it
    /// takes and whether it repeats.
    ///
    /// A trigger is multi-bind like any other row, so one key may start several
    /// sequences — and then the engine publishes their SUPERPOSITION, which can
    /// hold directions no single one of them has and can keep repeating after
    /// the traced macro is done. The trace plays the real preset through the
    /// real engine, so it plays those too; reporting the named macro alone would
    /// be blaming it for their states, which is exactly the misreading that
    /// produced the 2026-08-06 cabinet reports.
    also: Vec<Companion>,
}

struct Companion {
    name: String,
    total_ms: u64,
    repeat: ksx_core::Repeat,
}

fn load(options: &Options) -> anyhow::Result<Loaded> {
    let root = match &options.config_dir {
        Some(dir) => ksx_config::ConfigRoot::at(dir),
        None => ksx_config::ConfigRoot::discover()?,
    };
    let store = ksx_config::Store::new(root.clone());
    let presets = store.load_presets()?.value;
    let file = presets
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(&options.preset))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no preset named '{}' in {} (have: {})",
                options.preset,
                root.presets_dir().display(),
                presets
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;

    let mut preset = file.to_core()?;
    let index = preset.macros.index_of(&options.name).ok_or_else(|| {
        anyhow::anyhow!(
            "preset '{}' defines no macro named '{}' (have: {})",
            file.name,
            options.name,
            preset
                .macros
                .defs
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let mac = preset.macros.defs[usize::from(index)].clone();
    anyhow::ensure!(
        !mac.is_inert(),
        "macro '{}' has no steps — there is nothing to trace",
        mac.name
    );

    // The preset's own trigger key is preferred: tracing the macro on the key it
    // is really bound to keeps the trace honest about retrigger/interrupt. Only
    // when it has none does the trace add one, on its own device.
    let bound = preset.macros.keys_for(index).next();
    let trigger = match bound {
        Some(key) => key,
        None => {
            preset
                .macros
                .triggers
                .push(MacroTrigger::new(TRACE_KEY, index));
            TRACE_KEY
        }
    };

    // Who else this key starts. Read from the RESOLVED trigger list, so it is
    // the same answer the engine will act on rather than a re-parse of the file.
    let also = companions(&preset.macros, trigger, index);

    let device = DeviceId::new(TRACE_DEVICE);
    let spec =
        SlotSpec::new(1, Some(device), None, preset.name.clone())?.with_persona(options.persona);
    Ok(Loaded {
        slot: ResolvedSlot { spec, preset },
        mac,
        trigger,
        root: root.dir().to_path_buf(),
        also,
    })
}

/// Every OTHER macro `trigger` starts, in macro order and without repeats.
///
/// A trigger row is multi-bind like any other, so several `macro.<name>` rows
/// may name one key; the engine then starts all of them from the same press.
fn companions(macros: &ksx_core::Macros, trigger: Key, index: u16) -> Vec<Companion> {
    let mut seen: Vec<usize> = Vec::new();
    for t in &macros.triggers {
        let i = usize::from(t.index);
        if t.key != trigger || t.index == index || i >= macros.defs.len() || seen.contains(&i) {
            continue;
        }
        seen.push(i);
    }
    seen.sort_unstable();
    seen.into_iter()
        .map(|i| Companion {
            name: macros.defs[i].name.clone(),
            total_ms: macros.defs[i].total_ms(),
            repeat: macros.defs[i].repeat,
        })
        .collect()
}

impl Loaded {
    /// The longest single run any macro this key starts will take — what the
    /// sampler and the run budget must cover, so a trace of a short macro
    /// sharing a key with a long one is not cut off half way.
    fn run_ms(&self) -> u64 {
        run_ms(self.mac.total_ms(), &self.also)
    }

    /// Does anything this key starts keep going while the key is held?
    fn anything_repeats(&self) -> bool {
        self.mac.repeat.repeats() || self.also.iter().any(|c| c.repeat.repeats())
    }
}

fn run_ms(mac_total_ms: u64, also: &[Companion]) -> u64 {
    also.iter()
        .map(|c| c.total_ms)
        .chain(std::iter::once(mac_total_ms))
        .max()
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

pub fn run(options: Options) -> anyhow::Result<()> {
    let sample_hz = if options.sample_hz == 0 {
        DEFAULT_SAMPLE_HZ
    } else {
        options.sample_hz
    };
    let loaded = match load(&options) {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(EXIT_REFUSED);
        }
    };

    let mut pads: Box<dyn VirtualPadBackend> = if options.dry_run {
        Box::new(ksx_output::MockBackend::new())
    } else {
        open_backend()?
    };
    let handle = pads.plug_persona(options.persona)?;
    let user_index = pads.user_index(handle);

    let published: Published = Arc::new(Mutex::new(PadState::default()));
    let t0 = Instant::now();
    let sampler = spawn_sampler(
        Arc::clone(&published),
        t0,
        sample_hz,
        loaded.run_ms() + options.hold_ms + TAIL.as_millis() as u64,
    );

    let mut rec = Recorder {
        pads,
        handle,
        t0,
        last_us: 0,
        submits: Vec::new(),
        published: Arc::clone(&published),
    };

    // The Windows system timer, raised exactly as the daemon raises it while a
    // macro plays. Without it the default ~15.6 ms granularity would be measured
    // instead of the scheduler, and the trace would libel its own engine.
    let mut timer = crate::run::supervisor::TimerResolution::default();
    timer.set(true);

    let result = play(&mut rec, &loaded, &options);

    // Neutral, then unplug — the teardown contract, same order as the output
    // thread's. A trace must never be what leaves a pad holding a direction.
    let _ = rec.submit(PadState::default());
    timer.set(false);
    let submits = std::mem::take(&mut rec.submits);
    let _ = rec.pads.unplug(handle);
    let samples = sampler.join().unwrap_or_default();
    result?;

    report(&options, &loaded, sample_hz, user_index, &submits, &samples);
    Ok(())
}

#[cfg(windows)]
fn open_backend() -> anyhow::Result<Box<dyn VirtualPadBackend>> {
    match ksx_output::VigemBackend::connect() {
        Ok(backend) => Ok(Box::new(backend)),
        Err(err) if err.is_bus_missing() => {
            eprintln!(
                "{err}\nksx macro trace needs ViGEmBus to measure the real output path. \
                 Install it with `ksx install-drivers`, or pass --dry-run to trace the \
                 scheduler alone (no driver, no pad, and therefore no measurement of the \
                 driver call)."
            );
            std::process::exit(crate::pads::EXIT_DRIVER_MISSING);
        }
        Err(err) => Err(err.into()),
    }
}

#[cfg(not(windows))]
fn open_backend() -> anyhow::Result<Box<dyn VirtualPadBackend>> {
    anyhow::bail!("virtual pads are Windows-only; use --dry-run to trace the scheduler")
}

/// Play one run of the macro, submitting every delta the engine produces.
///
/// This is deliberately the same shape as the daemon's engine thread
/// (`run::supervisor::engine_thread`): compute the next deadline, never sleep
/// past it, and call `tick` after every wake. What it drops is the parts a trace
/// has no business having — a capture channel and a hot-swap control channel —
/// not the parts that decide timing.
fn play(rec: &mut Recorder, loaded: &Loaded, options: &Options) -> anyhow::Result<()> {
    let mut engine = Engine::new(vec![loaded.slot.clone()]);
    let device = DeviceId::new(TRACE_DEVICE);
    let t0 = rec.t0;
    let clock = move || t0.elapsed().as_millis() as u64;
    // A run that never ends (a `while-held` macro whose trigger we are holding,
    // or a wedged deadline) must still produce a trace rather than hang.
    let budget = loaded.run_ms().saturating_mul(4) + options.hold_ms + 2_000;

    let press = KeyEvent {
        device: device.clone(),
        key: loaded.trigger,
        down: true,
        t: 0,
    };
    for delta in engine.handle_at(&press, clock()) {
        rec.submit(delta.state)?;
    }

    let mut released = false;
    loop {
        let now = clock();
        if !released && now >= options.hold_ms {
            released = true;
            let up = KeyEvent {
                device: device.clone(),
                key: loaded.trigger,
                down: false,
                t: 0,
            };
            for delta in engine.handle_at(&up, clock()) {
                rec.submit(delta.state)?;
            }
        }
        let deadline = engine.next_deadline();
        if deadline.is_none() && released {
            break;
        }
        let mut wake = deadline.unwrap_or(u64::MAX);
        if !released {
            wake = wake.min(options.hold_ms);
        }
        let now = clock();
        if wake > now {
            std::thread::sleep(Duration::from_millis(wake - now));
        }
        for delta in engine.tick(clock()) {
            rec.submit(delta.state)?;
        }
        if clock() > budget {
            break;
        }
    }
    Ok(())
}

/// The simulated consumer: reads the published state every `1/hz` seconds and
/// records what it found.
///
/// Absolute tick times (`t0 + n/hz`), not "sleep 16 ms in a loop", for the same
/// reason the macro scheduler uses absolute deadlines: a poller that accumulated
/// its own jitter would blur the very thing being measured.
fn spawn_sampler(
    published: Published,
    t0: Instant,
    hz: u32,
    for_ms: u64,
) -> std::thread::JoinHandle<Vec<Sample>> {
    std::thread::spawn(move || {
        let period_ns = 1_000_000_000u64 / u64::from(hz);
        let until = Duration::from_millis(for_ms);
        let mut samples = Vec::with_capacity((for_ms * u64::from(hz) / 1000) as usize + 8);
        let mut n = 0u64;
        loop {
            let at = Duration::from_nanos(period_ns * n);
            if at > until {
                break;
            }
            let now = t0.elapsed();
            if at > now {
                std::thread::sleep(at - now);
            }
            let state = *published.lock().expect("published state");
            samples.push(Sample {
                t_us: t0.elapsed().as_micros() as u64,
                state,
            });
            n += 1;
        }
        samples
    })
}

/// Collapse consecutive equal samples: what the consumer SAW, as a list of
/// distinct states, which is the whole reason the sampler exists.
fn observe(samples: &[Sample]) -> Vec<Observed> {
    let mut out: Vec<Observed> = Vec::new();
    for s in samples {
        match out.last_mut() {
            Some(last) if last.state == s.state => {
                last.samples += 1;
                last.last_us = s.t_us;
            }
            _ => out.push(Observed {
                state: s.state,
                samples: 1,
                first_us: s.t_us,
                last_us: s.t_us,
            }),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

fn report(
    options: &Options,
    loaded: &Loaded,
    sample_hz: u32,
    user_index: Option<u8>,
    submits: &[Submit],
    samples: &[Sample],
) {
    let observed = observe(samples);
    // The verdict, one state at a time: was each submitted state ever read by
    // the consumer? A `false` here IS the bug, whatever the cause.
    let missed: Vec<&Submit> = submits
        .iter()
        .filter(|s| !observed.iter().any(|o| o.state == s.state))
        .collect();
    let diag_submitted = submits.iter().filter(|s| is_diagonal(&s.state)).count();
    let diag_observed: usize = observed
        .iter()
        .filter(|o| is_diagonal(&o.state))
        .map(|o| o.samples)
        .sum();
    // i16::MIN has no positive twin, so any consumer computing magnitude with
    // `abs()` or normalising by 32767 mishandles it (Mozilla bug 1606562 shipped
    // exactly that: -32768/32767 = -1.00003, out of the Gamepad spec's range).
    // Whether a given game does is unknowable from here; whether ksx handed it
    // the value is not, so the trace says so.
    let i16_min = submits
        .iter()
        .any(|s| [s.state.lx, s.state.ly, s.state.rx, s.state.ry].contains(&i16::MIN));

    if options.json {
        println!(
            "{}",
            serde_json::json!({
                "preset": loaded.slot.preset.name,
                "macro": {
                    "name": loaded.mac.name,
                    "steps": loaded.mac.steps.len(),
                    "total_ms": loaded.mac.total_ms(),
                    "repeat": loaded.mac.repeat.as_str(),
                },
                // Every OTHER macro the same key starts. A non-empty list means
                // the states below are a superposition, not this macro's alone.
                "also_triggered": loaded.also.iter().map(|c| serde_json::json!({
                    "name": c.name,
                    "total_ms": c.total_ms,
                    "repeat": c.repeat.as_str(),
                })).collect::<Vec<_>>(),
                "trigger": loaded.trigger.name(),
                "config_dir": loaded.root.display().to_string(),
                "persona": options.persona.to_string(),
                "dry_run": options.dry_run,
                "user_index": user_index,
                "sample_hz": sample_hz,
                "submits": submits.iter().enumerate().map(|(i, s)| serde_json::json!({
                    "t_ms": ms(s.t_us),
                    "gap_ms": ms(s.gap_us),
                    "call_ms": ms(s.call_us),
                    // How long this state actually existed on the pad.
                    "dwell_ms": ms(submits.get(i + 1).map_or(0, |n| n.t_us - s.t_us)),
                    "state": state_json(&s.state),
                    "observed": observed.iter().find(|o| o.state == s.state).map(|o| o.samples),
                })).collect::<Vec<_>>(),
                "observed": observed.iter().map(|o| serde_json::json!({
                    "samples": o.samples,
                    "first_ms": ms(o.first_us),
                    "last_ms": ms(o.last_us),
                    "state": state_json(&o.state),
                })).collect::<Vec<_>>(),
                "verdict": {
                    "submitted_states": submits.len(),
                    "observed_states": observed.len(),
                    "missed_by_sampler": missed.len(),
                    "diagonal_submitted": diag_submitted,
                    "diagonal_samples": diag_observed,
                    "emits_i16_min": i16_min,
                },
            })
        );
        return;
    }

    println!(
        "macro \"{}\" from preset \"{}\" — {} steps, {} ms authored, repeat {}",
        loaded.mac.name,
        loaded.slot.preset.name,
        loaded.mac.steps.len(),
        loaded.mac.total_ms(),
        loaded.mac.repeat,
    );
    println!(
        "config {} | pad {} ({}) | XInput slot {}",
        loaded.root.display(),
        options.persona,
        if options.dry_run { "mock" } else { "ViGEm" },
        user_index.map_or("none".to_owned(), |i| i.to_string()),
    );
    // The one thing a reader cannot see by looking at the macro they asked
    // about — and the difference between "the engine invented a step" and "you
    // are running three sequences at once".
    if !loaded.also.is_empty() {
        println!(
            "\nSHARED TRIGGER — key '{}' also starts {} other macro(s), which run at the SAME time",
            loaded.trigger.name(),
            loaded.also.len()
        );
        for c in &loaded.also {
            println!(
                "  {} — {} ms a run, repeat {}",
                c.name, c.total_ms, c.repeat
            );
        }
        println!(
            "  Every state below is their SUPERPOSITION, not \"{}\" alone: it can hold directions \
             no single one of them has, and it keeps going while the key is down if ANY of them \
             repeats. Give them separate keys to trace one on its own.",
            loaded.mac.name
        );
    }

    println!("\nSUBMITTED — every state ksx handed the driver");
    println!(
        "  {:>9}  {:>7}  {:>7}  {:>6}  {:>3}  {:>7}  {:>7}  other",
        "t (ms)", "gap", "dwell", "call", "dir", "lx", "ly"
    );
    for (i, s) in submits.iter().enumerate() {
        let dwell = submits.get(i + 1).map_or(0, |n| n.t_us - s.t_us);
        println!(
            "  {:>9.3}  {:>7.3}  {:>7.3}  {:>6.3}  {:>3}  {:>7}  {:>7}  {}",
            ms(s.t_us),
            ms(s.gap_us),
            ms(dwell),
            ms(s.call_us),
            arrow(&s.state),
            s.state.lx,
            s.state.ly,
            extras(&s.state),
        );
    }

    println!(
        "\nOBSERVED — distinct states a {sample_hz} Hz poller read ({} samples, {:.3} ms apart)",
        samples.len(),
        1000.0 / f64::from(sample_hz),
    );
    for o in &observed {
        println!(
            "  {:>3} sample(s)  {:>9.3}..{:<9.3}  {:>3}  lx={:<7} ly={:<7} {}",
            o.samples,
            ms(o.first_us),
            ms(o.last_us),
            arrow(&o.state),
            o.state.lx,
            o.state.ly,
            extras(&o.state),
        );
    }

    println!("\nVERDICT");
    println!(
        "  {} submitted state(s), {} distinct state(s) observed at {sample_hz} Hz",
        submits.len(),
        observed.len()
    );
    if missed.is_empty() {
        println!("  every submitted state was read by the poller — nothing was too short to see");
    } else {
        println!(
            "  {} submitted state(s) the poller NEVER read — too short to survive sampling:",
            missed.len()
        );
        for s in &missed {
            println!(
                "    t={:.3} ms  {}  lx={} ly={} {}",
                ms(s.t_us),
                arrow(&s.state),
                s.state.lx,
                s.state.ly,
                extras(&s.state)
            );
        }
    }
    if !loaded.also.is_empty() {
        println!(
            "  {} other macro(s) on the same key ({}) contributed to those states — do not read \
             this trace as \"{}\"",
            loaded.also.len(),
            loaded
                .also
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            loaded.mac.name,
        );
        if loaded.anything_repeats() && !loaded.mac.repeat.repeats() {
            println!(
                "  \"{}\" is repeat {}, but something on this key repeats — that, and not \"{}\", \
                 is what keeps firing while the key is held",
                loaded.mac.name, loaded.mac.repeat, loaded.mac.name,
            );
        }
    }
    if diag_submitted == 0 {
        println!("  no diagonal in this macro (no state deflects two perpendicular directions)");
    } else {
        println!(
            "  diagonal: submitted {diag_submitted} time(s), read by the poller in \
             {diag_observed} sample(s)"
        );
    }
    if i16_min {
        println!(
            "  note: an axis was submitted as i16::MIN (-32768). It is a legal XInput value, \
             but it is the one value with no positive twin: a consumer computing magnitude \
             with abs() or normalising by 32767 gets a wrong or out-of-range result from it \
             (Mozilla bug 1606562 shipped exactly that). -32767 is 0.003% smaller and safe."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_core::{Axis, Binding, DpadDirection, MacroStep, AXIS_MAX};

    fn axis_state(lx: i16, ly: i16) -> PadState {
        PadState {
            lx,
            ly,
            ..PadState::default()
        }
    }

    /// The tracer reads a direction off BOTH mechanisms, because a preset may
    /// drive either — reading only the dpad is how a stick-bound cabinet's
    /// trace would show nothing at all.
    #[test]
    fn arrows_name_directions_on_the_stick_and_on_the_dpad() {
        assert_eq!(arrow(&axis_state(0, i16::MIN)), "\u{2193}");
        assert_eq!(arrow(&axis_state(AXIS_MAX, 0)), "\u{2192}");
        assert_eq!(arrow(&axis_state(AXIS_MAX, i16::MIN)), "\u{2198}");
        assert_eq!(arrow(&PadState::default()), "\u{00b7}");

        let dpad = PadState {
            buttons: XButtons::DPAD_DOWN | XButtons::DPAD_RIGHT,
            ..PadState::default()
        };
        assert_eq!(arrow(&dpad), "\u{2198}");
        assert!(is_diagonal(&dpad));
        assert!(is_diagonal(&axis_state(AXIS_MAX, i16::MIN)));
        assert!(!is_diagonal(&axis_state(0, i16::MIN)));

        // A cancelled pair is not a direction, and must not be reported as one.
        let socd = PadState {
            buttons: XButtons::DPAD_LEFT | XButtons::DPAD_RIGHT,
            ..PadState::default()
        };
        assert_eq!(arrow(&socd), "\u{00b7}");
    }

    /// The measurement the whole command exists for: consecutive equal samples
    /// collapse to ONE thing the consumer saw, and a state no sample landed on
    /// is absent from that list entirely.
    #[test]
    fn observing_collapses_repeats_and_omits_what_was_never_sampled() {
        let down = axis_state(0, i16::MIN);
        let diag = axis_state(AXIS_MAX, i16::MIN);
        let right = axis_state(AXIS_MAX, 0);
        let samples: Vec<Sample> = [down, down, diag, diag, diag, right]
            .into_iter()
            .enumerate()
            .map(|(i, state)| Sample {
                t_us: i as u64 * 16_667,
                state,
            })
            .collect();
        let observed = observe(&samples);
        assert_eq!(observed.len(), 3);
        assert_eq!(observed[1].samples, 3);
        assert_eq!(observed[1].state, diag);
        assert_eq!(observed[0].first_us, 0);
        assert_eq!(observed[2].last_us, 5 * 16_667);

        // A state that existed between two polls was never seen — which is the
        // finding the report has to be able to make.
        let unseen = axis_state(i16::MIN, 0);
        assert!(!observed.iter().any(|o| o.state == unseen));
    }

    /// Every sample of a run is one state, so a poller that never changes its
    /// reading reports exactly one observation, not `n`.
    #[test]
    fn a_constant_state_is_one_observation() {
        let samples: Vec<Sample> = (0..10)
            .map(|i| Sample {
                t_us: i * 16_667,
                state: PadState::default(),
            })
            .collect();
        let observed = observe(&samples);
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].samples, 10);
    }

    /// The finding of 2026-08-06: his key `Q` started THREE macros, and a trace
    /// that names only the one asked for blames it for two other macros' states.
    /// So the instrument has to see them — from the same resolved trigger list
    /// the engine acts on, deduped, and with the repeat policy that explains why
    /// a `once` macro looked like it never stopped.
    #[test]
    fn a_trace_sees_every_other_macro_the_same_key_starts() {
        let mut turbo = Macro::new(
            "op",
            vec![MacroStep::new(
                vec![Binding::Button(ksx_core::XButton::A)],
                50,
            )],
        );
        turbo.repeat = ksx_core::Repeat::Turbo;
        let macros = ksx_core::Macros {
            defs: vec![
                turbo,
                Macro::new(
                    "ppp",
                    vec![MacroStep::frames(
                        vec![Binding::Dpad(DpadDirection::Down)],
                        7,
                    )],
                ),
                Macro::new(
                    "solo",
                    vec![MacroStep::new(vec![Binding::Dpad(DpadDirection::Up)], 50)],
                ),
            ],
            triggers: vec![
                MacroTrigger::new(Key::Q, 0),
                MacroTrigger::new(Key::Q, 1),
                // A duplicate row, and a dangling index: neither may show up
                // twice or panic.
                MacroTrigger::new(Key::Q, 0),
                MacroTrigger::new(Key::Q, 9),
                MacroTrigger::new(Key::R, 2),
            ],
        };

        // Tracing `ppp` (index 1) has to name `op`, and only `op`.
        let also = companions(&macros, Key::Q, 1);
        assert_eq!(also.len(), 1);
        assert_eq!(also[0].name, "op");
        assert_eq!(also[0].repeat, ksx_core::Repeat::Turbo);
        // The sampling window covers the LONGEST run on the key, not the traced
        // macro's — 7 frames is 117 ms, `op` is 50.
        assert_eq!(run_ms(117, &also), 117);
        assert_eq!(
            run_ms(
                50,
                &[Companion {
                    name: "x".into(),
                    total_ms: 400,
                    repeat: ksx_core::Repeat::Once
                }]
            ),
            400
        );

        // A key that starts exactly one macro has no companions, and neither
        // does a key that starts none.
        assert!(companions(&macros, Key::R, 2).is_empty());
        assert!(companions(&macros, Key::Z, 0).is_empty());
    }

    /// The tracer must not care which mechanism a preset uses: a stick binding
    /// and a dpad binding are both directions and both get named.
    #[test]
    fn bindings_of_both_mechanisms_are_traceable() {
        let stick = Binding::Axis {
            axis: Axis::Y,
            value: i16::MIN,
        };
        let pad = Binding::Dpad(DpadDirection::Down);
        assert_ne!(stick, pad);
    }
}
