//! `InterceptionBackend` — per-device keyboard capture via the Interception
//! driver (day-one default, scheduled for retirement at M6; see
//! `docs/research/keyboard-capture-2026.md` §1/§7 and risk review R1).
//!
//! # Why `kanata_interception::raw` instead of the safe wrapper
//!
//! Verified against the crate source in the registry cache
//! (`kanata-interception-0.3.0/src/lib.rs`):
//!
//! 1. **The safe `KeyState` bitflags define `E1 = 3`; the driver header (and
//!    `interception-sys`, and legacy C#) say `E1 = 0x04`.** A real E1 stroke
//!    (the Pause make, state `0x04`/`0x05`) fails `KeyState::from_bits` and the
//!    safe `receive()` *silently discards it* — the stroke is consumed from the
//!    driver but never re-sent, so the Pause key would die for every keyboard
//!    on the system while we run. Unacceptable.
//! 2. The safe `receive`/`send` allocate a `Vec` per call — the hot path must
//!    not allocate after startup.
//! 3. The safe `ScanCode` conversion maps unknown codes to `Esc`, corrupting a
//!    re-sent stroke. Re-sends must be byte-for-byte what the driver gave us.
//!
//! The raw FFI (`interception-sys 0.1.3`, re-exported as
//! `kanata_interception::raw`) has the correct constants and lets us keep the
//! `InterceptionKeyStroke` untouched from receive to send.
//!
//! # Threading and safety model
//!
//! `run` owns the only thread that ever touches the context after start. The
//! hot loop does exactly: wait (with timeout so ctl is honored), receive,
//! decide from an `arc-swap` snapshot, re-send non-captured strokes verbatim,
//! `try_send` the event (never block). No locks, no allocation after startup
//! except (a) the one-time `SlotEntry` build when a *new* device appears
//! (hotplug — rare by definition) and (b) the `DeviceId` clone each reported
//! `KeyEvent` requires by ksx-core contract (small, amortized; noted in the M3
//! summary as a candidate for `Arc<str>` in M4).
//!
//! Crash safety: [`Ctx`]'s `Drop` is the guard — it sets both class filters to
//! `NONE` **before** destroying the context, and it runs on panic unwind, on
//! normal return, and if the backend is dropped without ever running. Process
//! death needs no cleanup at all (the driver releases filters when the handle
//! closes); the guard exists for the deadly middle case — a dead capture
//! thread inside a living process (risk review §3 item 1).
//!
//! Thread priority is set inline with `windows-sys` rather than through
//! `ksx-platform`: one syscall does not justify an inter-crate dependency from
//! the capture hot path (deliberate design decision, see M3 notes).

use std::sync::Arc;

use arc_swap::ArcSwap;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError};
use kanata_interception::raw;
use ksx_core::{DeviceId, KeyEvent};

use crate::backend::{
    CaptureBackend, CaptureCtl, CaptureError, DeviceInfo, DeviceKind, ExitReason,
};
use crate::decision::process_keyboard_stroke;
use crate::exhaustion::{Exhaustion, ExhaustionDetector};
use crate::friendly;
use crate::health::HealthHandle;
use crate::watchdog::Watchdog;

/// Wait timeout: bounds both ctl-message latency and shutdown latency.
const WAIT_TIMEOUT_MS: u32 = 50;
/// Strokes drained per receive call (driver queues at most a handful).
const RECEIVE_BATCH: usize = 32;
/// Legacy `Interception.HardwareIdSize` (chars).
const HWID_BUF_CHARS: usize = 500;
/// Interception device ids: 1..=10 keyboards, 11..=20 mice.
const MAX_DEVICE: usize = 20;
const FILTER_KEY_ALL: u16 = 0xFFFF;
const FILTER_NONE: u16 = 0;

/// Owning wrapper around the raw Interception context.
///
/// `Drop` is the crash-safety guard: filters to NONE, then destroy. Runs on
/// panic unwind too — a panicking capture thread must never leave a filter
/// armed with nobody pumping `receive` (that deadens every keyboard until
/// reboot).
struct Ctx(raw::InterceptionContext);

// SAFETY: the context is created on one thread and moved into the capture
// thread; it is only ever used from one thread at a time. The Interception API
// itself is documented to be usable this way (legacy did the same).
unsafe impl Send for Ctx {}

impl Ctx {
    fn reset_filters(&self) {
        // SAFETY: valid context; is_keyboard/is_mouse are the canonical
        // predicates exported by the driver library itself.
        unsafe {
            raw::interception_set_filter(self.0, Some(raw::interception_is_keyboard), FILTER_NONE);
            raw::interception_set_filter(self.0, Some(raw::interception_is_mouse), FILTER_NONE);
        }
    }

    /// Re-send any keyboard strokes the driver captured for us that we never
    /// fetched, so dying mid-flight cannot swallow them (worst case otherwise:
    /// a lost key-up leaves a modifier stuck at OS level). Bounded so cleanup
    /// can never become its own hang; on the never-ran path the filter was
    /// never set, the zero-timeout wait returns immediately, and this is free.
    fn drain_resend_pending(&self) {
        let mut stroke = raw::InterceptionKeyStroke::default();
        for _ in 0..64 {
            // SAFETY: valid context; zero timeout returns 0 when idle.
            let dev = unsafe { raw::interception_wait_with_timeout(self.0, 0) };
            // SAFETY: predicates accept any value.
            if dev == 0
                || unsafe { raw::interception_is_invalid(dev) } != 0
                || unsafe { raw::interception_is_keyboard(dev) } == 0
            {
                break; // idle, or not a keyboard (mouse filter is never set)
            }
            // SAFETY: one-stroke keyboard receive/send, same layout contract
            // as the hot loop; the stroke is re-sent byte-for-byte.
            unsafe {
                let n = raw::interception_receive(
                    self.0,
                    dev,
                    (&mut stroke as *mut raw::InterceptionKeyStroke).cast(),
                    1,
                );
                if n < 1 {
                    break;
                }
                raw::interception_send(
                    self.0,
                    dev,
                    (&stroke as *const raw::InterceptionKeyStroke).cast(),
                    1,
                );
            }
        }
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        self.drain_resend_pending();
        self.reset_filters();
        // SAFETY: self.0 is a live context; after this call it is never used
        // again (we are in drop).
        unsafe { raw::interception_destroy_context(self.0) };
    }
}

/// Per-slot identity as the capture thread sees it.
struct SlotEntry {
    id: DeviceId,
}

type Slots = [Option<SlotEntry>; MAX_DEVICE + 1];

/// The arc-swap'd snapshot the hot loop decides from. Rebuilt (cold path) on
/// every ctl message or slot-table change; read (lock-free, alloc-free) per
/// batch.
struct SlotDecision {
    passthrough: bool,
    captured: [bool; MAX_DEVICE + 1],
}

impl SlotDecision {
    fn passthrough_all() -> Self {
        Self {
            passthrough: true,
            captured: [false; MAX_DEVICE + 1],
        }
    }
}

/// Interception-driver capture backend. Starts in passthrough mode: nothing is
/// suppressed until `CaptureCtl::SetCaptured` arrives.
pub struct InterceptionBackend {
    ctx: Ctx,
    health: HealthHandle,
}

impl InterceptionBackend {
    /// Creates the driver context. Harmless on its own: no filter is set until
    /// [`CaptureBackend::run`], so constructing (e.g. for `devices()`
    /// enumeration) cannot affect the machine's keyboards.
    pub fn new() -> Result<Self, CaptureError> {
        // SAFETY: plain FFI constructor; null-checked below.
        let ctx = unsafe { raw::interception_create_context() };
        if ctx.is_null() {
            return Err(CaptureError::DriverUnavailable);
        }
        // Loud EOL warning by design (design §1.2, risk review R1): this
        // backend is a bridge with a scheduled retirement at M6. The full
        // keyboard.sys-signature + CI-policy probe belongs to `ksx doctor`
        // (ksx-platform follow-up); the warning fires unconditionally here.
        tracing::warn!(
            "InterceptionBackend active: the Interception driver is end-of-life \
             (2012 cross-signed keyboard.sys; Microsoft's CI-policy enforcement \
             cliff is live as of 2026-08). If this machine's CI policy flips to \
             enforcement, ALL keyboards can die at boot (Code 39) — keep \
             docs/RECOVERY.md at hand and plan on the WinUSB backend (M6)."
        );
        Ok(Self {
            ctx: Ctx(ctx),
            health: HealthHandle::new(),
        })
    }
}

impl CaptureBackend for InterceptionBackend {
    fn devices(&mut self) -> Vec<DeviceInfo> {
        let mut out = Vec::new();
        // Full 1..=20. (Legacy `RescanInputDevices` looped `id < 20` and never
        // enumerated slot 20 — confirmed off-by-one, fixed here.)
        for slot in 1..=MAX_DEVICE as i32 {
            // SAFETY: predicates take any device number by design.
            if unsafe { raw::interception_is_invalid(slot) } != 0 {
                continue;
            }
            let Some(hwid) = hardware_id(&self.ctx, slot) else {
                continue;
            };
            // SAFETY: as above.
            let kind = if unsafe { raw::interception_is_keyboard(slot) } != 0 {
                DeviceKind::Keyboard
            } else {
                DeviceKind::Mouse
            };
            let friendly = friendly::friendly_name(&hwid, kind);
            out.push(DeviceInfo {
                id: DeviceId::from(hwid),
                interception_slot: Some(slot as u8),
                friendly,
                kind,
            });
        }
        out
    }

    fn health(&self) -> HealthHandle {
        self.health.clone()
    }

    fn run(
        self: Box<Self>,
        tx: Sender<KeyEvent>,
        ctl: Receiver<CaptureCtl>,
    ) -> std::io::Result<std::thread::JoinHandle<ExitReason>> {
        let InterceptionBackend { ctx, health } = *self;
        std::thread::Builder::new()
            .name("ksx-capture-interception".into())
            .spawn(move || {
                set_time_critical_priority();
                // `ctx` is owned by this closure; its Drop (filters NONE +
                // destroy) runs on every exit path, including unwind. The
                // catch_unwind below only exists to flag health and convert
                // the panic into a clean ExitReason.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    capture_loop(&ctx, &health, &tx, &ctl)
                }));
                match result {
                    Ok(reason) => reason,
                    Err(_) => {
                        health.set_panicked();
                        tracing::error!(
                            "capture loop panicked — drop guard resets the filter; keyboards keep working"
                        );
                        ExitReason::Panicked
                    }
                }
                // `ctx` dropped here: filter NONE, context destroyed.
            })
    }
}

fn capture_loop(
    ctx: &Ctx,
    health: &HealthHandle,
    tx: &Sender<KeyEvent>,
    ctl: &Receiver<CaptureCtl>,
) -> ExitReason {
    let start = std::time::Instant::now();
    let mut wd = Watchdog::default();
    let mut exhaustion = ExhaustionDetector::new();
    let mut slots: Slots = std::array::from_fn(|_| None);
    let mut captured_ids: Vec<DeviceId> = Vec::new();
    let decision: ArcSwap<SlotDecision> = ArcSwap::from_pointee(SlotDecision::passthrough_all());
    let unknown_device = DeviceId::from("<unknown-interception-device>");

    // Seed slot identities + the exhaustion baseline BEFORE opening the tap.
    rescan(ctx, &mut slots, &mut exhaustion, health);

    // Open the keyboard tap. The mouse filter is NEVER set in M3 — we must not
    // touch mouse.sys behavior at all.
    // SAFETY: valid context, canonical predicate.
    unsafe {
        raw::interception_set_filter(ctx.0, Some(raw::interception_is_keyboard), FILTER_KEY_ALL);
    }

    let mut kb_buf = [raw::InterceptionKeyStroke::default(); RECEIVE_BATCH];
    let mut mouse_buf = [raw::InterceptionMouseStroke::default(); RECEIVE_BATCH];

    loop {
        // Drain control first; wait's timeout guarantees we come back here at
        // least every WAIT_TIMEOUT_MS.
        loop {
            match ctl.try_recv() {
                Ok(CaptureCtl::SetCaptured(ids)) => {
                    captured_ids = ids;
                    // Re-arm a tripped watchdog: the supervisor re-enabling
                    // capture is a recovery decision, but if the consumer is
                    // still stalled the protection must be able to fire again
                    // (otherwise captured keyboards black-hole with no guard).
                    // The health `watchdog_tripped` flag stays latched.
                    if wd.tripped() {
                        wd = Watchdog::default();
                    }
                    publish(&decision, &slots, &captured_ids, false);
                }
                Ok(CaptureCtl::SetPassthrough) => {
                    publish(&decision, &slots, &captured_ids, true);
                }
                Ok(CaptureCtl::Shutdown) => return ExitReason::Shutdown,
                Err(TryRecvError::Empty) => break,
                // Controller gone: nobody can ever release a captured device
                // again, so stop capturing entirely.
                Err(TryRecvError::Disconnected) => return ExitReason::Shutdown,
            }
        }

        // SAFETY: valid context; returns 0 on timeout.
        let dev = unsafe { raw::interception_wait_with_timeout(ctx.0, WAIT_TIMEOUT_MS) };
        // SAFETY: predicate accepts any value.
        if dev == 0 || unsafe { raw::interception_is_invalid(dev) } != 0 {
            continue;
        }

        // SAFETY: predicate accepts any value.
        if unsafe { raw::interception_is_mouse(dev) } != 0 {
            // Mouse filter is NONE, so this should be unreachable; if the
            // driver hands us mouse strokes anyway, re-send them verbatim so
            // nothing is ever lost. Never reported in M3.
            // SAFETY: buffer of RECEIVE_BATCH raw mouse strokes; the driver
            // treats the pointer as a packed InterceptionMouseStroke array for
            // mouse devices (same layout contract the safe wrapper relies on).
            let n = unsafe {
                raw::interception_receive(
                    ctx.0,
                    dev,
                    mouse_buf.as_mut_ptr().cast(),
                    RECEIVE_BATCH as u32,
                )
            };
            if n > 0 {
                // SAFETY: sending back the exact strokes just received.
                unsafe { raw::interception_send(ctx.0, dev, mouse_buf.as_ptr().cast(), n as u32) };
            }
            continue;
        }

        // Keyboard stroke(s).
        // SAFETY: same layout contract as above, for InterceptionKeyStroke.
        let n = unsafe {
            raw::interception_receive(ctx.0, dev, kb_buf.as_mut_ptr().cast(), RECEIVE_BATCH as u32)
        };
        if n <= 0 {
            continue;
        }

        let slot = dev as usize; // 1..=10 after the is_invalid/is_mouse checks
        if slots[slot].is_none() {
            // Cold path: a device appeared on a slot we hadn't seen (hotplug,
            // or an id climbing after replug — the exhaustion signal).
            if let Some(hwid) = hardware_id(ctx, dev) {
                note_exhaustion(exhaustion.observe_keyboard(dev, &hwid), health);
                slots[slot] = Some(SlotEntry {
                    id: DeviceId::from(hwid),
                });
                let passthrough = decision.load().passthrough;
                publish(&decision, &slots, &captured_ids, passthrough);
            }
        }

        let mut snap = decision.load();
        for stroke in &kb_buf[..n as usize] {
            let (device, is_captured) = match slots[slot].as_ref() {
                Some(e) => (&e.id, snap.captured[slot]),
                // No hwid obtainable: treat as unknown device — never suppress
                // what we cannot attribute.
                None => (&unknown_device, false),
            };

            let out = process_keyboard_stroke(
                snap.passthrough,
                is_captured,
                device,
                stroke.code,
                stroke.state,
                qpc_now(),
            );

            if out.resend {
                // Re-send FIRST (latency to the OS), byte-for-byte: `stroke`
                // is untouched since receive — code, state and information all
                // preserved. A corrupted re-send breaks every keyboard.
                // SAFETY: one valid stroke, same layout contract as receive.
                unsafe {
                    raw::interception_send(
                        ctx.0,
                        dev,
                        (stroke as *const raw::InterceptionKeyStroke).cast(),
                        1,
                    );
                }
            }

            match tx.try_send(out.event) {
                Ok(()) => wd.on_send_ok(),
                Err(TrySendError::Full(_)) => {
                    health.add_dropped(1);
                    let now_ms = start.elapsed().as_millis() as u64;
                    if wd.on_send_failed(now_ms) {
                        tracing::error!(
                            threshold_ms = Watchdog::DEFAULT_THRESHOLD_MS,
                            "event consumer stalled — forcing passthrough so keystrokes reach the OS"
                        );
                        health.set_watchdog_tripped();
                        publish(&decision, &slots, &captured_ids, true);
                        snap = decision.load(); // rest of the batch passes through
                    }
                }
                Err(TrySendError::Disconnected(_)) => return ExitReason::ChannelClosed,
            }
        }
    }
}

/// Rebuild + publish the per-slot decision snapshot (cold path).
fn publish(
    decision: &ArcSwap<SlotDecision>,
    slots: &Slots,
    captured_ids: &[DeviceId],
    passthrough: bool,
) {
    let mut captured = [false; MAX_DEVICE + 1];
    for (i, entry) in slots.iter().enumerate() {
        if let Some(e) = entry {
            captured[i] = captured_ids.iter().any(|c| c == &e.id);
        }
    }
    decision.store(Arc::new(SlotDecision {
        passthrough,
        captured,
    }));
}

/// Populate the slot table and the exhaustion baseline (cold path, pre-filter).
fn rescan(
    ctx: &Ctx,
    slots: &mut Slots,
    exhaustion: &mut ExhaustionDetector,
    health: &HealthHandle,
) {
    for slot in 1..=MAX_DEVICE as i32 {
        // SAFETY: predicates accept any device number.
        if unsafe { raw::interception_is_invalid(slot) } != 0 {
            continue;
        }
        if let Some(hwid) = hardware_id(ctx, slot) {
            // SAFETY: as above.
            if unsafe { raw::interception_is_keyboard(slot) } != 0 {
                note_exhaustion(exhaustion.observe_keyboard(slot, &hwid), health);
            }
            slots[slot as usize] = Some(SlotEntry {
                id: DeviceId::from(hwid),
            });
        }
    }
}

fn note_exhaustion(event: Option<Exhaustion>, health: &HealthHandle) {
    if let Some(event) = event {
        health.set_reboot_required();
        // Loud by design: the legacy app's silent version of this failure was
        // one of its worst traits (risk review R2 mitigation 4).
        tracing::error!(
            ?event,
            "Interception keyboard slot exhaustion — REBOOT REQUIRED; \
             affected keyboards are invisible to the driver until then"
        );
    }
}

/// Hardware id for a device slot, like legacy `GetHardwareID`: reject 0-length
/// and buffer-overflow results, take the first NUL-terminated wide string.
fn hardware_id(ctx: &Ctx, dev: i32) -> Option<String> {
    let mut buf = [0u16; HWID_BUF_CHARS];
    // SAFETY: buffer is HWID_BUF_CHARS u16s = 2x bytes, exactly what we pass.
    let bytes = unsafe {
        raw::interception_get_hardware_id(
            ctx.0,
            dev,
            buf.as_mut_ptr().cast(),
            (HWID_BUF_CHARS * 2) as u32,
        )
    };
    if bytes == 0 || bytes as usize >= HWID_BUF_CHARS * 2 {
        return None;
    }
    let chars = bytes as usize / 2;
    let end = buf[..chars].iter().position(|&c| c == 0).unwrap_or(chars);
    if end == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..end]))
}

/// QPC ticks — the `KeyEvent.t` unit on Windows (ksx-core only compares /
/// subtracts, unit is backend-defined).
fn qpc_now() -> u64 {
    let mut t: i64 = 0;
    // SAFETY: out-pointer to a stack i64; QPC cannot fail on XP+.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut t) };
    t as u64
}

/// Raise this thread to TIME_CRITICAL (legacy used Highest; the design doc
/// specifies TIME_CRITICAL for the Rust capture thread). Inline windows-sys on
/// purpose — no ksx-platform dependency for one syscall.
fn set_time_critical_priority() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
    };
    // SAFETY: pseudo-handle to the current thread; no ownership transferred.
    let ok = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) };
    if ok == 0 {
        tracing::warn!("SetThreadPriority(TIME_CRITICAL) failed; continuing at default priority");
    }
}
