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
//! evaluate emergency escapes (`crate::escape` — before any pass/suppress
//! decision, so `LeftCtrl ×5` frees the keyboards from *this* thread with no
//! channel in the way), decide from an `arc-swap` snapshot, re-send
//! non-captured strokes verbatim, `try_send` the event (never block). No locks,
//! no allocation after startup
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
//!
//! # Where the DLL comes from
//!
//! Every `interception_*` call below goes through [`crate::interception_dll`]'s
//! `GetProcAddress` table, never through `interception-sys`'s import stubs. That
//! is not a style choice: a static import makes `interception.dll` a **load-time
//! requirement of `ksx.exe` itself**, so `ksx --version` on a WinUSB-only
//! cabinet dies in the Windows loader before `main`. Only `Api` (loaded when
//! this backend is *constructed*) is allowed to need the DLL. The types are
//! still `kanata_interception::raw`'s — a `#[repr(C)]` declaration creates no
//! linkage, and a second hand-written `InterceptionKeyStroke` would be a second
//! source of truth for the layout the hot path re-sends byte-for-byte.

use std::sync::Arc;

use arc_swap::ArcSwap;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError};
use kanata_interception::raw;
use ksx_core::{DeviceId, KeyEvent};

use crate::interception_dll::Api;

use crate::backend::{
    CaptureBackend, CaptureCtl, CaptureError, DeviceInfo, DeviceKind, ExitReason,
};
use crate::decision::{key_event, should_resend};
use crate::escape::{EscapeHandle, EscapeWatch};
use crate::exhaustion::{Exhaustion, ExhaustionDetector};
use crate::friendly;
use crate::health::HealthHandle;
use crate::presence::PresenceHandle;
use crate::watchdog::Watchdog;

/// Wait timeout: bounds both ctl-message latency and shutdown latency.
const WAIT_TIMEOUT_MS: u32 = 50;
/// How often the slot table is re-read for hotplug (arrivals *and* removals).
///
/// Runs only on the idle path — after `interception_wait_with_timeout` reports
/// the driver has nothing pending — so it can never sit between a keystroke and
/// its re-send. Arrivals are also caught instantly by the unknown-slot path
/// below; this exists for removals, which produce no stroke to react to.
const PRESENCE_RESCAN_MS: u64 = 2_000;
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
struct Ctx {
    handle: raw::InterceptionContext,
    /// The resolved DLL entry points. Held rather than re-fetched so the hot
    /// loop never touches the `OnceLock` (and so `Drop` cannot fail to find the
    /// functions it needs to reset the filters with).
    api: &'static Api,
}

// SAFETY: the context is created on one thread and moved into the capture
// thread; it is only ever used from one thread at a time. The Interception API
// itself is documented to be usable this way (legacy did the same). `api` is a
// table of function pointers with 'static lifetime, which is Sync by nature.
unsafe impl Send for Ctx {}

impl Ctx {
    fn reset_filters(&self) {
        // SAFETY: valid context; is_keyboard/is_mouse are the canonical
        // predicates exported by the driver library itself.
        unsafe {
            (self.api.set_filter)(self.handle, Some(self.api.is_keyboard), FILTER_NONE);
            (self.api.set_filter)(self.handle, Some(self.api.is_mouse), FILTER_NONE);
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
            let dev = unsafe { (self.api.wait_with_timeout)(self.handle, 0) };
            // SAFETY: predicates accept any value.
            if dev == 0
                || unsafe { (self.api.is_invalid)(dev) } != 0
                || unsafe { (self.api.is_keyboard)(dev) } == 0
            {
                break; // idle, or not a keyboard (mouse filter is never set)
            }
            // SAFETY: one-stroke keyboard receive/send, same layout contract
            // as the hot loop; the stroke is re-sent byte-for-byte.
            unsafe {
                let n = (self.api.receive)(
                    self.handle,
                    dev,
                    (&mut stroke as *mut raw::InterceptionKeyStroke).cast(),
                    1,
                );
                if n < 1 {
                    break;
                }
                (self.api.send)(
                    self.handle,
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
        // SAFETY: self.handle is a live context; after this call it is never
        // used again (we are in drop).
        unsafe { (self.api.destroy_context)(self.handle) };
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
    presence: PresenceHandle,
    escapes: EscapeHandle,
}

impl InterceptionBackend {
    /// Creates the driver context. Harmless on its own: no filter is set until
    /// [`CaptureBackend::run`], so constructing (e.g. for `devices()`
    /// enumeration) cannot affect the machine's keyboards.
    pub fn new() -> Result<Self, CaptureError> {
        Self::new_with(crate::backend::Handles::new())
    }

    /// [`Self::new`] publishing into pre-existing handles.
    ///
    /// Since M6 a session can run this backend alongside one
    /// [`crate::WinUsbBackend`] per rebound board. They must share a health
    /// state and — the part that matters at 2 a.m. — a single escape latch, so
    /// `LeftCtrl x5` on the WinUSB-claimed I-PAC also drops this backend's
    /// keyboard filter. See [`crate::CompositeBackend`].
    pub fn new_with(handles: crate::backend::Handles) -> Result<Self, CaptureError> {
        // The DLL is loaded HERE and nowhere earlier: this is the first moment
        // ksx actually needs Interception, so it is the first moment its absence
        // may cost anything. Everything upstream — `ksx --version`, `ksx
        // devices`, `ksx doctor`, a WinUSB-only session — must run on a machine
        // that has never had the driver (M6's exit criterion).
        let api = crate::interception_dll::api()?;
        // SAFETY: plain FFI constructor; null-checked below.
        let ctx = unsafe { (api.create_context)() };
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
            ctx: Ctx { handle: ctx, api },
            health: handles.health,
            presence: PresenceHandle::new(),
            escapes: handles.escapes,
        })
    }
}

impl CaptureBackend for InterceptionBackend {
    fn devices(&mut self) -> Vec<DeviceInfo> {
        let mut out = Vec::new();
        let api = self.ctx.api;
        // Full 1..=20. (Legacy `RescanInputDevices` looped `id < 20` and never
        // enumerated slot 20 — confirmed off-by-one, fixed here.)
        for slot in 1..=MAX_DEVICE as i32 {
            // SAFETY: predicates take any device number by design.
            if unsafe { (api.is_invalid)(slot) } != 0 {
                continue;
            }
            let Some(hwid) = hardware_id(&self.ctx, slot) else {
                continue;
            };
            // SAFETY: as above.
            let kind = if unsafe { (api.is_keyboard)(slot) } != 0 {
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

    fn escapes(&self) -> EscapeHandle {
        self.escapes.clone()
    }

    fn presence(&self) -> PresenceHandle {
        self.presence.clone()
    }

    fn run(
        self: Box<Self>,
        tx: Sender<KeyEvent>,
        ctl: Receiver<CaptureCtl>,
    ) -> std::io::Result<std::thread::JoinHandle<ExitReason>> {
        let InterceptionBackend {
            ctx,
            health,
            presence,
            escapes,
        } = *self;
        std::thread::Builder::new()
            .name("ksx-capture-interception".into())
            .spawn(move || {
                set_time_critical_priority();
                // `ctx` is owned by this closure; its Drop (filters NONE +
                // destroy) runs on every exit path, including unwind. The
                // catch_unwind below only exists to flag health and convert
                // the panic into a clean ExitReason.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    capture_loop(&ctx, &health, &presence, &escapes, &tx, &ctl)
                }));
                match result {
                    Ok(reason) => reason,
                    Err(_) => {
                        // Flag, don't log: `ctx` has not dropped yet, so the
                        // filter is still armed. A blocking console write here
                        // would delay the reset that frees the keyboards.
                        health.set_panicked();
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
    presence: &PresenceHandle,
    escape_handle: &EscapeHandle,
    tx: &Sender<KeyEvent>,
    ctl: &Receiver<CaptureCtl>,
) -> ExitReason {
    let start = std::time::Instant::now();
    // Hoisted out of the hot loop: one field read instead of a `&Ctx` deref per
    // call. The pointers are immutable for the life of the process.
    let api = ctx.api;
    let mut wd = Watchdog::default();
    let mut exhaustion = ExhaustionDetector::new();
    let mut slots: Slots = std::array::from_fn(|_| None);
    let mut captured_ids: Vec<DeviceId> = Vec::new();
    let decision: ArcSwap<SlotDecision> = ArcSwap::from_pointee(SlotDecision::passthrough_all());
    let unknown_device = DeviceId::from("<unknown-interception-device>");
    let mut last_rescan_ms: u64 = 0;
    let mut escapes = EscapeWatch::new(escape_handle.clone());
    // The three independent sources of passthrough, kept apart so none of them
    // can silently clear another: what the supervisor asked for, what the stall
    // watchdog forced, and what an emergency escape latched.
    let mut ctl_passthrough = true;
    let mut watchdog_passthrough = false;

    // Seed slot identities + the exhaustion baseline BEFORE opening the tap.
    rescan(
        ctx,
        &mut slots,
        &mut exhaustion,
        health,
        presence,
        &mut escapes,
    );

    // Open the keyboard tap. The mouse filter is NEVER set in M3 — we must not
    // touch mouse.sys behavior at all.
    // SAFETY: valid context, canonical predicate.
    unsafe {
        (api.set_filter)(ctx.handle, Some(api.is_keyboard), FILTER_KEY_ALL);
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
                    ctl_passthrough = false;
                    watchdog_passthrough = false;
                    // Re-arm a tripped watchdog: the supervisor re-enabling
                    // capture is a recovery decision, but if the consumer is
                    // still stalled the protection must be able to fire again
                    // (otherwise captured keyboards black-hole with no guard).
                    // The health `watchdog_tripped` flag stays latched.
                    if wd.tripped() {
                        wd = Watchdog::default();
                    }
                    // NOTE: an escape latch is deliberately NOT cleared here. A
                    // supervisor mirroring its own stale state must never be
                    // able to re-capture keyboards a `LeftCtrl ×5` just freed;
                    // only a second gesture does that.
                    publish(&decision, &slots, &captured_ids, escapes.passthrough());
                }
                Ok(CaptureCtl::SetPassthrough) => {
                    ctl_passthrough = true;
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
        let dev = unsafe { (api.wait_with_timeout)(ctx.handle, WAIT_TIMEOUT_MS) };
        if dev == 0 {
            // Driver idle: the one moment where a full slot re-read costs
            // nothing. Removals produce no stroke, so this is how the
            // supervisor ever learns a bound board was unplugged.
            let now_ms = start.elapsed().as_millis() as u64;
            if now_ms.saturating_sub(last_rescan_ms) >= PRESENCE_RESCAN_MS {
                last_rescan_ms = now_ms;
                rescan(
                    ctx,
                    &mut slots,
                    &mut exhaustion,
                    health,
                    presence,
                    &mut escapes,
                );
                let passthrough = ctl_passthrough || watchdog_passthrough || escapes.passthrough();
                publish(&decision, &slots, &captured_ids, passthrough);
            }
            continue;
        }
        // SAFETY: predicate accepts any value.
        if unsafe { (api.is_invalid)(dev) } != 0 {
            continue;
        }

        // SAFETY: predicate accepts any value.
        if unsafe { (api.is_mouse)(dev) } != 0 {
            // Mouse filter is NONE, so this should be unreachable; if the
            // driver hands us mouse strokes anyway, re-send them verbatim so
            // nothing is ever lost. Never reported in M3.
            // SAFETY: buffer of RECEIVE_BATCH raw mouse strokes; the driver
            // treats the pointer as a packed InterceptionMouseStroke array for
            // mouse devices (same layout contract the safe wrapper relies on).
            let n = unsafe {
                (api.receive)(
                    ctx.handle,
                    dev,
                    mouse_buf.as_mut_ptr().cast(),
                    RECEIVE_BATCH as u32,
                )
            };
            if n > 0 {
                // SAFETY: sending back the exact strokes just received.
                unsafe { (api.send)(ctx.handle, dev, mouse_buf.as_ptr().cast(), n as u32) };
            }
            continue;
        }

        // Keyboard stroke(s).
        // SAFETY: same layout contract as above, for InterceptionKeyStroke.
        let n = unsafe {
            (api.receive)(
                ctx.handle,
                dev,
                kb_buf.as_mut_ptr().cast(),
                RECEIVE_BATCH as u32,
            )
        };
        if n <= 0 {
            continue;
        }

        let slot = dev as usize; // 1..=10 after the is_invalid/is_mouse checks
        if slots[slot].is_none() {
            // Cold path: a device appeared on a slot we hadn't seen (hotplug,
            // or an id climbing after replug — the exhaustion signal).
            if let Some(hwid) = hardware_id(ctx, dev) {
                // A single device arriving outside a rescan: never compared
                // against its own siblings, so two identical boards can no
                // longer read as one board climbing (see `exhaustion`).
                note_exhaustion(exhaustion.observe_hotplug(dev, &hwid), health);
                slots[slot] = Some(SlotEntry {
                    id: DeviceId::from(hwid),
                });
                let passthrough = ctl_passthrough || watchdog_passthrough || escapes.passthrough();
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

            let event = key_event(device, stroke.code, stroke.state, qpc_now());

            // Emergency escapes are evaluated HERE: before the pass/suppress
            // decision, on this thread, with no channel between the gesture and
            // the release. Nothing downstream — a wedged engine, a wedged ViGEm
            // IOCTL, a full event channel — can starve it (risk review §3 item 2,
            // legacy `InputManager.CheckForEmergencyHit`).
            let action = escapes.observe(&event);
            if action.changed_passthrough() {
                // No logging on this thread, by rule: a console write can block
                // indefinitely (QuickEdit selection, a pipe whose reader died),
                // and stalling here means the filter stays armed with nothing
                // pumping it — every keyboard dead. The supervisor polls
                // `EscapeHandle` and does the announcing.
                publish(
                    &decision,
                    &slots,
                    &captured_ids,
                    ctl_passthrough || watchdog_passthrough || escapes.passthrough(),
                );
                // The rest of this batch — and this very stroke — obey the new
                // state immediately.
                snap = decision.load();
            }

            if should_resend(snap.passthrough, is_captured) {
                // Re-send FIRST (latency to the OS), byte-for-byte: `stroke`
                // is untouched since receive — code, state and information all
                // preserved. A corrupted re-send breaks every keyboard.
                // SAFETY: one valid stroke, same layout contract as receive.
                unsafe {
                    (api.send)(
                        ctx.handle,
                        dev,
                        (stroke as *const raw::InterceptionKeyStroke).cast(),
                        1,
                    );
                }
            }

            match tx.try_send(event) {
                Ok(()) => wd.on_send_ok(),
                Err(TrySendError::Full(_)) => {
                    health.add_dropped(1);
                    let now_ms = start.elapsed().as_millis() as u64;
                    if wd.on_send_failed(now_ms) {
                        // Flag only — see the escape branch above for why this
                        // thread never writes to a console. `ksx run` surfaces
                        // `watchdog_tripped` from health.
                        health.set_watchdog_tripped();
                        watchdog_passthrough = true;
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

/// Re-read the whole slot table and republish presence (cold path).
///
/// Runs once before the filter opens (seeding identities + the exhaustion
/// baseline) and thereafter only on the idle path. Slots whose device is gone
/// are **cleared**, which is what makes removal observable: the published id
/// list is the supervisor's hotplug signal.
///
/// Clearing fails safe. A transient `hardware_id` failure drops the slot's
/// identity, so its next stroke is attributed to `<unknown-interception-device>`
/// and therefore *passes through* to the OS (never suppressed) — and the hot
/// loop's unknown-slot path immediately re-learns the id. The error direction is
/// "a keystroke reaches Windows", never "a keystroke disappears".
fn rescan(
    ctx: &Ctx,
    slots: &mut Slots,
    exhaustion: &mut ExhaustionDetector,
    health: &HealthHandle,
    presence: &PresenceHandle,
    escapes: &mut EscapeWatch,
) {
    let before: Vec<DeviceId> = slots.iter().flatten().map(|e| e.id.clone()).collect();
    let mut ids = Vec::with_capacity(MAX_DEVICE);
    // One pass: an id seen twice in THIS walk is two identical boards, not a
    // climb. Only the comparison against the previous pass can report one.
    exhaustion.begin_pass();
    for slot in 1..=MAX_DEVICE as i32 {
        // SAFETY: predicates accept any device number.
        let gone = unsafe { (ctx.api.is_invalid)(slot) } != 0;
        let hwid = if gone { None } else { hardware_id(ctx, slot) };
        match hwid {
            Some(hwid) => {
                // SAFETY: as above.
                if unsafe { (ctx.api.is_keyboard)(slot) } != 0 {
                    note_exhaustion(exhaustion.observe_keyboard(slot, &hwid), health);
                }
                let id = DeviceId::from(hwid);
                ids.push(id.clone());
                slots[slot as usize] = Some(SlotEntry { id });
            }
            None => slots[slot as usize] = None,
        }
    }
    note_exhaustion(exhaustion.end_pass(), health);
    ids.sort_unstable();
    ids.dedup();
    // A device that left must not keep a Ctrl+Alt+Del combo half-armed.
    for old in before {
        if !ids.contains(&old) {
            escapes.forget_device(&old);
        }
    }
    presence.publish(ids);
}

/// Loud by design (risk review R2 mitigation 4) — but the shouting happens in
/// the supervisor, not here: this runs on the capture thread, where a blocking
/// console write would stop the pump with the filter armed. `ksx run` and
/// `ksx devices` both render `reboot_required` from health.
fn note_exhaustion(event: Option<Exhaustion>, health: &HealthHandle) {
    if event.is_some() {
        health.set_reboot_required();
    }
}

/// Hardware id for a device slot, like legacy `GetHardwareID`: reject 0-length
/// and buffer-overflow results, take the first NUL-terminated wide string.
fn hardware_id(ctx: &Ctx, dev: i32) -> Option<String> {
    let mut buf = [0u16; HWID_BUF_CHARS];
    // SAFETY: buffer is HWID_BUF_CHARS u16s = 2x bytes, exactly what we pass.
    let bytes = unsafe {
        (ctx.api.get_hardware_id)(
            ctx.handle,
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
