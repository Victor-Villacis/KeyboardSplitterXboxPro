//! `WinUsbBackend` — the capture path that outlives the 2026 signing cliff.
//!
//! One instance owns **one claimed USB interface** (the I-PAC's `MI_00`).
//! Several boards means several instances, composed by
//! [`crate::CompositeBackend`]; that keeps this type honest — a single device,
//! a single endpoint, a single blocking read — instead of a hand-rolled
//! multiplexer inside the hot loop.
//!
//! # Why this backend exists
//!
//! `oblitum/Interception`'s `keyboard.sys` is cross-signed with a certificate
//! that expired in 2012. This machine already runs Microsoft's
//! cross-signed-driver audit CI policy (`{784C4414-…}`, live 2026-08-03); when
//! it flips to enforcement the driver stops loading and every Interception
//! capture path dies at boot. Binding the interface to Microsoft's in-box
//! `winusb.sys` instead removes the third-party kernel driver from the picture
//! entirely (`docs/research/keyboard-capture-2026.md` §4, §7).
//!
//! # Blocking is structural — and that inverts everything
//!
//! Once the interface is bound to `winusb.sys`, it has left the `kbdclass`
//! stack. Windows does not see the board. Not "ksx suppresses its keystrokes" —
//! there is no keyboard there to suppress. That is the property this milestone
//! is bought with, and it is also the property that makes this backend behave
//! differently from every other one:
//!
//! | | `InterceptionBackend` | `WinUsbBackend` |
//! |---|---|---|
//! | default state of a device | typing into Windows | invisible to Windows |
//! | `SetCaptured([id])` | start suppressing `id` | stop **injecting** `id` |
//! | `SetPassthrough` | stop suppressing | start injecting everything |
//! | escape latch / watchdog | releases the filter | starts injecting |
//! | ksx crashes | keyboard works again in <1 s | keyboard stays dark |
//! | a bug leaks | a keystroke reaches Windows | a keystroke vanishes |
//!
//! So `SetCaptured`/`SetPassthrough` keep their *meaning* — "is this device's
//! input allowed to reach Windows" — and change their *mechanism*: passthrough
//! is synthesis via [`ksx_platform::inject::KeyInjector`], not a re-send. The shared
//! [`crate::decision::should_resend`] predicate still decides, unchanged, so
//! there is exactly one place in the codebase that answers the question.
//!
//! The last row of that table is the one to internalise: **crash-only recovery
//! does not apply to the blocking**. Killing ksx frees an Interception-captured
//! keyboard; it does not un-bind a WinUSB interface. What crash-only still buys
//! is that nothing is left half-pressed — the drop guard releases every key it
//! injected. Restoring the board to a keyboard is `pnputil` (`docs/RECOVERY.md`
//! §2), and the reason `docs/RECOVERY.md` insists on one never-claimed spare
//! keyboard on a different port.
//!
//! # Identity
//!
//! [`crate::backend::DeviceInfo::id`] is the interface's **device instance
//! path** — `USB\VID_D209&PID_0430&MI_00\7&1A2B3C4D&0&0000` — not a hardware
//! id. Two identical encoders on different ports get different paths, which is
//! the structural fix for T4 (`docs/USE-CASES.md`). Migration from an
//! Interception-hwid config is documented in `docs/MIGRATION-WINUSB.md`; `ksx devices`
//! prints the replacement line to paste.
//!
//! # Hot path
//!
//! Per report: one `wait_next_complete`, a decode into a stack [`crate::hid::UsageSet`],
//! an XOR diff, and per changed key an escape check, a `should_resend`, maybe
//! one `SendInput`, and one `try_send`. The transfer buffer is allocated once
//! and resubmitted forever. No locks, no logging, no allocation after startup
//! beyond the per-event `DeviceId` clone the `KeyEvent` contract requires.

pub mod enumerate;

use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError};
use ksx_core::KeyEvent;
use nusb::transfer::{Buffer, In, Interrupt, TransferError};
use nusb::{Endpoint, Interface, MaybeFuture as _};

use crate::backend::{
    CaptureBackend, CaptureCtl, CaptureError, DeviceInfo, DeviceKind, ExitReason, Handles,
};
use crate::decision::{key_event, should_resend};
use crate::escape::{EscapeHandle, EscapeWatch};
use crate::health::HealthHandle;
use crate::hid::{self, KeyboardFormat, ReportReader, UsageSet};
use crate::keymap::corrected_key;
use crate::presence::PresenceHandle;
use crate::watchdog::Watchdog;
use ksx_platform::inject::KeyInjector;

pub use enumerate::{candidates, Binding, UsbCandidate};

/// Endpoint wait timeout: bounds ctl-message and shutdown latency, exactly like
/// the Interception backend's `interception_wait_with_timeout`.
const WAIT_TIMEOUT_MS: u64 = 50;

/// `GET_DESCRIPTOR` timeout at claim time (cold path, once).
const DESCRIPTOR_TIMEOUT_MS: u64 = 500;

/// May this device's input reach Windows right now?
///
/// Three independent sources, OR-ed, kept apart so none can silently clear
/// another: what the supervisor asked for, what the stall watchdog forced, and
/// what an emergency escape latched.
///
/// The escape latch is read **live**, not from a cached snapshot, and that is
/// load-bearing rather than incidental. It lives in the shared
/// [`EscapeHandle`], so with several backends in one session (two claimed
/// boards plus an Interception context) the gesture that frees them all is
/// performed on *one* thread and observed by the others. A thread that trusted
/// its own last-published snapshot would keep suppressing until the supervisor
/// happened to send it a control message — i.e. `LeftCtrl x5` would free the
/// panel you pressed it on and leave the other one dead, which is precisely the
/// failure the escape hatch exists to prevent. The cost is one relaxed atomic
/// load per key transition.
///
/// The Interception backend needs an `arc-swap` snapshot because it publishes a
/// whole *slot table* that must not change mid-batch. This backend owns exactly
/// one device on exactly one thread, so there is nothing to publish and no
/// snapshot to go stale.
fn may_reach_windows(
    ctl_passthrough: bool,
    watchdog_passthrough: bool,
    escapes: &EscapeWatch,
) -> bool {
    ctl_passthrough || watchdog_passthrough || escapes.passthrough()
}

/// A claimed WinUSB keyboard interface.
///
/// **Constructing this claims the interface.** There is no way to build one
/// without taking exclusive ownership of the device, which is why enumeration
/// lives in [`enumerate`] as free functions: `ksx devices` can never
/// accidentally reach this type.
pub struct WinUsbBackend {
    info: DeviceInfo,
    interface: Interface,
    endpoint_address: u8,
    format: KeyboardFormat,
    injector: Box<dyn KeyInjector>,
    health: HealthHandle,
    presence: PresenceHandle,
    escapes: EscapeHandle,
}

impl WinUsbBackend {
    /// Claim `candidate`'s interface and prepare to read its reports.
    ///
    /// Refuses unless the interface is already bound to `winusb.sys`: ksx never
    /// performs the rebind (safety rule — a wrong rebind takes the arcade panel
    /// offline until someone can reach a working keyboard).
    ///
    /// The report descriptor is fetched and parsed here, on the cold path, so a
    /// board whose layout we cannot read is rejected *before* anything depends
    /// on it rather than producing garbage keys later.
    pub fn claim(
        candidate: &UsbCandidate,
        injector: Box<dyn KeyInjector>,
    ) -> Result<Self, CaptureError> {
        Self::claim_with(candidate, injector, Handles::new())
    }

    /// [`Self::claim`] publishing into pre-existing handles — how
    /// [`crate::CompositeBackend`] gives every backend in a session one shared
    /// health state and one shared escape latch.
    pub fn claim_with(
        candidate: &UsbCandidate,
        injector: Box<dyn KeyInjector>,
        handles: Handles,
    ) -> Result<Self, CaptureError> {
        if !candidate.binding.is_winusb() {
            return Err(CaptureError::NotRebound {
                id: candidate.id.as_str().to_owned(),
                bound: candidate.binding.label().to_owned(),
            });
        }

        let device_info = nusb::list_devices()
            .wait()
            .map_err(|source| CaptureError::Usb {
                context: "enumerating USB devices",
                source: source.into(),
            })?
            .find(|d| {
                d.instance_id().to_string_lossy().to_uppercase() == candidate.parent_id
                    && d.vendor_id() == candidate.vendor_id
                    && d.product_id() == candidate.product_id
            })
            .ok_or_else(|| CaptureError::NoSuchDevice {
                id: candidate.id.as_str().to_owned(),
            })?;

        let device = device_info
            .open()
            .wait()
            .map_err(|source| CaptureError::Usb {
                context: "opening the USB device",
                source: source.into(),
            })?;
        let interface = device
            .claim_interface(candidate.interface_number)
            .wait()
            .map_err(|source| CaptureError::Usb {
                context: "claiming the WinUSB interface",
                source: source.into(),
            })?;

        // The report descriptor, from the device, at runtime — not a hardcoded
        // boot-report assumption. See `crate::hid::descriptor`.
        let raw = interface
            .control_in(
                nusb::transfer::ControlIn {
                    control_type: nusb::transfer::ControlType::Standard,
                    recipient: nusb::transfer::Recipient::Interface,
                    request: 0x06, // GET_DESCRIPTOR
                    value: u16::from(hid::DESCRIPTOR_TYPE_REPORT) << 8,
                    index: u16::from(candidate.interface_number),
                    length: 4096,
                },
                Duration::from_millis(DESCRIPTOR_TIMEOUT_MS),
            )
            .wait()
            .map_err(|err| CaptureError::Usb {
                context: "reading the HID report descriptor",
                source: std::io::Error::other(err),
            })?;

        let format = hid::descriptor::parse(&raw);
        if format.is_empty() {
            return Err(CaptureError::NotAKeyboard {
                id: candidate.id.as_str().to_owned(),
            });
        }

        let endpoint_address =
            interrupt_in_address(&interface).ok_or_else(|| CaptureError::NoInterruptEndpoint {
                id: candidate.id.as_str().to_owned(),
            })?;

        let presence = PresenceHandle::new();
        presence.publish(vec![candidate.id.clone()]);

        tracing::info!(
            id = %candidate.id,
            reports = format.reports.len(),
            report_ids = format.uses_report_ids,
            "WinUSB interface claimed; this board is invisible to Windows until \
             the binding is removed (docs/RECOVERY.md §2)"
        );

        Ok(Self {
            info: DeviceInfo {
                id: candidate.id.clone(),
                // Interception slots do not exist here — that is the point.
                interception_slot: None,
                friendly: candidate.friendly().map(str::to_owned),
                kind: DeviceKind::Keyboard,
            },
            interface,
            endpoint_address,
            format,
            injector,
            health: handles.health,
            presence,
            escapes: handles.escapes,
        })
    }

    /// The parsed report layout, for diagnostics (`ksx devices`, tests).
    pub fn format(&self) -> &KeyboardFormat {
        &self.format
    }
}

/// First interrupt-IN endpoint of the interface's current alt setting.
fn interrupt_in_address(interface: &Interface) -> Option<u8> {
    use nusb::descriptors::TransferType;
    use nusb::transfer::Direction;

    interface
        .descriptor()?
        .endpoints()
        .find(|e| e.transfer_type() == TransferType::Interrupt && e.direction() == Direction::In)
        .map(|e| e.address())
}

impl CaptureBackend for WinUsbBackend {
    fn devices(&mut self) -> Vec<DeviceInfo> {
        vec![self.info.clone()]
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
        let WinUsbBackend {
            info,
            interface,
            endpoint_address,
            format,
            mut injector,
            health,
            presence,
            escapes,
        } = *self;

        std::thread::Builder::new()
            .name("ksx-capture-winusb".into())
            .spawn(move || {
                set_time_critical_priority();

                let endpoint = match interface.endpoint::<Interrupt, In>(endpoint_address) {
                    Ok(endpoint) => endpoint,
                    Err(err) => {
                        tracing::error!(
                            id = %info.id,
                            "opening the interrupt endpoint failed: {err}"
                        );
                        presence.publish(Vec::new());
                        return ExitReason::DeviceLost;
                    }
                };

                // Everything that must happen even on panic unwind lives in the
                // guard: release every key we injected, then let go of the
                // interface. A panicking thread that leaves LeftAlt injected-down
                // is a machine nobody can type on.
                let mut state = LoopState {
                    endpoint,
                    reader: ReportReader::new(format),
                    injected: UsageSet::new(),
                    injector: injector.as_mut(),
                    health: &health,
                };

                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    capture_loop(&info, &mut state, &presence, &escapes, &tx, &ctl)
                }));

                // Runs on every exit path, including unwind.
                state.release_injected();
                presence.publish(Vec::new());

                match result {
                    Ok(reason) => reason,
                    Err(_) => {
                        health.set_panicked();
                        ExitReason::Panicked
                    }
                }
            })
    }
}

/// The mutable pieces the loop and the cleanup both need.
struct LoopState<'a> {
    endpoint: Endpoint<Interrupt, In>,
    reader: ReportReader,
    /// Usages we have injected as down and not yet injected as up.
    injected: UsageSet,
    injector: &'a mut dyn KeyInjector,
    health: &'a HealthHandle,
}

impl LoopState<'_> {
    fn inject(&mut self, usage: u8, down: bool) {
        inject_one(&mut self.injected, self.injector, self.health, usage, down);
    }

    fn release_injected(&mut self) {
        release_injected(&mut self.injected, self.injector);
    }
}

/// The [`ksx_core::Key`] a usage injects as, or `None` when there is nothing to
/// type.
///
/// Note the deliberate difference from [`crate::decision::key_event`]: the
/// event reported to the engine obeys legacy's `is_down` rule (an E1 Pause
/// stroke never counts as a press), while injection uses the real transition.
/// Reporting legacy-compatibly keeps presets and the replay corpus valid;
/// injecting a release with no matching press would mean Windows never saw the
/// key at all. Pause is the only key where the two differ, and no arcade panel
/// emits it.
fn key_for_usage(usage: u8) -> Option<ksx_core::Key> {
    let (code, state) = hid::usage_to_stroke(usage, true)?;
    let key = corrected_key(code, state);
    (key != ksx_core::Key::Unknown && key != ksx_core::Key::None).then_some(key)
}

/// Inject one transition, tracking what is left held and counting refusals.
///
/// Failures are counted, never logged: this runs on the capture thread, where a
/// blocking console write is the exact hazard the Interception backend
/// documents. `ksx run` reports `inject_failures` from health.
///
/// A free function, not just a method, because the two things it guarantees --
/// "held state matches what the OS thinks" and "a refusal is counted, never
/// swallowed" -- are exactly what the tests must exercise, and they must not
/// need a USB endpoint to do it.
fn inject_one(
    injected: &mut UsageSet,
    injector: &mut dyn KeyInjector,
    health: &HealthHandle,
    usage: u8,
    down: bool,
) {
    let Some(key) = key_for_usage(usage) else {
        return; // no keyboard wire form; nothing synthesizable
    };
    match injector.stroke(key, down) {
        Ok(()) => {
            if down {
                injected.insert(usage);
            } else {
                *injected = without(injected, usage);
            }
        }
        Err(_) => health.add_inject_failures(1),
    }
}

/// Let go of every key we injected. Cold path; runs on unwind too.
///
/// This is the crash-safety guarantee that still holds on this backend: killing
/// ksx does **not** hand the board back to Windows (the WinUSB binding does
/// that, and it outlives the process), but it must never leave a synthetic
/// LeftAlt held down on a machine that now has no way to release it.
fn release_injected(injected: &mut UsageSet, injector: &mut dyn KeyInjector) {
    for usage in injected.iter() {
        if let Some(key) = key_for_usage(usage) {
            let _ = injector.stroke(key, false);
        }
    }
    *injected = UsageSet::new();
}

/// `set` minus `usage`. (`UsageSet` is intentionally insert-only on the hot
/// path; removal happens once per key release.)
fn without(set: &UsageSet, usage: u8) -> UsageSet {
    let mut out = UsageSet::new();
    for u in set.iter() {
        if u != usage {
            out.insert(u);
        }
    }
    out
}

fn capture_loop(
    info: &DeviceInfo,
    state: &mut LoopState<'_>,
    presence: &PresenceHandle,
    escape_handle: &EscapeHandle,
    tx: &Sender<KeyEvent>,
    ctl: &Receiver<CaptureCtl>,
) -> ExitReason {
    let start = std::time::Instant::now();
    let mut wd = Watchdog::default();
    let mut escapes = EscapeWatch::new(escape_handle.clone());
    // Passthrough by contract until the supervisor says otherwise. On this
    // backend that means "inject everything", so a freshly started session
    // leaves the panel typing exactly as it was.
    let mut ctl_passthrough = true;
    let mut watchdog_passthrough = false;
    let mut captured = false;

    // One buffer, allocated once, resubmitted forever. `submit` requires a
    // requested length that is a nonzero multiple of the max packet size.
    let packet = state.endpoint.max_packet_size().max(1);
    let wanted = state.reader.format().max_report_len().max(1);
    let len = wanted.div_ceil(packet) * packet;
    state.endpoint.submit(Buffer::new(len));

    loop {
        loop {
            match ctl.try_recv() {
                Ok(CaptureCtl::SetCaptured(ids)) => {
                    // "Captured" here means "do NOT inject": the OS already
                    // cannot see this device, so binding it to a slot simply
                    // stops us handing its keystrokes on.
                    captured = ids.iter().any(|id| id == &info.id);
                    ctl_passthrough = false;
                    watchdog_passthrough = false;
                    if wd.tripped() {
                        wd = Watchdog::default();
                    }
                    // An escape latch is deliberately NOT cleared here — same
                    // rule as the Interception backend: a supervisor mirroring
                    // its own stale state must never re-capture boards a
                    // `LeftCtrl x5` just freed.
                }
                Ok(CaptureCtl::SetPassthrough) => ctl_passthrough = true,
                Ok(CaptureCtl::Shutdown) => return ExitReason::Shutdown,
                Err(TryRecvError::Empty) => break,
                // Controller gone: nobody can release this device again.
                Err(TryRecvError::Disconnected) => return ExitReason::Shutdown,
            }
        }

        let Some(completion) = state
            .endpoint
            .wait_next_complete(Duration::from_millis(WAIT_TIMEOUT_MS))
        else {
            continue; // idle; go round for ctl
        };

        let mut buffer = completion.buffer;
        match completion.status {
            Ok(()) => {
                let mut lost = false;

                // Borrow dance: the callback needs `state` for injection, so the
                // report is decoded against a scratch reader taken out of state.
                let mut reader = std::mem::replace(
                    &mut state.reader,
                    ReportReader::new(KeyboardFormat::default()),
                );
                reader.feed(&buffer, |usage, down| {
                    if lost {
                        return;
                    }
                    let Some((code, key_state)) = hid::usage_to_stroke(usage, down) else {
                        return; // usage with no set-1 form: silent by contract
                    };
                    let event = key_event(&info.id, code, key_state, qpc_now());

                    // Escapes FIRST, on this thread, upstream of every
                    // channel: the stroke that completes the gesture is already
                    // flowing to Windows again by the time it is decided.
                    escapes.observe(&event);

                    // The SAME predicate the Interception backend uses. Only
                    // the verb changes: there it means "re-send", here it means
                    // "synthesize", because there is nothing to re-send to.
                    if should_resend(
                        may_reach_windows(ctl_passthrough, watchdog_passthrough, &escapes),
                        captured,
                    ) {
                        state.inject(usage, down);
                    }

                    match tx.try_send(event) {
                        Ok(()) => wd.on_send_ok(),
                        Err(TrySendError::Full(_)) => {
                            state.health.add_dropped(1);
                            let now_ms = start.elapsed().as_millis() as u64;
                            if wd.on_send_failed(now_ms) {
                                // Flag only — this thread never writes to a
                                // console; `ksx run` surfaces it from health.
                                state.health.set_watchdog_tripped();
                                watchdog_passthrough = true;
                            }
                        }
                        Err(TrySendError::Disconnected(_)) => lost = true,
                    }
                });
                state.reader = reader;

                if lost {
                    return ExitReason::ChannelClosed;
                }
            }
            Err(TransferError::Cancelled) => {
                // We never cancel; treat as idle and resubmit.
            }
            Err(TransferError::Stall) => {
                // Endpoint halted. Clearing it is a control transfer, not IO on
                // the data path, and it is the documented recovery.
                if state.endpoint.clear_halt().wait().is_err() {
                    presence.publish(Vec::new());
                    return ExitReason::DeviceLost;
                }
            }
            Err(_) => {
                // Disconnected / unknown: the board is gone. Release what we
                // hold so no key is left down anywhere and end this backend.
                state.reader.release_all(|usage, down| {
                    if let Some((code, key_state)) = hid::usage_to_stroke(usage, down) {
                        let event = key_event(&info.id, code, key_state, qpc_now());
                        escapes.forget_device(&info.id);
                        let _ = tx.try_send(event);
                    }
                });
                presence.publish(Vec::new());
                return ExitReason::DeviceLost;
            }
        }

        buffer.clear();
        buffer.set_requested_len(len);
        state.endpoint.submit(buffer);
    }
}

/// QPC ticks — the `KeyEvent.t` unit on Windows, identical to the Interception
/// backend's so `ksx doctor --latency` compares like with like.
fn qpc_now() -> u64 {
    let mut t: i64 = 0;
    // SAFETY: out-pointer to a stack i64; QPC cannot fail on XP+.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut t) };
    t as u64
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hid::fixtures;
    use ksx_core::Key;
    use ksx_platform::inject::RecordingInjector;

    /// The whole point of the module, expressed without any USB: what does a
    /// report *do* under each mode? This drives exactly the code path the hot
    /// loop drives — [`ReportReader::feed`] → [`should_resend`] → inject —
    /// with the endpoint replaced by a caller-supplied byte slice.
    /// Usage transitions reported to the engine.
    type Reported = Vec<(u8, bool)>;
    /// Key transitions handed to the OS.
    type Injected = Vec<(Key, bool)>;

    fn simulate(
        reports: &[&[u8]],
        passthrough: bool,
        captured: bool,
        injector: &mut dyn KeyInjector,
    ) -> (Reported, Injected) {
        let mut reader = ReportReader::new(crate::hid::descriptor::parse(
            fixtures::BOOT_KEYBOARD_DESCRIPTOR,
        ));
        let mut events = Vec::new();
        let mut injected = Vec::new();
        for report in reports {
            reader.feed(report, |usage, down| {
                events.push((usage, down));
                if should_resend(passthrough, captured) {
                    if let Some(key) = key_for_usage(usage) {
                        if injector.stroke(key, down).is_ok() {
                            injected.push((key, down));
                        }
                    }
                }
            });
        }
        (events, injected)
    }

    #[test]
    fn captured_means_reported_but_never_injected() {
        let mut inj = RecordingInjector::new();
        let (events, injected) = simulate(
            &[
                &fixtures::boot_report(0, &[0x04]),
                &fixtures::boot_report(0, &[]),
            ],
            false,
            true,
            &mut inj,
        );
        assert_eq!(events, vec![(0x04, true), (0x04, false)]);
        assert!(
            injected.is_empty(),
            "a captured board's keys must not reach Windows"
        );
    }

    #[test]
    fn passthrough_injects_every_transition_in_the_shared_key_vocabulary() {
        let mut inj = RecordingInjector::new();
        let (_, injected) = simulate(
            &[
                &fixtures::boot_report(0b0000_0001, &[0x50]), // LeftCtrl + Left arrow
                &fixtures::boot_report(0, &[]),
            ],
            true,
            true, // captured, but passthrough wins — the escape-hatch case
            &mut inj,
        );
        let mut sorted = injected.clone();
        sorted.sort_by_key(|(k, down)| (k.value(), *down));
        assert_eq!(
            sorted,
            vec![
                (Key::LeftControl, false),
                (Key::LeftControl, true),
                (Key::Left, false),
                (Key::Left, true),
            ],
            "the arrow must be the arrow, not Numpad4 — the E0 correction is \
             the same table the engine sees"
        );
    }

    #[test]
    fn an_unassigned_board_is_injected_because_nothing_else_will() {
        // The inversion in one test: on Interception an unbound device types
        // into Windows for free. Here, not injecting it means it types nowhere.
        let mut inj = RecordingInjector::new();
        let (_, injected) = simulate(
            &[&fixtures::boot_report(0, &[0x04])],
            false, // not in passthrough mode
            false, // ...and this device is not bound to a slot
            &mut inj,
        );
        assert_eq!(injected, vec![(Key::A, true)]);
    }

    #[test]
    fn a_refusing_injector_is_silent_not_fatal() {
        let mut inj = RecordingInjector::new();
        inj.fail_next(ksx_platform::inject::InjectError::Blocked {
            sent: 0,
            expected: 1,
        });
        let (events, injected) =
            simulate(&[&fixtures::boot_report(0, &[0x04])], true, false, &mut inj);
        assert_eq!(events, vec![(0x04, true)], "still reported to the engine");
        assert!(
            injected.is_empty(),
            "UIPI or the secure desktop swallowed it; the session carries on"
        );
    }

    #[test]
    fn without_removes_exactly_one_usage() {
        let mut set = UsageSet::new();
        set.insert(0x04);
        set.insert(0xE0);
        set.insert(0xFF);
        let out = without(&set, 0xE0);
        assert!(out.contains(0x04));
        assert!(!out.contains(0xE0));
        assert!(out.contains(0xFF));
        assert_eq!(out.len(), 2);
        // Removing something absent is a no-op.
        assert_eq!(without(&out, 0x77), out);
    }

    /// The crash-safety promise that still holds on this backend: whatever was
    /// injected down gets injected up, on every exit path including a panic.
    #[test]
    fn release_injected_lets_go_of_exactly_what_it_pressed() {
        let mut injector = RecordingInjector::new();
        let log = injector.log();
        let health = HealthHandle::new();
        let mut injected = UsageSet::new();

        for usage in [0xE0u8, 0x04, 0x50] {
            inject_one(&mut injected, &mut injector, &health, usage, true);
        }
        // One of them is released normally before the crash.
        inject_one(&mut injected, &mut injector, &health, 0x04, false);
        assert_eq!(injected.len(), 2);

        release_injected(&mut injected, &mut injector);
        assert!(injected.is_empty());

        let events = log.events();
        let downs = events.iter().filter(|(_, down)| *down).count();
        let ups = events.iter().filter(|(_, down)| !*down).count();
        assert_eq!(downs, 3);
        assert_eq!(ups, 3, "nothing is left held: {}", log.trace());
        assert_eq!(health.snapshot().inject_failures, 0);

        // Idempotent -- a second cleanup pass has nothing to release.
        release_injected(&mut injected, &mut injector);
        assert_eq!(log.events().len(), 6);
    }

    /// A refused injection must not be recorded as held, or the cleanup would
    /// send a key-up for a key that was never pressed.
    #[test]
    fn a_refused_injection_is_counted_and_not_tracked_as_held() {
        let mut injector = RecordingInjector::new();
        injector.fail_next(ksx_platform::inject::InjectError::Blocked {
            sent: 0,
            expected: 1,
        });
        let health = HealthHandle::new();
        let mut injected = UsageSet::new();
        inject_one(&mut injected, &mut injector, &health, 0xE0, true);
        assert!(injected.is_empty());
        assert_eq!(health.snapshot().inject_failures, 1);
    }

    /// A usage with no keyboard wire form must not count as an injection
    /// failure -- there was nothing to inject, and inflating the counter would
    /// make the one diagnostic that means "passthrough is broken" meaningless.
    #[test]
    fn an_unsynthesizable_usage_is_not_a_failure() {
        let mut injector = RecordingInjector::new();
        let health = HealthHandle::new();
        let mut injected = UsageSet::new();
        inject_one(&mut injected, &mut injector, &health, 0x68, true); // F13
        assert!(injected.is_empty());
        assert_eq!(health.snapshot().inject_failures, 0);
        assert!(injector.log().events().is_empty());
    }

    /// The bug this replaced a cached snapshot to fix.
    ///
    /// With several backends in one session, the escape gesture happens on one
    /// thread and has to be obeyed by all of them. Board B never sees the
    /// gesture and never receives a control message — it just has to notice
    /// that the shared latch flipped, on its very next key.
    #[test]
    fn an_escape_latched_by_another_thread_is_obeyed_immediately() {
        let shared = EscapeHandle::new();
        let board_b = EscapeWatch::new(shared.clone());

        assert!(
            !may_reach_windows(false, false, &board_b),
            "captured and nothing latched: board B suppresses"
        );

        // Board A's capture thread completes `LeftCtrl x5`, thousands of
        // instructions away, with no channel between them.
        let mut board_a = EscapeWatch::new(shared.clone());
        for _ in 0..5 {
            for down in [true, false] {
                board_a.observe(&ksx_core::KeyEvent {
                    device: ksx_core::DeviceId::from("board-a"),
                    key: Key::LeftControl,
                    down,
                    t: 0,
                });
            }
        }

        assert!(
            may_reach_windows(false, false, &board_b),
            "board B must free itself on the next key, with no ctl message"
        );
    }

    #[test]
    fn the_three_passthrough_sources_are_independent() {
        let handle = EscapeHandle::new();
        let escapes = EscapeWatch::new(handle.clone());
        assert!(!may_reach_windows(false, false, &escapes));
        assert!(may_reach_windows(true, false, &escapes), "supervisor asked");
        assert!(may_reach_windows(false, true, &escapes), "watchdog tripped");
        // ...and none of them can clear another: the escape latch stays set
        // while the supervisor believes it is capturing.
        let mut watch = EscapeWatch::new(handle.clone());
        for _ in 0..5 {
            for down in [true, false] {
                watch.observe(&ksx_core::KeyEvent {
                    device: ksx_core::DeviceId::from("d"),
                    key: Key::LeftControl,
                    down,
                    t: 0,
                });
            }
        }
        assert!(may_reach_windows(false, false, &escapes));
    }

    /// Injection and reporting must agree on which key a usage is, or a preset
    /// bound to `Left` would pad-move while Windows received `Numpad4`.
    #[test]
    fn injection_and_reporting_resolve_a_usage_to_the_same_key() {
        for usage in [0x04u8, 0x50, 0x58, 0x54, 0xE4, 0xE7, 0x46, 0x65] {
            let (code, state) = hid::usage_to_stroke(usage, true).unwrap();
            let reported =
                crate::decision::key_event(&ksx_core::DeviceId::from("d"), code, state, 0);
            assert_eq!(
                key_for_usage(usage),
                Some(reported.key),
                "usage {usage:#04x} injects as a different key than it reports"
            );
        }
    }
}
