//! How many of Windows' four XInput slots are occupied **right now**.
//!
//! This exists because counting ViGEmBus children answers a different
//! question. [`crate::virtual_pads`] can only ever see ksx's own virtual pads
//! — its module header says so explicitly: "a REAL DualShock or Xbox pad
//! plugged into the cabinet carries the very same ids, but it hangs off a USB
//! hub devnode, never off ViGEmBus, so it cannot reach the classifier at all."
//! A cabinet with two real wired Xbox 360 pads and no virtual ones therefore
//! reports zero XInput pads from the bus, and any sentence built on that
//! number ("so four more will be readable") is wrong by two.
//!
//! `XInputGetState` asks the thing that actually hands out the slots. Real
//! pads, virtual pads and pads created by some other process all count, which
//! is exactly the property a "how many more will a game see" sentence needs.
//!
//! **Read-only and cheap.** No handle is opened, no state is changed, and the
//! four calls are a poll of a driver that is already resident. It is *not*
//! free of races — another process can take a slot a microsecond later — so
//! callers must phrase the result as a ceiling ("at most N more"), never as a
//! promise.

/// How many of the [`ksx_core::MAX_XINPUT_SLOTS`] slots have a pad in them.
///
/// `None` means **ksx could not ask** — not "none are in use". The two are
/// different sentences and a surface must render them differently: a caller
/// that folds `None` into `0` produces "four more pads will be readable" on a
/// machine it never actually looked at.
#[cfg(windows)]
pub fn slots_in_use() -> Option<u8> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::UI::Input::XboxController::{XInputGetState, XINPUT_STATE};

    let mut used = 0u8;
    for index in 0..u32::from(ksx_core::MAX_XINPUT_SLOTS) {
        // SAFETY: `state` is a plain C struct that zeroes validly and
        // XInputGetState fills it on success. The call takes a scalar index
        // and an out-pointer to storage we own; it cannot fail unsafely. The
        // same call ksx-cabinet's `read_pad` makes every frame.
        let mut state: XINPUT_STATE = unsafe { std::mem::zeroed() };
        let result = unsafe { XInputGetState(index, &mut state) };
        // Anything other than success — ERROR_DEVICE_NOT_CONNECTED included —
        // means no pad is answering on this index.
        if result == ERROR_SUCCESS {
            used += 1;
        }
    }
    Some(used)
}

/// Off Windows there are no XInput slots to read, and saying "zero are in
/// use" would be a claim about a machine this build cannot inspect.
#[cfg(not(windows))]
pub fn slots_in_use() -> Option<u8> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever this machine answers, it cannot answer more slots than
    /// Windows has — and off Windows it must refuse to answer at all rather
    /// than report an empty machine it never looked at.
    #[test]
    fn the_count_never_exceeds_the_ceiling_and_never_guesses() {
        let answer = slots_in_use();
        if cfg!(windows) {
            let used = answer.expect("Windows can always ask XInput");
            assert!(
                used <= ksx_core::MAX_XINPUT_SLOTS,
                "{used} occupied slots is more than Windows has"
            );
        } else {
            assert_eq!(
                answer, None,
                "off Windows this must be unanswerable, never 0 — 0 is a claim"
            );
        }
    }
}
