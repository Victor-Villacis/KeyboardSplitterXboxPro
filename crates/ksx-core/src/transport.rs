//! How a device is attached, and which capture backends that lets ksx use.
//!
//! # The rule this module exists to state once
//!
//! **A Bluetooth keyboard can be captured by Interception and can never be
//! WinUSB-claimed.** Both halves surprise people, and neither is guessable from
//! the rest of the codebase:
//!
//! - It *can* be Interception-captured because Interception is a class filter
//!   on the Windows keyboard stack. A paired Bluetooth keyboard is a
//!   keyboard-class devnode on that stack exactly like a USB one, so splitting
//!   it into virtual pads works today, with no new code.
//! - It can *never* be WinUSB-claimed because a claim is an INF that binds a
//!   **USB interface** by hardware id (`USB\VID_xxxx&PID_yyyy&MI_zz`). A
//!   Bluetooth device has no USB interface for such an INF to match. That is
//!   the transport, not a feature ksx has not written yet, and saying "not
//!   supported" would invite someone to wait for a release that cannot come.
//!
//! # Why it lives in `ksx-core`
//!
//! `docs/SURFACES.md` §1: the backend owns state and every surface is a view.
//! The set of backends a device is eligible for is a ROSTER — the same shape as
//! [`crate::MAX_SLOTS`] — so it is decided here, once, and served. The
//! enumerators (`ksx-capture`), the claim planner (`ksx-platform`), the writer
//! (`ksx device pick`), the CLI report, Studio's Rust seam and Studio's
//! TypeScript island all read [`Reach::eligibility`] rather than each deciding
//! what "Bluetooth" implies. A sentence composed in TypeScript is the specific
//! bug that rule exists for.
//!
//! Nothing here reads hardware. It is a pure function of four facts a caller
//! has already established, which is why every wording below is exercised in
//! CI on any platform.

use std::fmt;

/// How a devnode reaches the machine.
///
/// Deliberately only the two ksx can say something true about. An enumerator
/// ksx does not model (`ACPI\`, `ROOT\`, `HID\`) yields `None` from
/// [`Transport::of_instance_path`] rather than being folded into one of these:
/// a wrong transport would produce a confident wrong sentence about which
/// backends can reach the device, which is the whole thing this module is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Transport {
    /// `USB\…` — a USB interface. The only transport a WinUSB claim can bind.
    Usb,
    /// `BTHENUM\…` — a paired Bluetooth device's service node.
    Bluetooth,
}

impl Transport {
    /// The stable word every surface renders and every `--json` consumer keys
    /// on.
    pub fn code(self) -> &'static str {
        match self {
            Transport::Usb => "usb",
            Transport::Bluetooth => "bluetooth",
        }
    }

    /// The word a human reads in a column header cell.
    pub fn label(self) -> &'static str {
        match self {
            Transport::Usb => "USB",
            Transport::Bluetooth => "Bluetooth",
        }
    }

    /// Could a WinUSB claim ever bind a device on this transport?
    ///
    /// A property of the transport alone, and permanent. See the module docs
    /// for why "no" here is not the same sentence as "not yet".
    pub fn can_winusb(self) -> bool {
        matches!(self, Transport::Usb)
    }

    /// Which transport a Windows device instance path names, when it is one
    /// ksx models.
    ///
    /// `None` for every other enumerator — `HID\`, `ACPI\`, `ROOT\`,
    /// `BTHLEDEVICE\`. `ksx-core`'s selector grammar already refuses to build a
    /// `usb:` selector out of those (`selector.rs`), and this refuses to
    /// describe their backends for the same reason.
    pub fn of_instance_path(instance_id: &str) -> Option<Self> {
        let enumerator = instance_id.split('\\').next()?;
        if enumerator.eq_ignore_ascii_case("USB") {
            Some(Transport::Usb)
        } else if enumerator.eq_ignore_ascii_case("BTHENUM") {
            Some(Transport::Bluetooth)
        } else {
            None
        }
    }
}

impl fmt::Display for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Why a WinUSB claim can never reach a Bluetooth device.
///
/// A named constant because it is the sentence this whole task exists to put in
/// front of a user, and because four surfaces have to say it identically: the
/// scan report, `ksx devices`, Studio, and the refusal `ksx device pick
/// --backend winusb` produces on a Bluetooth row.
pub const WINUSB_NEEDS_A_USB_INTERFACE: &str =
    "WinUSB binds a USB interface through an INF hardware id, and a Bluetooth device has no USB \
     interface to bind. That is the transport, not a missing feature — no future ksx release \
     claims this device.";

/// Why a claim has nothing to take on a devnode with no keyboard behind it.
pub const WINUSB_NEEDS_A_KEYBOARD: &str =
    "there are no keyboard reports on this devnode, so a claim would take a device off the input \
     stack and hand ksx no keys.";

/// Why Interception cannot see an interface ksx already holds.
pub const INTERCEPTION_NEEDS_THE_STACK: &str =
    "the interface is bound to winusb.sys, so it is not on the Windows keyboard stack for a class \
     filter to see.";

/// The four facts that decide which backends can reach one devnode.
///
/// A struct rather than four positional `bool`s: three of them are booleans
/// and a call site that transposed two would compile and lie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reach {
    pub transport: Transport,
    /// Does this devnode carry keyboard reports at all?
    ///
    /// For USB this is "it is a HID interface with a keyboard child"; for
    /// Bluetooth it is "the node is keyboard-class". Whoever enumerated it
    /// knows; this module does not guess.
    pub keyboard: bool,
    /// Bound to `winusb.sys` right now — ksx (or something else) holds it.
    /// Always `false` off the USB transport, because nothing else can be.
    pub claimed: bool,
    /// **Can it deliver a keystroke to Windows right now?**
    ///
    /// A paired-but-switched-off Bluetooth keyboard reports PRESENT all day
    /// (`CM_PROB_DEVICE_NOT_CONNECTED`) and cannot type a character. It is
    /// still eligible for Interception — it will work the moment it connects —
    /// but a sentence that did not distinguish the two would tell a user they
    /// have a spare keyboard when it is in a drawer with dead batteries.
    pub can_type: bool,
}

/// Which backends can reach a device, and the sentences that say so.
///
/// Composed in `ksx-core` so no surface composes them. See the module docs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Eligibility {
    /// Could the Interception backend capture this device — now, or as soon as
    /// it connects?
    pub interception: bool,
    /// Could the WinUSB backend ever capture this device?
    pub winusb: bool,
    /// The one line a row shows: which backends, and the caveat that goes with
    /// each. Never empty.
    pub line: String,
    /// Why [`Self::winusb`] is `false`, or empty when it is `true`.
    pub winusb_reason: &'static str,
    /// Is [`Self::winusb`] being `false` a property of the TRANSPORT — i.e.
    /// permanent, and not fixable by claiming, rebooting or waiting for a
    /// release?
    pub winusb_impossible_by_transport: bool,
}

impl Reach {
    /// Which backends can reach this devnode.
    pub fn eligibility(self) -> Eligibility {
        let winusb = self.transport.can_winusb() && self.keyboard;
        let winusb_reason = if winusb {
            ""
        } else if !self.transport.can_winusb() {
            WINUSB_NEEDS_A_USB_INTERFACE
        } else {
            WINUSB_NEEDS_A_KEYBOARD
        };
        // Interception is a class filter on the Windows keyboard stack, so it
        // reaches any keyboard-class devnode that is still ON that stack. A
        // claimed interface is not, by construction — that is what a claim
        // does.
        let interception = self.keyboard && !self.claimed;
        Eligibility {
            interception,
            winusb,
            line: self.line(interception, winusb),
            winusb_reason,
            winusb_impossible_by_transport: !self.transport.can_winusb(),
        }
    }

    fn line(self, interception: bool, winusb: bool) -> String {
        let transport = self.transport.label();
        match (interception, winusb) {
            // A USB keyboard ksx already holds.
            (false, true) if self.claimed => format!(
                "{transport} · winusb: capturing now (claimed) · interception: no — \
                 {INTERCEPTION_NEEDS_THE_STACK}"
            ),
            // The ordinary USB keyboard, still on the Windows stack.
            (true, true) => format!(
                "{transport} · interception: yes, now · winusb: yes, after `ksx winusb claim` \
                 takes it off the Windows keyboard stack"
            ),
            // The Bluetooth keyboard — the whole point of this module.
            (true, false) if self.can_type => format!(
                "{transport} · interception: yes, now — it is a keyboard on the Windows input \
                 stack, so ksx can split it into virtual pads today · winusb: never — \
                 {WINUSB_NEEDS_A_USB_INTERFACE}"
            ),
            // Paired, present, and not delivering a keystroke.
            (true, false) => format!(
                "{transport} · interception: yes, but not while it is disconnected — it is \
                 paired and present, and it cannot type right now · winusb: never — \
                 {WINUSB_NEEDS_A_USB_INTERFACE}"
            ),
            // No keyboard behind it at all, on either transport.
            (false, false) if !self.transport.can_winusb() => format!(
                "{transport} · no backend — no keyboard reports on this devnode, and \
                 {WINUSB_NEEDS_A_USB_INTERFACE}"
            ),
            (false, false) => format!("{transport} · no backend — {WINUSB_NEEDS_A_KEYBOARD}"),
            // `winusb && !interception && !claimed`: a USB keyboard interface
            // that is neither claimed nor on the keyboard stack. Reachable when
            // a foreign driver owns it.
            (false, true) => format!(
                "{transport} · winusb: possible, after `ksx winusb claim` · interception: no — \
                 nothing on the Windows keyboard stack is reading it"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bt(keyboard: bool, can_type: bool) -> Reach {
        Reach {
            transport: Transport::Bluetooth,
            keyboard,
            claimed: false,
            can_type,
        }
    }

    fn usb(keyboard: bool, claimed: bool) -> Reach {
        Reach {
            transport: Transport::Usb,
            keyboard,
            claimed,
            can_type: !claimed,
        }
    }

    #[test]
    fn a_transport_is_read_off_the_enumerator_and_unknown_ones_stay_unknown() {
        assert_eq!(
            Transport::of_instance_path(r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000"),
            Some(Transport::Usb)
        );
        assert_eq!(
            Transport::of_instance_path(
                r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002045E_PID&0800\7&2A0B8CBA&0&001BDC0F1FE7_C00000000"
            ),
            Some(Transport::Bluetooth)
        );
        // Case is Windows' business, not ours.
        assert_eq!(
            Transport::of_instance_path(r"bthenum\x\y"),
            Some(Transport::Bluetooth)
        );
        // An enumerator ksx does not model must not be folded into one it does:
        // a wrong transport produces a confident wrong sentence about backends.
        assert_eq!(
            Transport::of_instance_path(r"HID\VID_D209&PID_0430\x"),
            None
        );
        assert_eq!(Transport::of_instance_path(r"ACPI\PNP0303\4&x"), None);
        assert_eq!(Transport::of_instance_path("BTHLEDEVICE\\x\\y"), None);
        assert_eq!(Transport::of_instance_path(""), None);
    }

    /// **The rule the list exists to teach, in one assertion.**
    ///
    /// A Bluetooth keyboard is eligible for Interception and never for WinUSB,
    /// and the "never" says which transport fact makes it never.
    ///
    /// Breaks against: any implementation that treats "not USB" as "not
    /// capturable" (the shipped state — `ksx device scan` walked the USB tree
    /// and a BTHENUM keyboard did not appear in it at all), and against one
    /// that reports the WinUSB gap as a missing feature rather than the
    /// transport.
    #[test]
    fn a_bluetooth_keyboard_is_interception_eligible_and_never_winusb() {
        let e = bt(true, true).eligibility();
        assert!(
            e.interception,
            "a BT keyboard is on the Windows input stack"
        );
        assert!(!e.winusb);
        assert!(e.winusb_impossible_by_transport);
        assert_eq!(e.winusb_reason, WINUSB_NEEDS_A_USB_INTERFACE);
        assert!(
            e.winusb_reason.contains("no USB interface to bind"),
            "the reason must name the transport fact: {}",
            e.winusb_reason
        );
        assert!(
            e.winusb_reason.contains("not a missing feature"),
            "'not supported' invites waiting for a release that cannot come: {}",
            e.winusb_reason
        );
        assert!(e.line.contains("Bluetooth"), "{}", e.line);
        assert!(e.line.contains("interception: yes"), "{}", e.line);
        assert!(e.line.contains("never"), "{}", e.line);
    }

    /// The trap, in the sentence a user reads: a paired-but-disconnected
    /// keyboard is still Interception-eligible (it will work when it connects)
    /// and the line says out loud that it cannot type right now.
    ///
    /// Breaks against: reporting `can_type` nowhere, which is the shape that
    /// lets someone read "2 keyboards" and claim their panel.
    #[test]
    fn a_paired_but_disconnected_bluetooth_keyboard_says_it_cannot_type_now() {
        let e = bt(true, false).eligibility();
        assert!(
            e.interception,
            "it becomes capturable the moment it connects"
        );
        assert!(!e.winusb);
        assert!(
            e.line.contains("not while it is disconnected"),
            "{}",
            e.line
        );
        assert!(e.line.contains("cannot type right now"), "{}", e.line);
        // ...and it must NOT read like the connected one.
        assert_ne!(e.line, bt(true, true).eligibility().line);
    }

    #[test]
    fn a_usb_keyboard_on_the_stack_is_eligible_for_both_and_names_the_claim() {
        let e = usb(true, false).eligibility();
        assert!(e.interception && e.winusb);
        assert_eq!(e.winusb_reason, "");
        assert!(!e.winusb_impossible_by_transport);
        assert!(e.line.contains("ksx winusb claim"), "{}", e.line);
    }

    /// A claimed interface is off the Windows keyboard stack — that is what a
    /// claim IS — so Interception cannot see it, and the row has to say so
    /// rather than listing both backends as available.
    #[test]
    fn a_claimed_usb_interface_is_winusb_only_and_says_why() {
        let e = usb(true, true).eligibility();
        assert!(e.winusb);
        assert!(!e.interception);
        assert!(e.line.contains("capturing now"), "{}", e.line);
        assert!(e.line.contains("winusb.sys"), "{}", e.line);
    }

    /// A devnode with no keyboard behind it gets neither backend, and the two
    /// transports give different reasons — the Bluetooth one still has to
    /// carry the transport fact, because "no backend" alone reads as "ksx has
    /// not got round to it".
    #[test]
    fn a_devnode_with_no_keyboard_gets_no_backend_on_either_transport() {
        let u = usb(false, false).eligibility();
        assert!(!u.interception && !u.winusb);
        assert_eq!(u.winusb_reason, WINUSB_NEEDS_A_KEYBOARD);
        assert!(!u.winusb_impossible_by_transport, "USB could, in principle");

        let b = bt(false, true).eligibility();
        assert!(!b.interception && !b.winusb);
        assert_eq!(b.winusb_reason, WINUSB_NEEDS_A_USB_INTERFACE);
        assert!(b.winusb_impossible_by_transport);
        assert!(b.line.contains("no USB interface to bind"), "{}", b.line);
    }

    /// Every combination must produce a non-empty line naming its transport.
    /// An empty cell in a device list is the shape that reads as "nothing to
    /// say" when it means "nobody decided".
    #[test]
    fn every_combination_yields_a_line_that_names_its_transport() {
        for transport in [Transport::Usb, Transport::Bluetooth] {
            for keyboard in [true, false] {
                for claimed in [true, false] {
                    for can_type in [true, false] {
                        let reach = Reach {
                            transport,
                            keyboard,
                            claimed,
                            can_type,
                        };
                        let e = reach.eligibility();
                        assert!(!e.line.is_empty(), "{reach:?}");
                        assert!(e.line.contains(transport.label()), "{reach:?} → {}", e.line);
                        assert_eq!(
                            e.winusb_reason.is_empty(),
                            e.winusb,
                            "a refused backend must carry its reason: {reach:?}"
                        );
                    }
                }
            }
        }
    }
}
