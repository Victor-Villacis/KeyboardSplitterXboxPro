//! [`DeviceSelector`] — what a `[[device]] id` MEANS, as opposed to what one
//! particular devnode happened to be called last Tuesday.
//!
//! # The defect this exists to remove
//!
//! A `[[device]]` entry used to hold a raw Windows device instance path:
//!
//! ```text
//! id = 'USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000'
//! ```
//!
//! Everything after the last `\` is `usbccgp`'s `ParentIdPrefix` plus the
//! interface number, and `ParentIdPrefix` is derived from **which physical USB
//! socket the board is in**. Move the board one port over and that string
//! changes. Nothing errors, nothing warns: the config now names a devnode that
//! does not exist, the plan finds no candidate, and the panel is dead.
//!
//! Identity that breaks when you unplug and replug into a different socket is
//! not identity. It is a cache key.
//!
//! # What replaces it
//!
//! A selector says *which board*, in the weakest terms that are still unique
//! (`docs/DEVICE-IDENTITY.md`). Three rungs, strongest-surviving first:
//!
//! | rung | spelling | survives a replug | tells twins apart |
//! |---|---|---|---|
//! | serial | `usb:d209:0430:00:sn=4` | yes | only if the firmware serials differ |
//! | model | `usb:d209:0430:00` | yes | **no** — refuses while twins are present |
//! | port | `usb:d209:0430:00:port=7&25EEA38C&0&0000` | no | yes, always |
//!
//! Two identical boards MUST stay tellable apart — that is the whole reason M6
//! beats Interception, and this module does not regress it: a selector that
//! matches more than one connected interface never silently picks one. It
//! reports [`Match::Ambiguous`] with every candidate, and the caller refuses.
//!
//! # On this hardware, honestly
//!
//! The I-PAC 4X *does* report a `iSerialNumber` string — and it is `"4"`. The
//! Ultimarc trackball next to it reports `"6"`. Those are one-character
//! constants that look like model or board indices, not per-unit serials, so
//! **two I-PAC 4X boards very probably both answer `"4"`**. A serial is
//! therefore treated as a *hint that is verified at match time*, never as a
//! promise: [`DeviceSelector::match_against`] compares it against every
//! connected interface and, if two answer, says so rather than guessing.
//!
//! The honest fallback when the serials do collide is the port rung, and the
//! selection flow writes it automatically in exactly that case — with the
//! trade stated out loud, because a port-pinned board is one that stops
//! working when it is moved and the user has to know which of their boards
//! that applies to.
//!
//! # Legacy spellings still resolve
//!
//! A raw instance path and an Interception hardware id are both still valid
//! `id` values ([`DeviceSelector::InstancePath`] and
//! [`DeviceSelector::HardwareId`]). Every config that works today keeps
//! working; what changes is that a *newly written* entry uses the strongest
//! surviving rung, and that `ksx device scan` prints the upgrade.

use core::fmt;

use crate::DeviceId;

/// The facts about one connected interface that a selector may match on.
///
/// Deliberately a plain data struct with no platform types: the matcher is pure
/// and CI-testable, and the Windows enumerator fills this in
/// (`ksx_capture::UsbCandidate::facts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceFacts {
    /// The concrete, right-now device instance path. This stays the runtime
    /// [`DeviceId`] — a selector *finds* one of these, it does not replace it.
    pub id: DeviceId,
    pub vendor_id: u16,
    pub product_id: u16,
    /// `bInterfaceNumber` — `MI_00` on the I-PAC's keyboard interface.
    pub interface_number: u8,
    /// `iSerialNumber`, as the device reports it. `None` when the descriptor
    /// index is 0 (most cheap HID boards) — which is the common case, and the
    /// reason the model rung exists.
    pub serial: Option<String>,
    /// The instance tail: everything after the last `\` of [`Self::id`]. This
    /// is the port-derived part, kept separate so a port-pinned selector never
    /// has to re-parse a path.
    pub instance: String,
}

impl DeviceFacts {
    /// The instance tail of an instance path: everything after the last `\`.
    /// A path with no `\` is its own tail.
    pub fn instance_of(id: &str) -> &str {
        match id.rfind('\\') {
            Some(i) => &id[i + 1..],
            None => id,
        }
    }
}

/// Which board a `[[device]] id` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceSelector {
    /// `usb:<vid>:<pid>:<mi>[:sn=<serial>][:port=<instance>]` — the form ksx
    /// writes. Vendor/product/interface always; at most one qualifier.
    Usb {
        vendor_id: u16,
        product_id: u16,
        interface_number: u8,
        qualifier: Qualifier,
    },
    /// A raw device instance path, matched byte-exactly and case-insensitively
    /// (`USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000`). Every config written
    /// before this module existed holds one of these.
    InstancePath(String),
    /// An Interception hardware id (`HID\VID_D209&PID_0430&REV_0056&MI_00`).
    /// Never matches a USB interface — which is exactly what makes the
    /// half-migrated config diagnosable instead of merely broken.
    HardwareId(String),
}

/// How much a [`DeviceSelector::Usb`] pins down beyond vendor/product/interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Qualifier {
    /// Nothing. Unique while exactly one such board is connected; ambiguous the
    /// moment a second one is, and reported as such rather than guessed.
    Model,
    /// `sn=` — the device's own `iSerialNumber`.
    Serial(String),
    /// `port=` — the instance tail, i.e. the physical socket. The strongest
    /// discriminator and the only one that does NOT survive a replug.
    Port(String),
}

impl Qualifier {
    /// Does this qualifier survive being unplugged and plugged in elsewhere?
    pub fn survives_replug(&self) -> bool {
        !matches!(self, Qualifier::Port(_))
    }

    /// The word a UI prints for the rung.
    pub fn rung(&self) -> &'static str {
        match self {
            Qualifier::Model => "model",
            Qualifier::Serial(_) => "serial",
            Qualifier::Port(_) => "port",
        }
    }
}

/// What a selector found in one enumeration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Match<'a> {
    /// Exactly one connected interface answers. The only outcome that may be
    /// captured.
    One(&'a DeviceFacts),
    /// Nothing answers — unplugged, or not rebound, or the config names a board
    /// that was never here.
    None,
    /// Two or more answer. **Never resolved by picking one.** Two identical
    /// boards driving one slot is the exact ambiguity M6 exists to remove, and
    /// silently choosing would reintroduce it with a friendlier face.
    Ambiguous(Vec<&'a DeviceFacts>),
}

impl Match<'_> {
    pub fn is_one(&self) -> bool {
        matches!(self, Match::One(_))
    }

    /// The resolved id, or `None` for both failure shapes.
    pub fn id(&self) -> Option<&DeviceId> {
        match self {
            Match::One(facts) => Some(&facts.id),
            _ => None,
        }
    }
}

/// Why a selector string could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectorParseError {
    pub input: String,
    pub problem: &'static str,
}

impl fmt::Display for SelectorParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "\"{}\" is not a device selector: {} (expected usb:<vid>:<pid>:<mi>[:sn=… | :port=…], \
             a USB instance path, or an Interception hardware id)",
            self.input, self.problem
        )
    }
}

impl std::error::Error for SelectorParseError {}

impl DeviceSelector {
    /// Parse a `[[device]] id` value.
    ///
    /// Order of recognition matters and is not arbitrary: the `usb:` prefix is
    /// checked first because it is the only spelling ksx *writes*, then the two
    /// legacy path shapes are told apart by their enumerator prefix.
    ///
    /// # Any other path is an opaque id, not an error
    ///
    /// It is tempting to reject an unrecognised prefix — a config that names
    /// nothing should say so at load, not at 2 a.m. when the panel is dead. But
    /// `USB\` and `HID\` are not the only enumerators a keyboard can live
    /// under, and ksx writes the others *itself*: the setup wizard commits
    /// whatever Raw Input reports, and for a laptop or PS/2 keyboard that is
    ///
    /// ```text
    /// \\.\ACPI#PNP0303#4&1a2b3c4d&0   ->   ACPI\PNP0303\4&1A2B3C4D&0
    /// ```
    ///
    /// So being strict here would refuse to load a config **ksx's own wizard
    /// wrote**, for an entirely ordinary keyboard, with an error about an
    /// unrecognised prefix for a value the user never typed.
    ///
    /// `ConfigFile::resolve_device` already states the right contract — *a
    /// literal instance path (contains `\`) passes through unchanged* — and
    /// this matches it. An unknown prefix with a backslash is an opaque id with
    /// [`Self::HardwareId`] semantics: byte-exact, case-insensitive, and never
    /// matching a USB interface, so a half-migrated config stays diagnosable.
    ///
    /// Only a spelling with no backslash *and* no known prefix is a parse
    /// error — that is a typo, not a device.
    pub fn parse(input: &str) -> Result<Self, SelectorParseError> {
        let trimmed = input.trim();
        let bad = |problem: &'static str| SelectorParseError {
            input: input.to_owned(),
            problem,
        };
        if trimmed.is_empty() {
            return Err(bad("it is empty"));
        }
        if let Some(rest) = strip_prefix_ci(trimmed, "usb:") {
            return Self::parse_usb(input, rest);
        }
        // Legacy: a real path has an enumerator, a `\`, and a hardware id.
        if starts_with_ci(trimmed, "USB\\") {
            return Ok(Self::InstancePath(trimmed.to_uppercase()));
        }
        if starts_with_ci(trimmed, "HID\\") {
            return Ok(Self::HardwareId(trimmed.to_uppercase()));
        }
        if trimmed.contains('\\') {
            // ACPI\…, ROOT\…, BTHENUM\… — an enumerator ksx does not model,
            // carrying a device it must still be able to name.
            return Ok(Self::HardwareId(trimmed.to_uppercase()));
        }
        Err(bad(
            "it has no recognised prefix and is not a device path (no `\\`)",
        ))
    }

    fn parse_usb(original: &str, rest: &str) -> Result<Self, SelectorParseError> {
        let bad = |problem: &'static str| SelectorParseError {
            input: original.to_owned(),
            problem,
        };
        // Split into at most four fields: a qualifier value may itself contain
        // `&` and `=` but never `:`, which keeps this a plain split.
        let mut parts = rest.split(':');
        let vendor_id = parts
            .next()
            .and_then(parse_hex16)
            .ok_or_else(|| bad("the vendor id is not four hex digits"))?;
        let product_id = parts
            .next()
            .and_then(parse_hex16)
            .ok_or_else(|| bad("the product id is not four hex digits"))?;
        let interface_number = parts
            .next()
            .and_then(parse_hex8)
            .ok_or_else(|| bad("the interface number is not two hex digits"))?;
        let qualifier = match parts.next() {
            None => Qualifier::Model,
            Some("") => return Err(bad("the qualifier after the last `:` is empty")),
            Some(q) => match (strip_prefix_ci(q, "sn="), strip_prefix_ci(q, "port=")) {
                (Some(sn), _) if !sn.is_empty() => Qualifier::Serial(sn.to_owned()),
                (_, Some(port)) if !port.is_empty() => Qualifier::Port(port.to_uppercase()),
                _ => return Err(bad("the qualifier is neither `sn=…` nor `port=…`")),
            },
        };
        if parts.next().is_some() {
            return Err(bad("it has more than one qualifier"));
        }
        Ok(Self::Usb {
            vendor_id,
            product_id,
            interface_number,
            qualifier,
        })
    }

    /// The strongest selector that survives a replug for `facts`, given
    /// everything else that is connected.
    ///
    /// This is the function the selection flow writes with, and the rule it
    /// encodes is the whole design in three lines:
    ///
    /// 1. try the model rung — the weakest thing that could be unique;
    /// 2. if a twin is connected, add the serial, but only if the twin's serial
    ///    actually differs (on this hardware it usually does not);
    /// 3. only if that still does not separate them, pin the port — and the
    ///    caller is expected to say out loud that this board is now
    ///    port-specific.
    ///
    /// Never returns a port selector when a weaker one is already unique, which
    /// is what stops a working config from silently acquiring the very
    /// fragility this module exists to remove.
    pub fn strongest_for(facts: &DeviceFacts, connected: &[DeviceFacts]) -> Self {
        let model = |q: Qualifier| Self::Usb {
            vendor_id: facts.vendor_id,
            product_id: facts.product_id,
            interface_number: facts.interface_number,
            qualifier: q,
        };
        let same_model = |other: &&DeviceFacts| {
            other.vendor_id == facts.vendor_id
                && other.product_id == facts.product_id
                && other.interface_number == facts.interface_number
        };
        let twins: Vec<&DeviceFacts> = connected.iter().filter(same_model).collect();
        if twins.len() <= 1 {
            return model(Qualifier::Model);
        }
        if let Some(serial) = &facts.serial {
            let sharing = twins
                .iter()
                .filter(|other| other.serial.as_deref() == Some(serial.as_str()))
                .count();
            if sharing == 1 {
                return model(Qualifier::Serial(serial.clone()));
            }
        }
        model(Qualifier::Port(facts.instance.to_uppercase()))
    }

    /// Every connected interface this selector names.
    ///
    /// Returning all matches rather than the first is the load-bearing choice:
    /// [`Match::Ambiguous`] is a *result*, not an error path bolted on, so no
    /// caller can accidentally take one of two identical boards.
    pub fn match_against<'a>(&self, connected: &'a [DeviceFacts]) -> Match<'a> {
        let hits: Vec<&'a DeviceFacts> = connected.iter().filter(|f| self.matches(f)).collect();
        match hits.len() {
            0 => Match::None,
            1 => Match::One(hits[0]),
            _ => Match::Ambiguous(hits),
        }
    }

    /// Does one interface answer to this selector?
    pub fn matches(&self, facts: &DeviceFacts) -> bool {
        match self {
            Self::Usb {
                vendor_id,
                product_id,
                interface_number,
                qualifier,
            } => {
                if facts.vendor_id != *vendor_id
                    || facts.product_id != *product_id
                    || facts.interface_number != *interface_number
                {
                    return false;
                }
                match qualifier {
                    Qualifier::Model => true,
                    Qualifier::Serial(sn) => facts.serial.as_deref() == Some(sn.as_str()),
                    Qualifier::Port(port) => facts.instance.eq_ignore_ascii_case(port),
                }
            }
            // Case-insensitive on purpose: the registry hands these back in
            // mixed case and a config built from one command's output must
            // still match another's. `ksx device scan` prints the canonical
            // uppercase form so the two never *look* different either.
            Self::InstancePath(path) => facts.id.as_str().eq_ignore_ascii_case(path),
            // An Interception hardware id names a devnode on the keyboard
            // stack. A claimed USB interface is not one, and pretending
            // otherwise is what turned a half-finished migration into a dead
            // panel with nothing in the log.
            Self::HardwareId(_) => false,
        }
    }

    /// Does this selector still name the same board after a replug into a
    /// different socket?
    pub fn survives_replug(&self) -> bool {
        match self {
            Self::Usb { qualifier, .. } => qualifier.survives_replug(),
            // The instance path IS the port. That is the bug.
            Self::InstancePath(_) => false,
            // Interception hardware ids have no instance component at all, so
            // they survive a replug — and pay for it by being unable to tell
            // two identical boards apart at all. That trade is the reason M6
            // exists.
            Self::HardwareId(_) => true,
        }
    }

    /// One word for the rung, for a table column.
    pub fn rung(&self) -> &'static str {
        match self {
            Self::Usb { qualifier, .. } => qualifier.rung(),
            Self::InstancePath(_) => "port",
            Self::HardwareId(_) => "hardware-id",
        }
    }

    /// The sentence a surface prints under a device row: what this selector
    /// pins down and what it costs.
    pub fn explain(&self) -> String {
        match self {
            Self::Usb { qualifier, .. } => match qualifier {
                Qualifier::Model => {
                    "any board of this model — unique while only one is plugged in, \
                     and survives being moved to another USB port"
                        .to_owned()
                }
                Qualifier::Serial(sn) => format!(
                    "the board whose serial is \"{sn}\" — survives being moved to another USB port"
                ),
                Qualifier::Port(port) => format!(
                    "the board in one specific USB socket ({port}) — MOVING IT TO ANOTHER PORT \
                     BREAKS THIS ENTRY, which is the price of telling two identical boards apart"
                ),
            },
            Self::InstancePath(_) => "one specific USB socket (a legacy full instance path) — \
                 MOVING THE BOARD TO ANOTHER PORT BREAKS THIS ENTRY; `ksx device scan` prints the \
                 replug-proof replacement"
                .to_owned(),
            Self::HardwareId(_) => {
                "an Interception hardware id — it names a model, not a board, so \
                 two identical boards are indistinguishable, and it never matches a WinUSB-claimed \
                 interface"
                    .to_owned()
            }
        }
    }
}

impl fmt::Display for DeviceSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usb {
                vendor_id,
                product_id,
                interface_number,
                qualifier,
            } => {
                write!(
                    f,
                    "usb:{vendor_id:04x}:{product_id:04x}:{interface_number:02x}"
                )?;
                match qualifier {
                    Qualifier::Model => Ok(()),
                    Qualifier::Serial(sn) => write!(f, ":sn={sn}"),
                    Qualifier::Port(port) => write!(f, ":port={port}"),
                }
            }
            Self::InstancePath(path) | Self::HardwareId(path) => f.write_str(path),
        }
    }
}

fn starts_with_ci(haystack: &str, prefix: &str) -> bool {
    haystack.len() >= prefix.len() && haystack[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn strip_prefix_ci<'a>(haystack: &'a str, prefix: &str) -> Option<&'a str> {
    starts_with_ci(haystack, prefix).then(|| &haystack[prefix.len()..])
}

fn parse_hex16(text: &str) -> Option<u16> {
    (!text.is_empty() && text.len() <= 4)
        .then(|| u16::from_str_radix(text, 16).ok())
        .flatten()
}

fn parse_hex8(text: &str) -> Option<u8> {
    (!text.is_empty() && text.len() <= 2)
        .then(|| u8::from_str_radix(text, 16).ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cabinet's board, in the socket it is in tonight.
    fn ipac(instance: &str, serial: Option<&str>) -> DeviceFacts {
        DeviceFacts {
            id: DeviceId::new(format!("USB\\VID_D209&PID_0430&MI_00\\{instance}")),
            vendor_id: 0xD209,
            product_id: 0x0430,
            interface_number: 0,
            serial: serial.map(str::to_owned),
            instance: instance.to_owned(),
        }
    }

    fn sel(text: &str) -> DeviceSelector {
        DeviceSelector::parse(text).expect(text)
    }

    #[test]
    fn the_three_rungs_round_trip_through_their_text() {
        for text in [
            "usb:d209:0430:00",
            "usb:d209:0430:00:sn=4",
            "usb:d209:0430:00:port=7&25EEA38C&0&0000",
        ] {
            assert_eq!(sel(text).to_string(), text, "{text}");
        }
        // Case folds on the way in; the canonical spelling is what comes out,
        // so two commands never print one board two ways.
        assert_eq!(sel("USB:D209:0430:00").to_string(), "usb:d209:0430:00");
        assert_eq!(
            sel("usb:d209:0430:00:PORT=7&25eea38c&0&0000").to_string(),
            "usb:d209:0430:00:port=7&25EEA38C&0&0000"
        );
    }

    #[test]
    fn legacy_spellings_still_parse_and_are_told_apart() {
        let path = sel(r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000");
        assert!(matches!(path, DeviceSelector::InstancePath(_)));
        assert_eq!(path.rung(), "port");
        assert!(!path.survives_replug());

        let hwid = sel(r"HID\VID_D209&PID_0430&REV_0056&MI_00");
        assert!(matches!(hwid, DeviceSelector::HardwareId(_)));
        assert_eq!(hwid.rung(), "hardware-id");
    }

    #[test]
    fn nonsense_is_refused_rather_than_becoming_a_literal_that_never_matches() {
        for text in [
            "",
            "   ",
            "ipac",
            "usb:",
            "usb:zzzz:0430:00",
            "usb:d209:0430",
            "usb:d209:0430:00:",
            "usb:d209:0430:00:serial=4",
            "usb:d209:0430:00:sn=",
            "usb:d209:0430:00:sn=4:port=x",
        ] {
            assert!(
                DeviceSelector::parse(text).is_err(),
                "{text:?} must not parse"
            );
        }
        let err = DeviceSelector::parse("ipac").unwrap_err();
        assert!(err.to_string().contains("usb:<vid>:<pid>:<mi>"), "{err}");
    }

    /// THE BUG, stated as a test. The user's real config held the port-specific
    /// path; the board moves one socket over and it names nothing.
    #[test]
    fn a_full_instance_path_stops_matching_when_the_board_changes_port() {
        let before = ipac("7&25EEA38C&0&0000", Some("4"));
        let after = ipac("6&1B2C3D4E&0&0000", Some("4"));

        let pinned = sel(r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000");
        assert!(pinned.match_against(std::slice::from_ref(&before)).is_one());
        assert_eq!(
            pinned.match_against(std::slice::from_ref(&after)),
            Match::None,
            "this is the silent cabinet death the selector exists to prevent"
        );

        // The model rung names the same board in either socket.
        let stable = sel("usb:d209:0430:00");
        assert!(stable.match_against(&[before]).is_one());
        assert!(stable.match_against(&[after]).is_one());
        assert!(stable.survives_replug());
    }

    /// T4 must not regress: two identical boards stay tellable apart.
    #[test]
    fn two_identical_boards_are_never_silently_collapsed_into_one() {
        let a = ipac("7&25EEA38C&0&0000", Some("4"));
        let b = ipac("6&1B2C3D4E&0&0000", Some("4"));
        let both = vec![a.clone(), b.clone()];

        // The weak rung sees both, and says so. It does NOT pick one.
        match sel("usb:d209:0430:00").match_against(&both) {
            Match::Ambiguous(hits) => assert_eq!(hits.len(), 2),
            other => panic!("model rung must report ambiguity, got {other:?}"),
        }
        // Their firmware serials are identical too (measured: the I-PAC 4X
        // reports "4"), so the serial rung is no help either — and admits it.
        assert!(matches!(
            sel("usb:d209:0430:00:sn=4").match_against(&both),
            Match::Ambiguous(_)
        ));
        // The port rung separates them, always.
        assert_eq!(
            sel("usb:d209:0430:00:port=7&25EEA38C&0&0000")
                .match_against(&both)
                .id(),
            Some(&a.id)
        );
        assert_eq!(
            sel("usb:d209:0430:00:port=6&1B2C3D4E&0&0000")
                .match_against(&both)
                .id(),
            Some(&b.id)
        );
    }

    /// The rule the selection flow writes by: the weakest thing that is still
    /// unique, and a port pin only when nothing weaker separates the boards.
    #[test]
    fn strongest_for_climbs_only_as_far_as_it_must() {
        let a = ipac("7&25EEA38C&0&0000", Some("4"));
        let b = ipac("6&1B2C3D4E&0&0000", Some("4"));
        let distinct = ipac("6&1B2C3D4E&0&0000", Some("SEVEN"));

        // Alone: the model rung, which survives a replug.
        let solo = DeviceSelector::strongest_for(&a, std::slice::from_ref(&a));
        assert_eq!(solo.to_string(), "usb:d209:0430:00");
        assert!(solo.survives_replug());

        // A twin with a DIFFERENT serial: the serial rung still survives a
        // replug, so nothing is pinned.
        let by_serial = DeviceSelector::strongest_for(&a, &[a.clone(), distinct.clone()]);
        assert_eq!(by_serial.to_string(), "usb:d209:0430:00:sn=4");
        assert!(by_serial.survives_replug());
        assert!(by_serial.match_against(&[a.clone(), distinct]).is_one());

        // A twin with the SAME serial — this hardware's real case: the port pin
        // is the honest fallback, and it announces the cost.
        let pinned = DeviceSelector::strongest_for(&a, &[a.clone(), b.clone()]);
        assert_eq!(
            pinned.to_string(),
            "usb:d209:0430:00:port=7&25EEA38C&0&0000"
        );
        assert!(!pinned.survives_replug());
        assert!(pinned.explain().contains("BREAKS THIS ENTRY"));
        assert!(pinned.match_against(&[a, b]).is_one());
    }

    /// A board with no serial at all (the common cheap-HID case) must not
    /// produce a `sn=` selector with an empty value.
    #[test]
    fn a_board_with_no_serial_falls_straight_to_the_port_rung() {
        let a = ipac("7&25EEA38C&0&0000", None);
        let b = ipac("6&1B2C3D4E&0&0000", None);
        let chosen = DeviceSelector::strongest_for(&a, &[a.clone(), b]);
        assert_eq!(chosen.rung(), "port");
        assert!(!chosen.to_string().contains("sn="));
    }

    /// Different interfaces of ONE board are different selectors. The I-PAC
    /// exposes MI_00/MI_01/MI_02 and only MI_00 carries the key matrix.
    #[test]
    fn the_interface_number_is_part_of_the_identity() {
        let keys = ipac("7&25EEA38C&0&0000", Some("4"));
        let mut aux = keys.clone();
        aux.interface_number = 2;
        aux.id = DeviceId::new(r"USB\VID_D209&PID_0430&MI_02\7&25EEA38C&0&0002");
        assert!(sel("usb:d209:0430:00").matches(&keys));
        assert!(!sel("usb:d209:0430:00").matches(&aux));
        assert!(sel("usb:d209:0430:02").matches(&aux));
    }

    /// An Interception hardware id can never name a claimed USB interface.
    /// That is what makes "you flipped `backend` and nothing else" a
    /// diagnosable state rather than a dead panel.
    #[test]
    fn a_hardware_id_never_matches_a_usb_interface() {
        let board = ipac("7&25EEA38C&0&0000", Some("4"));
        assert_eq!(
            sel(r"HID\VID_D209&PID_0430&REV_0056&MI_00").match_against(&[board]),
            Match::None
        );
    }

    #[test]
    fn instance_tails_are_split_off_the_end_of_a_path() {
        assert_eq!(
            DeviceFacts::instance_of(r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000"),
            "7&25EEA38C&0&0000"
        );
        assert_eq!(DeviceFacts::instance_of("no-backslash"), "no-backslash");
        assert_eq!(DeviceFacts::instance_of(r"A\"), "");
    }

    #[test]
    fn every_rung_explains_itself_in_a_sentence() {
        for text in [
            "usb:d209:0430:00",
            "usb:d209:0430:00:sn=4",
            "usb:d209:0430:00:port=7&X&0&0",
            r"USB\VID_D209&PID_0430&MI_00\7&X&0&0",
            r"HID\VID_D209&PID_0430&REV_0056&MI_00",
        ] {
            let s = sel(text);
            assert!(!s.explain().is_empty(), "{text}");
            assert!(!s.rung().is_empty(), "{text}");
        }
    }

    /// A keyboard that is not on the USB enumerator still has to load.
    ///
    /// This is the exact string ksx's own setup wizard commits for a laptop or
    /// PS/2 keyboard: `ksx_capture::rawinput` normalises the Raw Input device
    /// name `\\.\ACPI#PNP0303#4&1a2b3c4d&0` to this, and `upsert_device` writes
    /// it verbatim into `[[device]] id`. Rejecting an unrecognised prefix would
    /// mean a config ksx itself wrote refuses to load — the user's laptop
    /// keyboard breaking on upgrade, with an error about a value they never
    /// typed.
    #[test]
    fn an_acpi_keyboard_path_parses_because_ksx_is_what_wrote_it() {
        let s = sel(r"ACPI\PNP0303\4&1A2B3C4D&0");
        assert!(
            matches!(s, DeviceSelector::HardwareId(_)),
            "an unmodelled enumerator gets opaque, byte-exact semantics: {s:?}"
        );
        assert_eq!(
            s.to_string(),
            r"ACPI\PNP0303\4&1A2B3C4D&0",
            "and it must round-trip unchanged, or saving a config rewrites it"
        );
    }

    /// Leniency has a floor. A value with no backslash and no known prefix
    /// cannot name a device on Windows, so it is a typo and saying so at load
    /// beats a silently-never-matching literal at 2 a.m.
    #[test]
    fn a_bare_word_is_still_a_parse_error() {
        let err = DeviceSelector::parse("my-keyboard").expect_err("not a device");
        assert!(
            err.to_string().contains("not a device path"),
            "the error should point at what is missing: {err}"
        );
    }
}
