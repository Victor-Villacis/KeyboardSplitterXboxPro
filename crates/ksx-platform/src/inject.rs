//! Keystroke *injection* — the other half of a WinUSB claim.
//!
//! # Why this exists
//!
//! Once the I-PAC's `MI_00` interface is bound to `winusb.sys` it is no longer a
//! keyboard. That is the whole point (blocking becomes structural — see
//! `docs/research/keyboard-capture-2026.md` §4), but it has a consequence the
//! Interception backend never had: **the panel stops typing**. LaunchBox and
//! RetroBat navigate their menus with keystrokes, so a claimed panel with no
//! emulation running is a dead panel.
//!
//! The answer is symmetric to the claim: ksx owns the device, so ksx is
//! responsible for putting the keystrokes back whenever it is *not* translating
//! them into pad state. [`Typethrough`] is that policy, [`KeyInjector`] is the
//! mechanism, and `SendInput` is the syscall.
//!
//! # Scancodes, not virtual keys
//!
//! Every stroke is sent with `KEYEVENTF_SCANCODE` and `wVk = 0`, plus
//! `KEYEVENTF_EXTENDEDKEY` for the E0 keys. That is deliberate:
//! [`ksx_core::Key`] *is* a set-1 scancode vocabulary (it was ported from
//! Interception's, which reads make codes off the wire), so a scancode round
//! trip is the identity function. Sending virtual keys instead would push every
//! stroke through the foreground layout, and an arcade panel that types `Z` on a
//! QWERTY desktop and `W` on an AZERTY one is not a fixed panel. The one
//! exception is [`Key::Pause`], which has no single-scancode form at all — see
//! [`KeyStroke::VirtualKey`].
//!
//! # Honest limits of SendInput (read these before trusting it)
//!
//! `SendInput` injects at the *top* of the input stack, above every driver.
//! It is not a driver, and it cannot pretend to be one:
//!
//! - **Injected strokes are flagged.** `LLKHF_INJECTED` (and
//!   `LLKHF_LOWER_IL_INJECTED` when the sender is less privileged) is set on
//!   every one. Anti-cheat that rejects injected input will reject these; so
//!   will some games' raw-input paths, because `SendInput` never produces a
//!   `WM_INPUT` from a real HID device. Frontend menus (the actual use case)
//!   read ordinary `WM_KEYDOWN` and are fine.
//! - **The secure desktop is unreachable.** UAC consent, the lock screen,
//!   Ctrl+Alt+Del and the login screen run on a separate desktop that no
//!   user-mode injection can touch. On a machine whose only keyboard is a
//!   WinUSB-claimed panel you cannot type your PIN. Keep a real keyboard —
//!   `ksx winusb claim` refuses to take the last one for exactly this reason.
//! - **UIPI applies.** A non-elevated ksx cannot inject into an elevated
//!   window. `SendInput` returns a short count; [`InjectError::Blocked`] is that
//!   case, and it is a *report*, not a retry loop.
//! - **It is not the physical device.** Nothing downstream can tell which panel
//!   a re-injected key came from, because there is no device behind it any more.
//!   Per-device identity survives only inside ksx.
//!
//! Every injected event carries [`KSX_EXTRA_INFO`] in `dwExtraInfo` so ksx's own
//! hooks (and anyone debugging with Spy++) can recognise its own traffic. With a
//! WinUSB claim there is no feedback path to loop through — the claimed
//! interface cannot see injected input — but the tag costs nothing and the
//! Interception/RawInput backends share this module.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ksx_core::{Key, KeyEvent};

/// `dwExtraInfo` stamped on every event ksx injects: ASCII `ksx` + a version
/// byte. Lets ksx (or a debugger) tell its own synthetic strokes from real ones
/// without guessing at the `LLKHF_INJECTED` flag, which any process can set.
pub const KSX_EXTRA_INFO: usize = 0x0000_6B73_7801;

/// `VK_PAUSE`. The one key ksx injects as a virtual key — see [`KeyStroke`].
pub const VK_PAUSE: u16 = 0x13;

// ---------------------------------------------------------------------------
// Key -> wire form
// ---------------------------------------------------------------------------

/// How one [`Key`] is expressed to `SendInput`.
///
/// Almost everything is a [`KeyStroke::Scancode`]. [`Key::Pause`] is the
/// exception: on the wire it is the two-scancode sequence `E1 1D 45`, and a
/// single `KEYBDINPUT` has nowhere to put the `E1` prefix (`KEYEVENTF_*` has an
/// extended flag but no *second* extended flag). Injecting the bare `0x45` would
/// type NumLock — worse than not typing at all — so Pause goes through as a
/// virtual key and accepts the layout dependency that implies. Pause is not a
/// key an arcade panel emits, so this is a completeness case, not a hot path.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum KeyStroke {
    Scancode {
        /// Set-1 make code, without any prefix byte.
        code: u16,
        /// The stroke's real prefix is `E0`; `KEYEVENTF_EXTENDEDKEY` carries it.
        extended: bool,
    },
    VirtualKey {
        vk: u16,
    },
}

impl KeyStroke {
    /// The scancode form, if this is one. Convenience for tests and rendering.
    pub const fn scancode(self) -> Option<(u16, bool)> {
        match self {
            KeyStroke::Scancode { code, extended } => Some((code, extended)),
            KeyStroke::VirtualKey { .. } => None,
        }
    }
}

/// The wire form of `key`, or `None` if ksx cannot type it.
///
/// This is the exact inverse of `ksx_capture::keymap::corrected_key`: for every
/// key that function produces from an `(code, E0)` pair, this returns that same
/// pair. The two tables are deliberately written out independently — a shared
/// table would make a transcription error invisible in both directions — and
/// [`tests::every_extended_key_is_the_inverse_of_the_capture_table`] pins the
/// values that matter.
///
/// `None` means "not a keyboard stroke": [`Key::None`], [`Key::Unknown`], and
/// the mouse pseudo-keys ([`Key::is_mouse_pseudo`]). Mouse re-injection is out
/// of scope for M6 on purpose — the I-PAC's mouse/system/consumer collections
/// live on `MI_01`, which ksx does **not** claim, so they never stop working and
/// never need putting back.
pub fn stroke_for(key: Key) -> Option<KeyStroke> {
    use Key::*;

    let sc = |code: u16, extended: bool| Some(KeyStroke::Scancode { code, extended });

    match key {
        // --- not typeable ------------------------------------------------
        Key::None | Unknown => Option::None,
        k if k.is_mouse_pseudo() => Option::None,

        // --- the one virtual-key key -------------------------------------
        // E1 1D 45 cannot be expressed in one KEYBDINPUT.
        Pause => Some(KeyStroke::VirtualKey { vk: VK_PAUSE }),

        // --- E0-prefixed: GUI + right-hand modifiers ---------------------
        // NOTE: `Key::LeftWindows.value()` is 91 and the capture table also
        // accepts a *plain* 91, a legacy artefact. On the wire the GUI keys and
        // Menu only ever exist as E0 5B/5C/5D, so that is what ksx sends; the
        // capture table maps both forms back to the same key, so the round trip
        // is unaffected.
        LeftWindows => sc(0x5B, true),
        RightWindows => sc(0x5C, true),
        Menu => sc(0x5D, true),
        RightControl => sc(0x1D, true),
        RightAlt => sc(0x38, true),

        // --- E0-prefixed: navigation cluster (vs. the numpad twins) ------
        Up => sc(0x48, true),
        Down => sc(0x50, true),
        Left => sc(0x4B, true),
        Right => sc(0x4D, true),
        Home => sc(0x47, true),
        PageUp => sc(0x49, true),
        End => sc(0x4F, true),
        PageDown => sc(0x51, true),
        Insert => sc(0x52, true),
        Delete => sc(0x53, true),

        // --- E0-prefixed: numpad keys that are not numpad scancodes ------
        NumpadEnter => sc(0x1C, true),
        NumpadDivide => sc(0x35, true),

        // --- E0-prefixed: print screen / break / the fake shift ----------
        PrintScreen => sc(0x37, true),
        Break => sc(0x46, true),
        // The synthetic E0 2A the keyboard driver brackets PrintScreen and the
        // numpad-with-NumLock keys with. Injectable, and round-trips, but it is
        // a *prefix*, not a key anyone binds.
        ShiftModifier => sc(0x2A, true),

        // --- E0-prefixed: media ------------------------------------------
        MediaPreviousTrack => sc(0x10, true),
        MediaNextTrack => sc(0x19, true),
        MediaPlayPause => sc(0x22, true),
        VolumeUp => sc(0x30, true),
        VolumeDown => sc(0x2E, true),
        VolumeMute => sc(0x20, true),

        // --- E0-prefixed: the legacy OEM grab-bag ------------------------
        Oem0 => sc(0x01, true),
        Oem2 => sc(0x12, true),
        Oem3 => sc(0x17, true),
        Oem4 => sc(0x6E, true),
        Oem5 => sc(0x5F, true),
        Oem6 => sc(0x0A, true),
        Oem7 => sc(0x32, true),
        Oem13 => sc(0x78, true),

        // --- everything else: the enum value IS the set-1 make code ------
        other => sc(other.value(), false),
    }
}

// ---------------------------------------------------------------------------
// The injector
// ---------------------------------------------------------------------------

/// Why an injection did not happen.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InjectError {
    #[error("{0} has no keyboard wire form (mouse pseudo-key, None, or Unknown)")]
    Unsupported(Key),
    /// `SendInput` accepted fewer events than it was given. The documented
    /// causes are UIPI (a more-privileged window has the focus) and the secure
    /// desktop. Not retryable: the next stroke will be blocked for the same
    /// reason, and a retry loop would spin at keyboard rate.
    #[error("SendInput was blocked (sent {sent} of {expected}) — UIPI or the secure desktop has the input focus")]
    Blocked { sent: u32, expected: u32 },
    #[error("SendInput failed (win32 error {0})")]
    Os(u32),
    #[error("keystroke injection is Windows-only")]
    UnsupportedPlatform,
}

/// Synthesises keystrokes.
///
/// Behind a trait so the [`Typethrough`] policy — which is where the
/// stuck-key-on-transition bug would live — is exercised in CI against
/// [`RecordingInjector`], with no Win32 involved and no keystrokes escaping into
/// the developer's session.
pub trait KeyInjector: Send {
    /// Inject one transition. `down = false` is a release.
    fn stroke(&mut self, key: Key, down: bool) -> Result<(), InjectError>;
}

/// The real thing: one `SendInput` call per transition.
///
/// One call per stroke rather than a batch is deliberate. Batching would only
/// pay off for bursts, this is driven by human fingers, and a partial batch
/// (`SendInput` returning a short count mid-array) would leave ksx unsure which
/// halves of which press/release pairs landed — which is exactly how keys get
/// stuck down.
#[derive(Debug, Default)]
pub struct SendInputInjector {
    _private: (),
}

impl SendInputInjector {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl KeyInjector for SendInputInjector {
    fn stroke(&mut self, key: Key, down: bool) -> Result<(), InjectError> {
        let stroke = stroke_for(key).ok_or(InjectError::Unsupported(key))?;
        send_one(stroke, down)
    }
}

#[cfg(windows)]
pub use win::{input_for, send_one};

#[cfg(windows)]
mod win {
    use super::{InjectError, KeyStroke, KSX_EXTRA_INFO};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
        KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
    };

    /// Build the `INPUT` for one transition. Pure — no syscall — so the mapping
    /// is unit-tested field by field without a single key reaching the desktop.
    pub fn input_for(stroke: KeyStroke, down: bool) -> INPUT {
        let (vk, scan, mut flags) = match stroke {
            KeyStroke::Scancode { code, extended } => {
                let mut f = KEYEVENTF_SCANCODE;
                if extended {
                    f |= KEYEVENTF_EXTENDEDKEY;
                }
                // wVk MUST be 0 in scancode mode: a non-zero virtual key makes
                // the system translate the vk and ignore wScan, which is the
                // layout dependency this whole module exists to avoid.
                (0u16, code, f)
            }
            KeyStroke::VirtualKey { vk } => (vk, 0u16, 0u32),
        };
        if !down {
            flags |= KEYEVENTF_KEYUP;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: scan,
                    dwFlags: flags,
                    // 0 = "stamp it with the current tick". A fabricated
                    // timestamp is how you get input re-ordered relative to a
                    // real keyboard.
                    time: 0,
                    dwExtraInfo: KSX_EXTRA_INFO,
                },
            },
        }
    }

    pub fn send_one(stroke: KeyStroke, down: bool) -> Result<(), InjectError> {
        let input = input_for(stroke, down);
        // SAFETY: one correctly-sized INPUT owned by this frame; SendInput only
        // reads it and returns before the borrow ends.
        let sent = unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
        if sent == 1 {
            return Ok(());
        }
        // GetLastError is only meaningful when nothing was sent; a short count
        // on a one-element array is the blocked case either way.
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        if code == 0 {
            Err(InjectError::Blocked { sent, expected: 1 })
        } else {
            Err(InjectError::Os(code))
        }
    }
}

#[cfg(not(windows))]
pub fn send_one(_stroke: KeyStroke, _down: bool) -> Result<(), InjectError> {
    Err(InjectError::UnsupportedPlatform)
}

/// The transitions a [`RecordingInjector`] accepted, observable after the
/// injector has been moved into (and dropped with) a [`Typethrough`].
///
/// That is the point of the shared handle: the release-on-drop guarantee can
/// only be asserted from *outside* the value being dropped.
#[derive(Clone, Debug, Default)]
pub struct InjectionLog(Arc<Mutex<Vec<(Key, bool)>>>);

impl InjectionLog {
    pub fn events(&self) -> Vec<(Key, bool)> {
        self.0.lock().map(|log| log.clone()).unwrap_or_default()
    }

    /// Rendered as `A+ A- Up+` for readable assertions.
    pub fn trace(&self) -> String {
        self.events()
            .iter()
            .map(|(k, down)| format!("{}{}", k.name(), if *down { "+" } else { "-" }))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn clear(&self) {
        if let Ok(mut log) = self.0.lock() {
            log.clear();
        }
    }
}

/// A [`KeyInjector`] that records instead of typing.
///
/// Used by every [`Typethrough`] test — and available to the daemon for a
/// "show me what would be typed" mode that touches no desktop.
#[derive(Debug, Default)]
pub struct RecordingInjector {
    log: InjectionLog,
    /// Scripted failure, consumed by the next stroke.
    fail_next: Option<InjectError>,
}

impl RecordingInjector {
    pub fn new() -> Self {
        Self::default()
    }

    /// A handle to what this injector records. Clone it before handing the
    /// injector away.
    pub fn log(&self) -> InjectionLog {
        self.log.clone()
    }

    /// Make the next `stroke()` fail with `err` (then behave normally again).
    pub fn fail_next(&mut self, err: InjectError) {
        self.fail_next = Some(err);
    }
}

impl KeyInjector for RecordingInjector {
    fn stroke(&mut self, key: Key, down: bool) -> Result<(), InjectError> {
        if let Some(err) = self.fail_next.take() {
            return Err(err);
        }
        if stroke_for(key).is_none() {
            return Err(InjectError::Unsupported(key));
        }
        if let Ok(mut log) = self.log.0.lock() {
            log.push((key, down));
        }
        Ok(())
    }
}

/// A [`KeyInjector`] that accepts every stroke and does nothing with it.
///
/// This is not a test double. It is what the **daemon's** claimed panel is
/// built with (`ksx_app::daemon::panel`): the daemon holds one claim for its
/// whole lifetime and one [`Typethrough`] behind it, and that `Typethrough` is
/// the single injector for the panel. Handing the capture backend a real
/// injector as well would give the panel two independent held-key sets, and a
/// mode switch between them is exactly how a key gets stranded down on the
/// desktop — the bug [`Typethrough`] exists to prevent.
///
/// `ksx run` (no daemon, one session, no typethrough) still hands the backend a
/// [`SendInputInjector`]: there, the backend *is* the only injector.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullInjector;

impl KeyInjector for NullInjector {
    fn stroke(&mut self, _key: Key, _down: bool) -> Result<(), InjectError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Typethrough — the policy that makes a claimed panel behave like a keyboard
// ---------------------------------------------------------------------------

/// Counters worth printing when someone asks why the panel is not typing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TypethroughStats {
    /// Transitions handed to the injector and accepted.
    pub injected: u64,
    /// Transitions deliberately not injected because emulation owns them.
    pub suppressed: u64,
    /// Transitions for keys with no keyboard wire form (mouse pseudo-keys).
    pub unsupported: u64,
    /// Releases for keys this instance never pressed — dropped, see
    /// [`Typethrough::on_key`].
    pub orphan_releases: u64,
    /// Injector errors. Non-zero almost always means UIPI or the secure desktop.
    pub failed: u64,
}

/// Re-injects a claimed device's keystrokes whenever emulation is **not**
/// running, so a WinUSB-claimed arcade panel still drives the frontend menu.
///
/// ```text
///   emulation stopped                 emulation running
///   ------------------                -----------------
///   key down  -> SendInput down       key down  -> (engine owns it)
///   key up    -> SendInput up         key up    -> (engine owns it)
///
///   on start of emulation: release every key we are still holding down
///   on drop / release_all: same
/// ```
///
/// # The bug this type exists to prevent
///
/// A player holds P1-Left while the frontend launches a game. ksx starts
/// emulating mid-hold. If nothing releases the key, the *physical* release is
/// now consumed by the engine and never re-injected — so Windows believes Left
/// is held down forever, and the desktop scrolls sideways until someone taps a
/// real arrow key. [`Typethrough::set_emulating`] releases the held set on the
/// way in, and [`Drop`] releases it if ksx is torn down mid-hold. Both are
/// unit-tested; neither is reachable by inspection alone.
///
/// # Threading
///
/// Not `Sync` and deliberately owned by one thread — the same thread that drains
/// the capture channel. `SendInput` is thread-safe, but the held-key set is the
/// only record of what Windows believes, and two writers would lose strokes.
#[derive(Debug)]
pub struct Typethrough<I: KeyInjector> {
    injector: I,
    emulating: bool,
    /// Exactly the keys this instance has injected *down* and not yet released:
    /// the set Windows currently believes is held because of us.
    held: BTreeSet<Key>,
    stats: TypethroughStats,
}

impl<I: KeyInjector> Typethrough<I> {
    /// Starts **not** emulating — i.e. typing. That is the safe default and the
    /// state a freshly-claimed panel must be in: a daemon that comes up and does
    /// nothing should still leave the user able to navigate their frontend.
    pub fn new(injector: I) -> Self {
        Self {
            injector,
            emulating: false,
            held: BTreeSet::new(),
            stats: TypethroughStats::default(),
        }
    }

    pub fn is_emulating(&self) -> bool {
        self.emulating
    }

    pub fn stats(&self) -> TypethroughStats {
        self.stats
    }

    /// Keys Windows currently believes are held because of us.
    pub fn held(&self) -> impl Iterator<Item = Key> + '_ {
        self.held.iter().copied()
    }

    /// The injector, for configuration only (scripting a fake, reading its own
    /// counters). Injecting through it directly desynchronises [`Self::held`]
    /// and forfeits the stuck-key guarantee — go through [`Self::on_key`].
    pub fn injector_mut(&mut self) -> &mut I {
        &mut self.injector
    }

    /// Switch between "the panel is a keyboard" and "the panel is a gamepad".
    ///
    /// Entering emulation releases everything we are holding (see the type
    /// docs). Leaving it injects nothing: a key physically held across the
    /// transition will produce its own release, which is then an orphan and
    /// dropped — pressing it again is one keystroke, whereas synthesising a
    /// press for a key we never saw go down is a phantom input.
    pub fn set_emulating(&mut self, emulating: bool) {
        if emulating == self.emulating {
            return;
        }
        self.emulating = emulating;
        if emulating {
            self.release_all();
        }
    }

    /// Feed one capture event.
    pub fn on_event(&mut self, event: &KeyEvent) {
        self.on_key(event.key, event.down);
    }

    /// Feed one transition.
    ///
    /// A release for a key not in [`Self::held`] is dropped, not forwarded: it
    /// belongs to a press that happened while emulation owned the panel, and
    /// injecting a lone release for a key Windows never saw pressed can cancel a
    /// *real* keyboard's held key of the same code.
    pub fn on_key(&mut self, key: Key, down: bool) {
        if self.emulating {
            self.stats.suppressed += 1;
            return;
        }
        if stroke_for(key).is_none() {
            self.stats.unsupported += 1;
            return;
        }
        if !down && !self.held.contains(&key) {
            self.stats.orphan_releases += 1;
            return;
        }
        match self.injector.stroke(key, down) {
            Ok(()) => {
                self.stats.injected += 1;
                if down {
                    self.held.insert(key);
                } else {
                    self.held.remove(&key);
                }
            }
            Err(_) => {
                self.stats.failed += 1;
                // A failed *release* still leaves the key out of `held`: we have
                // no way to make Windows let go of it, and retrying on every
                // subsequent stroke would turn one blocked window into an
                // unbounded retry loop at keyboard rate. A failed *press* is
                // simply not held.
                if !down {
                    self.held.remove(&key);
                }
            }
        }
    }

    /// Release every key we are holding. Idempotent.
    ///
    /// Called on entering emulation and from [`Drop`]. Safe to call from a
    /// teardown path that may already have released.
    pub fn release_all(&mut self) {
        if self.held.is_empty() {
            return;
        }
        for key in std::mem::take(&mut self.held) {
            match self.injector.stroke(key, false) {
                Ok(()) => self.stats.injected += 1,
                Err(_) => self.stats.failed += 1,
            }
        }
    }
}

impl<I: KeyInjector> Drop for Typethrough<I> {
    fn drop(&mut self) {
        self.release_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A typethrough plus a handle to what it types. The handle outlives the
    /// value, which is the only way to assert the release-on-drop guarantee.
    fn typethrough() -> (Typethrough<RecordingInjector>, InjectionLog) {
        let injector = RecordingInjector::new();
        let log = injector.log();
        (Typethrough::new(injector), log)
    }

    // -----------------------------------------------------------------
    // Key -> wire form
    // -----------------------------------------------------------------

    /// Every key in the vocabulary is classified: typeable with a wire form, or
    /// explicitly not typeable. Nothing may fall through unexamined.
    #[test]
    fn every_key_is_either_typeable_or_explicitly_not() {
        for &key in Key::ALL {
            let stroke = stroke_for(key);
            let expect_none = key == Key::None || key == Key::Unknown || key.is_mouse_pseudo();
            assert_eq!(
                stroke.is_none(),
                expect_none,
                "{key} classified wrong (stroke = {stroke:?})"
            );
        }
    }

    /// The E0 table, written out here against the set-1 scancodes rather than
    /// derived from the capture table, so a transcription slip in either one
    /// shows up as a disagreement instead of matching itself.
    ///
    /// Each row is `(key, set-1 make code)` and every row is extended.
    #[test]
    fn every_extended_key_is_the_inverse_of_the_capture_table() {
        let rows: &[(Key, u16)] = &[
            // navigation cluster — the classic E0 twins of the numpad
            (Key::Up, 0x48),
            (Key::Down, 0x50),
            (Key::Left, 0x4B),
            (Key::Right, 0x4D),
            (Key::Home, 0x47),
            (Key::PageUp, 0x49),
            (Key::End, 0x4F),
            (Key::PageDown, 0x51),
            (Key::Insert, 0x52),
            (Key::Delete, 0x53),
            // right-hand modifiers share the left-hand codes
            (Key::RightControl, 0x1D),
            (Key::RightAlt, 0x38),
            // GUI + application key
            (Key::LeftWindows, 0x5B),
            (Key::RightWindows, 0x5C),
            (Key::Menu, 0x5D),
            // numpad keys whose codes belong to other keys
            (Key::NumpadEnter, 0x1C),
            (Key::NumpadDivide, 0x35),
            // print screen / break / the driver's fake shift
            (Key::PrintScreen, 0x37),
            (Key::Break, 0x46),
            (Key::ShiftModifier, 0x2A),
            // media
            (Key::MediaPreviousTrack, 0x10),
            (Key::MediaNextTrack, 0x19),
            (Key::MediaPlayPause, 0x22),
            (Key::VolumeUp, 0x30),
            (Key::VolumeDown, 0x2E),
            (Key::VolumeMute, 0x20),
            // the legacy OEM grab-bag
            (Key::Oem0, 0x01),
            (Key::Oem2, 0x12),
            (Key::Oem3, 0x17),
            (Key::Oem4, 0x6E),
            (Key::Oem5, 0x5F),
            (Key::Oem6, 0x0A),
            (Key::Oem7, 0x32),
            (Key::Oem13, 0x78),
        ];
        for &(key, code) in rows {
            assert_eq!(
                stroke_for(key),
                Some(KeyStroke::Scancode {
                    code,
                    extended: true
                }),
                "{key} must be E0 {code:#04X}"
            );
        }

        // And the set is exhaustive: no *other* key claims to be extended.
        let expected: BTreeSet<Key> = rows.iter().map(|(k, _)| *k).collect();
        for &key in Key::ALL {
            let is_ext = matches!(
                stroke_for(key),
                Some(KeyStroke::Scancode { extended: true, .. })
            );
            assert_eq!(
                is_ext,
                expected.contains(&key),
                "{key} extended-ness disagrees with the table"
            );
        }
    }

    /// The numpad/navigation collision is the whole reason the extended flag
    /// exists: the same make code must mean different things.
    #[test]
    fn numpad_twins_share_a_code_and_differ_only_in_the_extended_flag() {
        for (numpad, nav) in [
            (Key::Numpad8, Key::Up),
            (Key::Numpad2, Key::Down),
            (Key::Numpad4, Key::Left),
            (Key::Numpad6, Key::Right),
            (Key::Numpad7, Key::Home),
            (Key::Numpad9, Key::PageUp),
            (Key::Numpad1, Key::End),
            (Key::Numpad3, Key::PageDown),
            (Key::Numpad0, Key::Insert),
            (Key::NumpadDelete, Key::Delete),
        ] {
            let (nc, ne) = stroke_for(numpad).unwrap().scancode().unwrap();
            let (vc, ve) = stroke_for(nav).unwrap().scancode().unwrap();
            assert_eq!(nc, vc, "{numpad} and {nav} must share a make code");
            assert!(!ne, "{numpad} must not be extended");
            assert!(ve, "{nav} must be extended");
        }
        // Enter vs NumpadEnter and LeftControl vs RightControl, same story.
        assert_eq!(
            stroke_for(Key::Enter).unwrap().scancode(),
            Some((0x1C, false))
        );
        assert_eq!(
            stroke_for(Key::NumpadEnter).unwrap().scancode(),
            Some((0x1C, true))
        );
        assert_eq!(
            stroke_for(Key::LeftControl).unwrap().scancode(),
            Some((0x1D, false))
        );
        assert_eq!(
            stroke_for(Key::RightControl).unwrap().scancode(),
            Some((0x1D, true))
        );
    }

    /// Plain keys are the identity function on the legacy enum value, which is
    /// what makes the whole vocabulary layout-independent.
    #[test]
    fn plain_keys_send_their_enum_value_as_the_make_code() {
        for key in [
            Key::Escape,
            Key::One,
            Key::A,
            Key::Z,
            Key::Space,
            Key::Enter,
            Key::LeftShift,
            Key::RightShift,
            Key::LeftAlt,
            Key::LeftControl,
            Key::CapsLock,
            Key::F1,
            Key::F12,
            Key::NumLock,
            Key::ScrollLock,
            Key::Numpad5,
            Key::NumpadAsterisk,
            Key::Oem16,
            Key::LeftBackslashPipe,
        ] {
            assert_eq!(
                stroke_for(key),
                Some(KeyStroke::Scancode {
                    code: key.value(),
                    extended: false
                }),
                "{key}"
            );
        }
    }

    /// No two typeable keys may collapse onto the same wire form — that would
    /// silently type the wrong key.
    #[test]
    fn the_wire_form_is_injective() {
        let mut seen: std::collections::HashMap<KeyStroke, Key> = std::collections::HashMap::new();
        for &key in Key::ALL {
            let Some(stroke) = stroke_for(key) else {
                continue;
            };
            if let Some(other) = seen.insert(stroke, key) {
                panic!("{key} and {other} both map to {stroke:?}");
            }
        }
    }

    #[test]
    fn pause_is_the_documented_virtual_key_exception() {
        assert_eq!(
            stroke_for(Key::Pause),
            Some(KeyStroke::VirtualKey { vk: VK_PAUSE })
        );
        // ...and it is the ONLY one. Every other typeable key is a scancode,
        // or the layout independence claim in the module docs is false.
        for &key in Key::ALL {
            if key == Key::Pause {
                continue;
            }
            assert!(
                !matches!(stroke_for(key), Some(KeyStroke::VirtualKey { .. })),
                "{key} must not be injected as a virtual key"
            );
        }
    }

    #[test]
    fn mouse_pseudo_keys_are_never_injected() {
        for key in [
            Key::MouseLeftButton,
            Key::MouseWheelUp,
            Key::MouseMoveLeft,
            Key::MouseExtraRight,
        ] {
            assert_eq!(stroke_for(key), Option::None, "{key}");
            let mut rec = RecordingInjector::new();
            let log = rec.log();
            assert_eq!(
                rec.stroke(key, true),
                Err(InjectError::Unsupported(key)),
                "{key}"
            );
            assert!(log.events().is_empty());
        }
    }

    // -----------------------------------------------------------------
    // Typethrough policy
    // -----------------------------------------------------------------

    #[test]
    fn a_stopped_session_types_every_key_verbatim() {
        let (mut tt, log) = typethrough();
        for (key, down) in [
            (Key::Up, true),
            (Key::Up, false),
            (Key::Enter, true),
            (Key::Enter, false),
        ] {
            tt.on_key(key, down);
        }
        drop(tt);
        assert_eq!(log.trace(), "Up+ Up- Enter+ Enter-");
    }

    #[test]
    fn a_running_session_types_nothing() {
        let (mut tt, log) = typethrough();
        tt.set_emulating(true);
        tt.on_key(Key::A, true);
        tt.on_key(Key::A, false);
        assert_eq!(tt.stats().suppressed, 2);
        assert_eq!(tt.stats().injected, 0);
        drop(tt);
        assert_eq!(log.trace(), "");
    }

    /// **The stuck-key guarantee.** Emulation starting mid-hold must release
    /// what we are holding, or Windows believes the key is down forever.
    #[test]
    fn starting_emulation_releases_everything_held() {
        let (mut tt, log) = typethrough();
        tt.on_key(Key::Left, true);
        tt.on_key(Key::LeftShift, true);
        assert_eq!(tt.held().count(), 2);

        tt.set_emulating(true);
        assert_eq!(tt.held().count(), 0, "nothing may still be held");

        // Presses in arrival order, then a release for each — the release order
        // is the held set's (by key id) and deliberately not asserted.
        let events = log.events();
        assert_eq!(
            &events[..2],
            &[(Key::Left, true), (Key::LeftShift, true)],
            "{}",
            log.trace()
        );
        for key in [Key::Left, Key::LeftShift] {
            assert!(
                events.contains(&(key, false)),
                "{key} was never released: {}",
                log.trace()
            );
        }
    }

    /// The same guarantee on the teardown path: ksx dying mid-hold must not
    /// leave a key latched down on the desktop.
    #[test]
    fn dropping_mid_hold_releases_everything_held() {
        let (mut tt, log) = typethrough();
        tt.on_key(Key::Right, true);
        tt.on_key(Key::Space, true);
        drop(tt);
        let events = log.events();
        assert_eq!(&events[..2], &[(Key::Right, true), (Key::Space, true)]);
        for key in [Key::Right, Key::Space] {
            assert!(
                events.contains(&(key, false)),
                "{key} outlived the session held down: {}",
                log.trace()
            );
        }
    }

    /// The physical release that arrives *after* emulation started belongs to a
    /// press the engine owns; re-injecting it could cancel a real keyboard's
    /// identical key.
    #[test]
    fn a_release_for_a_key_we_never_pressed_is_dropped() {
        let (mut tt, log) = typethrough();
        tt.set_emulating(true);
        tt.on_key(Key::B, true); // suppressed, so never "held"
        tt.set_emulating(false);
        tt.on_key(Key::B, false); // the physical release lands here

        assert_eq!(tt.stats().orphan_releases, 1);
        drop(tt);
        assert_eq!(log.trace(), "", "nothing may be injected");
    }

    /// Stopping emulation must not synthesise presses for keys that were held
    /// while it ran: ksx never saw them go down through the typethrough path.
    #[test]
    fn stopping_emulation_injects_nothing() {
        let (mut tt, log) = typethrough();
        tt.set_emulating(true);
        tt.on_key(Key::Up, true);
        tt.set_emulating(false);
        drop(tt);
        assert_eq!(log.trace(), "");
    }

    #[test]
    fn toggling_to_the_same_state_is_a_no_op() {
        let (mut tt, log) = typethrough();
        tt.on_key(Key::A, true);
        tt.set_emulating(false); // already false
        assert_eq!(tt.held().collect::<Vec<_>>(), vec![Key::A]);
        tt.set_emulating(true);
        tt.set_emulating(true); // already true
        drop(tt);
        assert_eq!(log.trace(), "A+ A-");
    }

    /// Repeat presses (key-repeat, or a bounced switch) must not grow the held
    /// set or produce a matching pile of releases.
    #[test]
    fn repeated_presses_release_exactly_once() {
        let (mut tt, log) = typethrough();
        for _ in 0..5 {
            tt.on_key(Key::Down, true);
        }
        assert_eq!(tt.held().collect::<Vec<_>>(), vec![Key::Down]);
        tt.set_emulating(true);
        drop(tt);
        assert_eq!(log.trace(), "Down+ Down+ Down+ Down+ Down+ Down-");
    }

    /// A blocked press (UIPI, secure desktop) must not enter the held set — or
    /// teardown would inject a release for a press that never happened.
    #[test]
    fn a_blocked_press_is_counted_and_never_held() {
        let mut injector = RecordingInjector::new();
        let log = injector.log();
        injector.fail_next(InjectError::Blocked {
            sent: 0,
            expected: 1,
        });
        let mut tt = Typethrough::new(injector);
        tt.on_key(Key::F1, true);
        assert_eq!(tt.stats().failed, 1);
        assert_eq!(tt.stats().injected, 0);
        assert_eq!(tt.held().count(), 0);
        drop(tt);
        assert_eq!(log.trace(), "");
    }

    /// A blocked *release* must still leave the held set, or every later stroke
    /// would drag a permanent retry along behind it.
    #[test]
    fn a_blocked_release_still_leaves_the_held_set() {
        let injector = RecordingInjector::new();
        let log = injector.log();
        let mut tt = Typethrough::new(injector);
        tt.on_key(Key::G, true);
        tt.injector_mut().fail_next(InjectError::Blocked {
            sent: 0,
            expected: 1,
        });
        tt.on_key(Key::G, false);
        assert_eq!(tt.held().count(), 0);
        assert_eq!(tt.stats().failed, 1);
        drop(tt);
        assert_eq!(log.trace(), "G+", "the release was blocked, not repeated");
    }

    #[test]
    fn unsupported_keys_are_counted_not_injected() {
        let (mut tt, _log) = typethrough();
        tt.on_key(Key::MouseLeftButton, true);
        tt.on_key(Key::Unknown, true);
        assert_eq!(tt.stats().unsupported, 2);
        assert_eq!(tt.stats().injected, 0);
    }

    #[test]
    fn key_events_feed_straight_in() {
        let (mut tt, _log) = typethrough();
        tt.on_event(&KeyEvent {
            device: ksx_core::DeviceId::new("HID\\VID_D209&PID_0430&MI_00\\8&2A0D0500&0&0000"),
            key: Key::Enter,
            down: true,
            t: 0,
        });
        assert_eq!(tt.stats().injected, 1);
    }

    // -----------------------------------------------------------------
    // INPUT struct construction (Windows) — no SendInput is ever called
    // -----------------------------------------------------------------

    #[cfg(windows)]
    mod input_struct {
        use super::*;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            INPUT_KEYBOARD, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
        };

        /// Exhaustive over the vocabulary: every typeable key, both directions,
        /// checked field by field.
        #[test]
        fn every_typeable_key_builds_the_right_input_in_both_directions() {
            for &key in Key::ALL {
                let Some(stroke) = stroke_for(key) else {
                    continue;
                };
                for down in [true, false] {
                    let input = input_for(stroke, down);
                    assert_eq!(input.r#type, INPUT_KEYBOARD, "{key}");
                    let ki = unsafe { input.Anonymous.ki };
                    assert_eq!(ki.dwExtraInfo, KSX_EXTRA_INFO, "{key} must be tagged");
                    assert_eq!(ki.time, 0, "{key} must let the system stamp the time");

                    match stroke {
                        KeyStroke::Scancode { code, extended } => {
                            assert_eq!(ki.wScan, code, "{key} scancode");
                            assert_eq!(
                                ki.wVk, 0,
                                "{key}: wVk must be 0 or the system ignores wScan"
                            );
                            assert_ne!(
                                ki.dwFlags & KEYEVENTF_SCANCODE,
                                0,
                                "{key} must be sent as a scancode"
                            );
                            assert_eq!(
                                ki.dwFlags & KEYEVENTF_EXTENDEDKEY != 0,
                                extended,
                                "{key} extended flag"
                            );
                        }
                        KeyStroke::VirtualKey { vk } => {
                            assert_eq!(ki.wVk, vk, "{key} virtual key");
                            assert_eq!(ki.wScan, 0, "{key}");
                            assert_eq!(
                                ki.dwFlags & KEYEVENTF_SCANCODE,
                                0,
                                "{key} is the vk exception"
                            );
                        }
                    }
                    assert_eq!(
                        ki.dwFlags & KEYEVENTF_KEYUP != 0,
                        !down,
                        "{key} KEYUP flag for down={down}"
                    );
                }
            }
        }

        /// The three E0 keys most likely to be got wrong, spelled out as raw
        /// flag values so a change to the mapping cannot pass by redefining a
        /// helper.
        #[test]
        fn extended_keys_carry_exactly_scancode_plus_extendedkey() {
            for (key, code) in [
                (Key::Up, 0x48u16),
                (Key::RightAlt, 0x38),
                (Key::NumpadEnter, 0x1C),
            ] {
                let down = input_for(stroke_for(key).unwrap(), true);
                let ki = unsafe { down.Anonymous.ki };
                assert_eq!(ki.wScan, code, "{key}");
                assert_eq!(
                    ki.dwFlags,
                    KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY,
                    "{key} down flags"
                );
                let up = input_for(stroke_for(key).unwrap(), false);
                let ki = unsafe { up.Anonymous.ki };
                assert_eq!(
                    ki.dwFlags,
                    KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP,
                    "{key} up flags"
                );
            }
        }

        /// A plain key must NOT carry the extended flag — the single most
        /// consequential bit in this module (E0 0x4B is Left, 0x4B is Numpad4).
        #[test]
        fn plain_keys_never_carry_the_extended_flag() {
            for key in [Key::A, Key::Numpad4, Key::Enter, Key::LeftControl] {
                let input = input_for(stroke_for(key).unwrap(), true);
                let ki = unsafe { input.Anonymous.ki };
                assert_eq!(ki.dwFlags, KEYEVENTF_SCANCODE, "{key}");
            }
        }
    }
}
