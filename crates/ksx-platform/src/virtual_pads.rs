//! Virtual pads ViGEmBus is exposing *right now* — the ghost-pad check behind
//! `ksx doctor`. Plain serializable data, collected on Windows by
//! [`crate::collect`], constructible anywhere so classification and JSON shape
//! are testable off-cabinet.
//!
//! A pad that survives its creator (killed process, wedged session) is a
//! *ghost*: it sits in joy.cpl, holds an XInput slot, and confuses games. The
//! bus only removes a child pad when the owning client handle closes, so a
//! `taskkill /f` at the wrong moment leaves one behind until the bus device is
//! restarted or the machine reboots.
//!
//! Classification is reachable only through
//! [`VirtualPadReport::from_bus_children`], whose input is the child list of
//! the ViGEmBus devnode. That is the containment that makes the VID/PID match
//! safe: a REAL DualShock or Xbox pad plugged into the cabinet carries the
//! very same ids, but it hangs off a USB hub devnode, never off ViGEmBus — so
//! it cannot reach the classifier at all, and nothing in this module (or in
//! the collector feeding it) matches pad VID/PIDs system-wide.

use serde::Serialize;

use crate::process::ProcessEntry;

/// What a bus child's VID/PID says it is pretending to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonaGuess {
    Xbox360,
    /// Renamed to match `ksx_core::Persona::as_str` — one spelling of
    /// "playstation" across every `--json` surface.
    #[serde(rename = "playstation")]
    PlayStation,
    /// Ids stay visible for unknowns ([`VirtualPadInfo::hardware_id`]) so a new
    /// ViGEm persona shows up as evidence, not as a silent miscount.
    Unknown,
}

impl PersonaGuess {
    /// Human name for doctor's report lines.
    pub const fn label(self) -> &'static str {
        match self {
            PersonaGuess::Xbox360 => "Xbox 360 pad",
            PersonaGuess::PlayStation => "PlayStation (DS4) pad",
            PersonaGuess::Unknown => "unknown pad",
        }
    }
}

/// One child pad of the ViGEmBus devnode.
#[derive(Debug, Clone, Serialize)]
pub struct VirtualPadInfo {
    /// Full device instance id, e.g. `USB\VID_045E&PID_028E\2&2B7A94F5&0&01`.
    pub instance_id: String,
    /// The id the guess was made from (`USB\VID_045E&PID_028E`), reported so an
    /// `unknown` row is still actionable.
    pub hardware_id: String,
    pub persona_guess: PersonaGuess,
}

/// A live process that could legitimately own the pads. Heuristic by nature —
/// a third-party ViGEm feeder is invisible to it — which is why the ghost
/// verdict says "no *known* splitter", never "no owner exists".
#[derive(Debug, Clone, Serialize)]
pub struct OwnerProcess {
    pub pid: u32,
    pub name: String,
}

/// The bus's current children, classified. All fields best-effort in the
/// [`crate::report::DriverReport`] tradition: no bus devnode yields an empty
/// report, never an error.
#[derive(Debug, Clone, Serialize)]
pub struct VirtualPadReport {
    /// The ViGEmBus devnode the children were read from — also the argument to
    /// the fix (`pnputil /restart-device "<id>"`). `None` when the bus has no
    /// *present* devnode (never installed, or a registered leftover of an
    /// uninstalled/stopped bus), in which case there are no children either.
    pub bus_instance_id: Option<String>,
    /// Always `pads.len()`; spelled out so `--json` consumers can key on the
    /// count without walking the array.
    pub count: usize,
    pub pads: Vec<VirtualPadInfo>,
    /// Known splitter processes alive right now ([`owner_candidates`]).
    pub owners: Vec<OwnerProcess>,
}

impl VirtualPadReport {
    /// Build the report from the ViGEmBus devnode's children — the only
    /// constructor, so a device id can only be classified after the collector
    /// has already proven it hangs off the bus.
    pub fn from_bus_children<I, S>(
        bus_instance_id: Option<String>,
        children: I,
        owners: Vec<OwnerProcess>,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let pads: Vec<VirtualPadInfo> = children
            .into_iter()
            .map(|id| {
                let id = id.into();
                VirtualPadInfo {
                    hardware_id: hardware_id_of(&id),
                    persona_guess: classify(&id),
                    instance_id: id,
                }
            })
            .collect();
        Self {
            bus_instance_id,
            count: pads.len(),
            pads,
            owners,
        }
    }

    /// No bus devnode, no pads — the shape a machine without ViGEmBus reports,
    /// and the fixture baseline for tests.
    pub fn empty() -> Self {
        Self::from_bus_children(None, std::iter::empty::<String>(), Vec::new())
    }

    /// Pads with nothing known to own them. Not proof — see [`OwnerProcess`] —
    /// but the strongest signal a one-shot diagnostic can have.
    pub fn is_ghost_suspect(&self) -> bool {
        self.count > 0 && self.owners.is_empty()
    }
}

/// Process names that legitimately plug ViGEm pads on this machine: ksx itself
/// (a `ksx run`/daemon/`ksx pads` in another shell) and the legacy C# splitter.
pub const SPLITTER_PROCESS_NAMES: &[&str] = &["ksx.exe", "KeyboardSplitter.exe"];

/// Filter a process snapshot down to possible pad owners. `self_pid` is
/// excluded because doctor itself is `ksx.exe` and owns nothing.
pub fn owner_candidates(processes: &[ProcessEntry], self_pid: u32) -> Vec<OwnerProcess> {
    processes
        .iter()
        .filter(|p| p.pid != self_pid)
        .filter(|p| SPLITTER_PROCESS_NAMES.iter().any(|n| p.name_matches(n)))
        .map(|p| OwnerProcess {
            pid: p.pid,
            name: p.name.clone(),
        })
        .collect()
}

/// The exact ids ViGEmBus gives its child PDOs (`BusQueryDeviceID` in the bus
/// source): the emulated pad's VID/PID, not Nefarius's own.
const X360_VID_PID: (u32, u32) = (0x045E, 0x028E);
const DS4_VID_PID: (u32, u32) = (0x054C, 0x05C4);

/// Private on purpose: the only route here is [`VirtualPadReport::from_bus_children`],
/// so a system-wide id can never be classified by accident.
fn classify(child_id: &str) -> PersonaGuess {
    match (
        crate::winusb::hex_after(child_id, "VID_"),
        crate::winusb::hex_after(child_id, "PID_"),
    ) {
        (Some(vid), Some(pid)) if (vid, pid) == X360_VID_PID => PersonaGuess::Xbox360,
        (Some(vid), Some(pid)) if (vid, pid) == DS4_VID_PID => PersonaGuess::PlayStation,
        _ => PersonaGuess::Unknown,
    }
}

/// `USB\VID_xxxx&PID_xxxx` from a full instance id — enumerator + device key,
/// without the per-instance tail.
fn hardware_id_of(instance_id: &str) -> String {
    let mut parts = instance_id.splitn(3, '\\');
    match (parts.next(), parts.next()) {
        (Some(enumerator), Some(device_key)) => {
            format!("{enumerator}\\{device_key}").to_uppercase()
        }
        _ => instance_id.to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GHOST_X360: &str = "USB\\VID_045E&PID_028E\\2&2B7A94F5&0&01";
    const GHOST_DS4: &str = "USB\\VID_054C&PID_05C4\\2&2B7A94F5&0&02";

    fn report(children: &[&str]) -> VirtualPadReport {
        VirtualPadReport::from_bus_children(
            Some("ROOT\\SYSTEM\\0002".into()),
            children.iter().copied(),
            Vec::new(),
        )
    }

    #[test]
    fn classification_table() {
        let cases = [
            (GHOST_X360, PersonaGuess::Xbox360),
            (GHOST_DS4, PersonaGuess::PlayStation),
            // Case-insensitive: registry casing varies by writer.
            ("usb\\vid_045e&pid_028e\\2&aa&0&01", PersonaGuess::Xbox360),
            // Real-controller ids that are NOT ViGEm personas classify as
            // unknown *if* they ever arrive as bus children (a DualSense, an
            // Xbox One pad) — they are never miscounted as a known persona.
            ("USB\\VID_054C&PID_0CE6\\3&AA&0&1", PersonaGuess::Unknown),
            ("USB\\VID_045E&PID_02D1\\3&AA&0&2", PersonaGuess::Unknown),
            // Malformed / id without VID&PID.
            ("HID\\NOPE\\1", PersonaGuess::Unknown),
        ];
        for (id, guess) in cases {
            let r = report(&[id]);
            assert_eq!(r.pads[0].persona_guess, guess, "{id}");
        }
    }

    #[test]
    fn unknown_pads_still_carry_an_actionable_hardware_id() {
        let r = report(&["USB\\VID_054C&PID_0CE6\\3&AA&0&1", "HID\\NOPE\\1"]);
        assert_eq!(r.pads[0].hardware_id, "USB\\VID_054C&PID_0CE6");
        assert_eq!(r.pads[1].hardware_id, "HID\\NOPE");
    }

    /// The live collector walks `CM_Get_Child`/`CM_Get_Sibling` from the
    /// *ViGEmBus devnode*, so the only ids that can reach the classifier are
    /// ids whose PnP parent is the bus. Model that contract with a synthetic
    /// (parent, child) tree: a REAL DualShock — same VID/PID as the ghost
    /// persona — hangs off a USB root hub and must never appear in the report.
    #[test]
    fn a_real_pad_under_a_usb_hub_never_reaches_the_classifier() {
        const BUS: &str = "ROOT\\SYSTEM\\0002";
        let tree = [
            // A real DS4 on a physical port: parent is a hub, not the bus.
            (
                "USB\\ROOT_HUB30\\5&AAAA&0&0",
                "USB\\VID_054C&PID_05C4\\9&REAL&0&1",
            ),
            // A real Xbox pad, likewise.
            (
                "USB\\ROOT_HUB30\\5&AAAA&0&0",
                "USB\\VID_045E&PID_028E\\9&REAL&0&2",
            ),
            // The ghost: a child of the ViGEmBus devnode.
            (BUS, GHOST_X360),
        ];
        let children: Vec<&str> = tree
            .iter()
            .filter(|(parent, _)| *parent == BUS)
            .map(|(_, child)| *child)
            .collect();
        let r = VirtualPadReport::from_bus_children(Some(BUS.into()), children, Vec::new());
        assert_eq!(r.count, 1);
        assert!(r.pads.iter().all(|p| !p.instance_id.contains("REAL")));
        assert_eq!(r.pads[0].persona_guess, PersonaGuess::Xbox360);
    }

    #[test]
    fn ghost_suspicion_requires_pads_and_no_owner() {
        assert!(!VirtualPadReport::empty().is_ghost_suspect());
        assert!(report(&[GHOST_X360]).is_ghost_suspect());
        let owned = VirtualPadReport::from_bus_children(
            Some("ROOT\\SYSTEM\\0002".into()),
            [GHOST_X360],
            vec![OwnerProcess {
                pid: 4242,
                name: "ksx.exe".into(),
            }],
        );
        assert!(!owned.is_ghost_suspect());
    }

    #[test]
    fn owner_candidates_match_splitters_case_insensitively_excluding_self() {
        let procs = [
            ProcessEntry {
                pid: 100,
                parent_pid: 1,
                name: "KSX.EXE".into(),
            },
            ProcessEntry {
                pid: 200,
                parent_pid: 1,
                name: "keyboardsplitter.exe".into(),
            },
            ProcessEntry {
                pid: 300,
                parent_pid: 1,
                name: "mame.exe".into(),
            },
            // Doctor itself.
            ProcessEntry {
                pid: 400,
                parent_pid: 1,
                name: "ksx.exe".into(),
            },
        ];
        let owners = owner_candidates(&procs, 400);
        let pids: Vec<u32> = owners.iter().map(|o| o.pid).collect();
        assert_eq!(pids, vec![100, 200]);
    }

    #[test]
    fn report_serializes_to_stable_json() {
        let r = report(&[GHOST_X360, GHOST_DS4]);
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(
            v.pointer("/bus_instance_id"),
            Some(&serde_json::json!("ROOT\\SYSTEM\\0002"))
        );
        assert_eq!(v.pointer("/count"), Some(&serde_json::json!(2)));
        assert_eq!(
            v.pointer("/pads/0/persona_guess"),
            Some(&serde_json::json!("xbox360"))
        );
        assert_eq!(
            v.pointer("/pads/1/persona_guess"),
            Some(&serde_json::json!("playstation"))
        );
        assert_eq!(
            v.pointer("/pads/0/hardware_id"),
            Some(&serde_json::json!("USB\\VID_045E&PID_028E"))
        );
        assert_eq!(v.pointer("/owners"), Some(&serde_json::json!([])));
    }
}
