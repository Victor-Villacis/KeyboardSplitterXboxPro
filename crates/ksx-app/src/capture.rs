//! Building the session's capture backend from a resolved plan (M6).
//!
//! Until M6 this was one line — `InterceptionBackend::new()`. Backend choice is
//! now per device (`[[device]] backend = "interception" | "winusb"`), so a
//! session can need one claimed WinUSB interface per rebound board *and* an
//! Interception context for whatever is still on the keyboard stack. This module
//! is the one place that turns a [`RunPlan`] into the right shape, so `ksx run`
//! and the daemon cannot drift apart on it.
//!
//! # The three shapes
//!
//! | plan | backend |
//! |---|---|
//! | no `winusb` devices | `InterceptionBackend` alone — byte-identical to M3–M5 |
//! | some of each | `CompositeBackend` of N `WinUsbBackend`s + one `InterceptionBackend` |
//! | all `winusb` | `CompositeBackend` of N `WinUsbBackend`s, **no Interception context created** |
//!
//! That last row is the milestone's exit criterion: a full session with the
//! end-of-life driver uninstalled. It falls out of
//! [`RunPlan::needs_interception`] rather than being a special case anyone has
//! to remember.
//!
//! # Ordering, and why it is this way round
//!
//! WinUSB interfaces are claimed **first**, before the Interception context
//! exists. A claim failure (board unplugged, rebind not performed, descriptor
//! unreadable) then happens with every keyboard on the machine still completely
//! normal — no filter armed, nothing to undo. The reverse order would create the
//! Interception context first and leave it to a `Drop` on an error path.
//!
//! # What this module will not do
//!
//! It never rebinds a device. If an interface is configured for WinUSB but is
//! still on the keyboard stack, the run is **refused** with the exact `pnputil`
//! situation spelled out. Rebinding is a supervised manual step
//! (`docs/MIGRATION-WINUSB.md`, `docs/RECOVERY.md` §2): a wrong one takes the arcade panel
//! offline until someone can reach a keyboard that still types.

#![cfg(windows)]

use ksx_capture::{
    CaptureBackend, CaptureError, CompositeBackend, Handles, InterceptionBackend, UsbCandidate,
    WinUsbBackend,
};
use ksx_core::DeviceId;

use crate::daemon::panel::Panel;
use crate::run::plan::RunPlan;

/// Why a session could not get the capture backend it asked for.
///
/// Every variant is an exit-code-2 refusal: nothing was plugged and no keyboard
/// filter was ever armed.
#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("{0}")]
    Interception(CaptureError),
    #[error("USB enumeration failed: {0}")]
    Enumeration(std::io::Error),
    #[error(
        "no connected USB interface has the instance path {id}. If that is an Interception \
         hardware id (starts with HID\\), the [[device]] entry was switched to \
         backend = \"winusb\" without updating its id — run `ksx devices` for the USB\\ id to \
         paste in (docs/MIGRATION-WINUSB.md)"
    )]
    NoSuchDevice { id: String },
    #[error("{0}")]
    Claim(CaptureError),
    #[error(
        "the daemon claimed its panel at startup and {id} is not part of it — the \
         configuration named this device as backend = \"winusb\" after the daemon was \
         already running. Restart the daemon (`quit`, then `ksx daemon`) so it can claim \
         it: ksx never claims a device in the middle of a session"
    )]
    PanelMissing { id: String },
    #[error("could not start the daemon's panel threads: {0}")]
    PanelStart(std::io::Error),
}

impl SetupError {
    /// Stable `--json` error code.
    pub fn code(&self) -> &'static str {
        match self {
            SetupError::Interception(_) => "interception-unavailable",
            SetupError::Enumeration(_) => "usb-enumeration-failed",
            SetupError::NoSuchDevice { .. } => "winusb-device-missing",
            SetupError::Claim(CaptureError::NotRebound { .. }) => "winusb-rebind-missing",
            SetupError::Claim(_) => "winusb-claim-failed",
            SetupError::PanelMissing { .. } => "winusb-panel-missing",
            SetupError::PanelStart(_) => "winusb-panel-start-failed",
        }
    }
}

/// Build the capture backend a plan requires.
///
/// **Claims interfaces.** Only `ksx run` and the daemon may call it; anything
/// that merely wants to *look* at devices uses `ksx_capture::usb_candidates()`,
/// which opens nothing.
pub fn build(plan: &RunPlan) -> Result<Box<dyn CaptureBackend>, SetupError> {
    // One shared health state and — the part that matters when you are standing
    // at a wedged cabinet — one shared escape latch across every backend, so
    // `LeftCtrl x5` on any board frees them all.
    build_with(plan, Handles::new())
}

/// [`build`], publishing into handles the caller already holds.
///
/// The seam `ksx play` needs: it composes this backend with a replay source,
/// and the two have to share one escape latch or a gesture on the real panel
/// would flip a latch the supervisor is not watching — the hatch would look
/// dead at exactly the moment somebody needed it.
pub fn build_with(plan: &RunPlan, handles: Handles) -> Result<Box<dyn CaptureBackend>, SetupError> {
    if plan.winusb.is_empty() {
        // Unchanged from M3–M5: one backend, no composition.
        return InterceptionBackend::new_with(handles)
            .map(|b| Box::new(b) as Box<dyn CaptureBackend>)
            .map_err(SetupError::Interception);
    }

    let candidates = ksx_capture::usb_candidates().map_err(SetupError::Enumeration)?;

    let mut children: Vec<Box<dyn CaptureBackend>> = Vec::with_capacity(plan.winusb.len() + 1);
    for id in &plan.winusb {
        let candidate = find(&candidates, id).ok_or_else(|| SetupError::NoSuchDevice {
            id: id.as_str().to_owned(),
        })?;
        // Passthrough on a claimed interface is synthesis, not a re-send: the
        // board has left the keyboard stack, so nothing else can deliver its
        // keystrokes to Windows.
        let injector = Box::new(ksx_platform::inject::SendInputInjector::new());
        let backend = WinUsbBackend::claim_with(candidate, injector, handles.clone())
            .map_err(SetupError::Claim)?;
        children.push(Box::new(backend));
    }

    // Only now, with every claim already succeeded, is the end-of-life driver
    // touched — and only if something still needs it.
    if plan.needs_interception() {
        let backend =
            InterceptionBackend::new_with(handles.clone()).map_err(SetupError::Interception)?;
        children.push(Box::new(backend));
    }

    Ok(Box::new(CompositeBackend::new(children, handles)))
}

fn find<'a>(candidates: &'a [UsbCandidate], id: &DeviceId) -> Option<&'a UsbCandidate> {
    candidates.iter().find(|c| &c.id == id)
}

// ---------------------------------------------------------------------------
// M6: the daemon's claim, and the sessions that borrow it
// ---------------------------------------------------------------------------

/// Claim every WinUSB interface the plan names, **once**, for the daemon's
/// whole lifetime.
///
/// Returns `None` when the plan names none, which is every Interception-only
/// configuration — those devices are still keyboards between sessions and the
/// daemon has nothing to hold.
///
/// **Claims interfaces.** Only `daemon::run` may call it, exactly once, before
/// the control loop starts. See [`crate::daemon::panel`] for why the ownership
/// is inverted relative to M4/M5, and note the injector: the panel's backend is
/// built with [`ksx_platform::inject::NullInjector`] because the panel's
/// typethrough is the single thing that types for a claimed board.
pub fn claim_panel(plan: &RunPlan) -> Result<Option<std::sync::Arc<Panel>>, SetupError> {
    if plan.winusb.is_empty() {
        return Ok(None);
    }
    let handles = Handles::new();
    let candidates = ksx_capture::usb_candidates().map_err(SetupError::Enumeration)?;

    let mut children: Vec<Box<dyn CaptureBackend>> = Vec::with_capacity(plan.winusb.len());
    for id in &plan.winusb {
        let candidate = find(&candidates, id).ok_or_else(|| SetupError::NoSuchDevice {
            id: id.as_str().to_owned(),
        })?;
        let backend = WinUsbBackend::claim_with(
            candidate,
            Box::new(ksx_platform::inject::NullInjector),
            handles.clone(),
        )
        .map_err(SetupError::Claim)?;
        children.push(Box::new(backend));
    }

    // One claimed board is the cabinet's case; wrapping it in a composite would
    // buy nothing but a thread.
    let backend: Box<dyn CaptureBackend> = if children.len() == 1 {
        children.pop().expect("checked")
    } else {
        Box::new(CompositeBackend::new(children, handles.clone()))
    };
    let panel = Panel::start(
        backend,
        handles,
        ksx_platform::inject::SendInputInjector::new(),
    )
    .map_err(SetupError::PanelStart)?;
    Ok(Some(panel))
}

/// Build one session's capture backend, borrowing the daemon's claim.
///
/// This is [`build`] with the WinUSB half replaced by a view of an existing
/// claim: nothing is claimed, nothing is released, and the same
/// [`Handles`] cover the whole session so `LeftCtrl ×5` on the panel still frees
/// the Interception-backed boards next to it.
///
/// `panel` is `None` for `ksx run`, which has no daemon and therefore claims per
/// run — unchanged from the M6 shape that path was written with.
///
/// Known corner: if the daemon holds a claim and the configuration is then
/// edited so that *no* device uses it, this falls through to [`build`] and the
/// session never attaches the panel — so a board the daemon still holds stays
/// muted for the length of that session (the control loop muted it at start) and
/// types again when the session ends. Not a brick and not a claim anyone loses,
/// but the fix is a daemon restart, which is what changing which board is
/// claimed needs anyway.
pub fn build_session(
    plan: &RunPlan,
    panel: Option<&std::sync::Arc<Panel>>,
) -> Result<Box<dyn CaptureBackend>, SetupError> {
    // No claim, or a plan that wants none: the ordinary path, untouched.
    let (Some(panel), false) = (panel, plan.winusb.is_empty()) else {
        return build(plan);
    };

    // The daemon claimed what the configuration said at startup. If the
    // configuration has since named another board, say so — claiming it now
    // would be ksx taking a keyboard off the machine that nobody asked it to
    // take at the moment they asked for a game.
    for id in &plan.winusb {
        if !panel.covers(id) {
            return Err(SetupError::PanelMissing {
                id: id.as_str().to_owned(),
            });
        }
    }

    let handles = panel.handles();
    let mut children: Vec<Box<dyn CaptureBackend>> = vec![panel.session_backend()];
    if plan.needs_interception() {
        let backend =
            InterceptionBackend::new_with(handles.clone()).map_err(SetupError::Interception)?;
        children.push(Box::new(backend));
    }
    if children.len() == 1 {
        return Ok(children.pop().expect("checked"));
    }
    Ok(Box::new(CompositeBackend::new(children, handles)))
}

#[cfg(test)]
mod tests {
    use ksx_capture::winusb::Binding;

    use super::*;

    const IPAC_A: &str = "USB\\VID_D209&PID_0430&MI_00\\7&1A2B3C4D&0&0000";
    const IPAC_B: &str = "USB\\VID_D209&PID_0430&MI_00\\7&5E6F7A8B&0&0000";

    fn candidate(id: &str, binding: Binding) -> UsbCandidate {
        UsbCandidate {
            id: DeviceId::from(id),
            parent_id: "USB\\VID_D209&PID_0430\\4".into(),
            vendor_id: 0xD209,
            product_id: 0x0430,
            interface_number: 0,
            interface_class: 0x03,
            interface_subclass: 1,
            interface_protocol: 1,
            interface_string: None,
            serial: None,
            product: Some("I-PAC".into()),
            device_desc: Some("HID Keyboard Device".into()),
            port_chain: vec![1, 4],
            bus_id: "1".into(),
            binding,
        }
    }

    /// Two identical boards must be found separately by instance path — the
    /// lookup that makes T4 work. A `find` that matched on VID/PID would return
    /// the same board twice and both players would share one panel.
    #[test]
    fn lookup_is_by_instance_path_not_by_model() {
        let all = vec![
            candidate(IPAC_A, Binding::WinUsb),
            candidate(IPAC_B, Binding::WinUsb),
        ];
        let a = find(&all, &DeviceId::from(IPAC_A)).expect("board A");
        let b = find(&all, &DeviceId::from(IPAC_B)).expect("board B");
        assert_eq!(a.id.as_str(), IPAC_A);
        assert_eq!(b.id.as_str(), IPAC_B);
        assert_ne!(a.id, b.id, "identical hardware, distinct identities");
        assert!(find(&all, &DeviceId::from("USB\\GONE")).is_none());
        // An Interception hardware id can never match a USB instance path,
        // which is exactly what makes the migration mistake diagnosable.
        assert!(find(
            &all,
            &DeviceId::from("HID\\VID_D209&PID_0430&REV_0056&MI_00")
        )
        .is_none());
    }

    /// The error a half-migrated config produces has to name the fix. This is
    /// the message a user meets after flipping `backend` and nothing else.
    #[test]
    fn a_missing_device_error_points_at_the_migration() {
        let err = SetupError::NoSuchDevice {
            id: "HID\\VID_D209&PID_0430&REV_0056&MI_00".into(),
        };
        let text = err.to_string();
        assert!(text.contains("Interception hardware id"), "{text}");
        assert!(text.contains("ksx devices"), "{text}");
        assert_eq!(err.code(), "winusb-device-missing");
    }

    #[test]
    fn a_missing_rebind_has_its_own_code_and_never_offers_to_fix_it() {
        let err = SetupError::Claim(CaptureError::NotRebound {
            id: IPAC_A.into(),
            bound: "hidusb.sys (keyboard stack)".into(),
        });
        assert_eq!(err.code(), "winusb-rebind-missing");
        let text = err.to_string();
        assert!(text.contains("ksx never rebinds a device itself"), "{text}");
    }

    #[test]
    fn error_codes_are_distinct_and_stable() {
        let codes = [
            SetupError::Interception(CaptureError::DriverUnavailable).code(),
            SetupError::Enumeration(std::io::Error::other("x")).code(),
            SetupError::NoSuchDevice { id: "x".into() }.code(),
            SetupError::Claim(CaptureError::NotRebound {
                id: "x".into(),
                bound: "y".into(),
            })
            .code(),
            SetupError::Claim(CaptureError::NotAKeyboard { id: "x".into() }).code(),
        ];
        let mut sorted = codes.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len(), "codes must be distinguishable");
    }
}
