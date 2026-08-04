//! `ksx winusb {status,claim,release}` — the WinUSB rebind lifecycle.
//!
//! The decision surface (survey, refusals, INF text, command lines) lives in
//! `ksx_platform::winusb`, where it is pure and CI-tested against a synthetic
//! copy of this cabinet's device tree. This file is the command around it:
//! argument parsing, the dry-run/`--yes` gate, the elevation check, rendering,
//! and exit codes.
//!
//! # Why every verb is a dry run by default
//!
//! `install-drivers` runs a signed installer; the worst case is a driver you did
//! not want. `winusb claim` takes a keyboard *out of the keyboard stack*. The
//! worst case is a panel that no longer types and a user who cannot type the
//! command to undo it. So the default of every mutating verb is to print the
//! exact INF and the exact `pnputil` line and stop — the same bytes `--yes`
//! would use, so what you read is what would run.
//!
//! # Exit codes
//!
//! | 0 | reported, or the operation succeeded |
//! | 1 | unexpected error |
//! | 2 | refused: unknown/ambiguous device, not a keyboard, already claimed, elevation needed, **or the last keyboard on the machine** |
//! | 3 | pnputil ran and failed |

use ksx_platform::winusb::{self, ClaimPlan, Refusal, ReleasePlan, Survey};

/// Refused — nothing was changed. Same value as every other "cannot start".
pub const EXIT_REFUSED: i32 = 2;
/// pnputil ran and returned a failure.
pub const EXIT_APPLY_FAILED: i32 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Status,
    Claim { device: String },
    Release { device: String, force: bool },
}

pub struct Options {
    pub action: Action,
    /// Report only. Default for every mutating verb.
    pub dry_run: bool,
    /// Actually run pnputil. Required on top of `!dry_run`.
    pub yes: bool,
    pub json: bool,
}

pub fn run(opts: Options) -> anyhow::Result<()> {
    let survey = winusb::survey();
    match &opts.action {
        Action::Status => status(&survey, opts.json),
        Action::Claim { device } => claim(&survey, device, &opts),
        Action::Release { device, force } => release(&survey, device, *force, &opts),
    }
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

fn status(survey: &Survey, json: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&survey.to_json())?);
    } else {
        print!("{}", render_status(survey));
    }
    Ok(())
}

/// Rendered separately from printing so the shape is snapshot-testable.
pub fn render_status(survey: &Survey) -> String {
    use winusb::ClaimState;

    let mut out = String::new();
    out.push_str("USB interfaces ksx can reason about (read-only; nothing was opened)\n\n");
    if survey.candidates.is_empty() {
        out.push_str("  (none found)\n");
    }
    for c in &survey.candidates {
        let driver = c.interface.service.as_deref().unwrap_or("(none)");
        let verdict = match c.state {
            ClaimState::Claimed => "CLAIMED  — ksx can open this; Windows sees no keyboard",
            ClaimState::Claimable => "CLAIMABLE — ksx could claim this",
            ClaimState::NotAKeyboard => "no keys   — not a keyboard interface; ksx leaves it alone",
            ClaimState::ForeignDriver => "foreign   — another vendor's driver owns it",
        };
        out.push_str(&format!(
            "  {}\n    driver     : {driver}\n    device     : {}\n    verdict    : {verdict}\n",
            // Uppercased deliberately. This is the string a user pastes into
            // `[[device]] id`, and `ksx_capture::winusb::enumerate` canonicalizes
            // every id it produces to uppercase, while config matching
            // (`run::plan`, `capture::build`) is byte-exact. The registry hands
            // this path back in mixed case, so printing it verbatim would show
            // the same interface two different ways in two different ksx
            // commands and a config built from THIS one would not match.
            // `ksx winusb claim/release` fold case on lookup, so they still
            // accept it.
            c.interface.instance_id.to_uppercase(),
            c.interface.description(),
        ));
        if let Some(kb) = &c.keyboard {
            out.push_str(&format!("    keyboard   : {}\n", kb.instance_id));
        }
        if c.is_ultimarc() {
            out.push_str("    note       : Ultimarc (VID D209) — the arcade encoder family\n");
        }
        out.push('\n');
    }

    // Deliberately "can type right now", not "present": a claimed, disabled or
    // paired-but-disconnected keyboard is present and will not type the command
    // that undoes a claim. This number is the refusal's, and printing anything
    // else here would make the refusal look arbitrary.
    out.push_str(&format!(
        "keyboards that can type right now: {}\n",
        survey.keyboard_count()
    ));
    for kb in survey.usable_keyboards() {
        out.push_str(&format!(
            "  {}  {}\n",
            kb.node.instance_id,
            kb.node.description()
        ));
    }
    let unusable: Vec<&ksx_platform::winusb::KeyboardNode> = survey
        .keyboards
        .iter()
        .filter(|kb| !kb.is_usable())
        .collect();
    if !unusable.is_empty() {
        out.push_str("not counted (present, but cannot deliver a keystroke):\n");
        for kb in unusable {
            out.push_str(&format!(
                "  {}  {} — {}\n",
                kb.node.instance_id,
                kb.node.description(),
                kb.unusable.unwrap_or("unknown")
            ));
        }
    }
    if survey.keyboard_count() <= 1 {
        out.push_str(
            "\n[!] Only one keyboard is present. `ksx winusb claim` will REFUSE to take it: a\n\
             \x20   claimed interface is invisible to Windows, and SendInput re-injection cannot\n\
             \x20   reach the lock screen, a UAC prompt or Ctrl+Alt+Del. Plug a second keyboard\n\
             \x20   into a different port and leave it unassigned.\n",
        );
    }
    out.push_str(
        "\nA claimed panel types only while ksx is running: `ksx daemon` holds the claim for\n\
         its whole lifetime and re-injects the panel's keys whenever emulation is stopped,\n\
         including between two games. `ksx run` claims for one session only, so the panel is\n\
         dark before and after it. See docs/ARCHITECTURE.md \"M6\" and docs/RECOVERY.md \
         section 2.\n",
    );
    out
}

// ---------------------------------------------------------------------------
// claim
// ---------------------------------------------------------------------------

fn claim(survey: &Survey, device: &str, opts: &Options) -> anyhow::Result<()> {
    let dir = inf_dir()?;
    let plan = match winusb::plan_claim(survey, device, &dir) {
        Ok(plan) => plan,
        Err(refusal) => refuse(&refusal, opts.json),
    };

    let will_apply = opts.yes && !opts.dry_run;
    if will_apply && !is_elevated() {
        refuse(&Refusal::NeedsElevation, opts.json);
    }

    report_claim(&plan, opts, will_apply)?;
    if !will_apply {
        return Ok(());
    }

    match winusb::apply_claim(&plan) {
        Ok(log) => {
            for line in log {
                println!("{}", line.trim_end());
            }
            println!(
                "\nClaimed. Run `ksx winusb status` to confirm the driver is now WinUSB, then\n\
                 `ksx devices` to see it through the WinUSB backend. To undo:\n  ksx winusb \
                 release {} --yes",
                plan.instance_id
            );
            Ok(())
        }
        Err(err) => apply_failed(&err, opts.json),
    }
}

fn report_claim(plan: &ClaimPlan, opts: &Options, will_apply: bool) -> anyhow::Result<()> {
    if opts.json {
        let mut value = plan.to_json(!will_apply);
        value["will_apply"] = serde_json::json!(will_apply);
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    print!("{}", plan.render_human(!will_apply));
    if !will_apply {
        println!(
            "\nNothing was written and nothing was run. Re-run with --yes to apply.\n\
             Read docs/MIGRATION-WINUSB.md first — this changes what the device IS."
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// release
// ---------------------------------------------------------------------------

fn release(survey: &Survey, device: &str, force: bool, opts: &Options) -> anyhow::Result<()> {
    let plan = match winusb::plan_release(survey, device, force) {
        Ok(plan) => plan,
        Err(refusal) => refuse(&refusal, opts.json),
    };

    let will_apply = opts.yes && !opts.dry_run;
    if will_apply && !is_elevated() {
        refuse(&Refusal::NeedsElevation, opts.json);
    }

    report_release(&plan, opts, will_apply)?;
    if !will_apply {
        return Ok(());
    }

    match winusb::apply_release(&plan) {
        Ok(log) => {
            for line in log {
                println!("{}", line.trim_end());
            }
            println!(
                "\nReleased. The keyboard driver should be bound again — check with\n  ksx \
                 winusb status\nIf it is not, unplug and replug the board, or reboot."
            );
            Ok(())
        }
        Err(err) => apply_failed(&err, opts.json),
    }
}

fn report_release(plan: &ReleasePlan, opts: &Options, will_apply: bool) -> anyhow::Result<()> {
    if opts.json {
        let mut value = plan.to_json(!will_apply);
        value["will_apply"] = serde_json::json!(will_apply);
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    print!("{}", plan.render_human(!will_apply));
    if !will_apply {
        println!("\nNothing was run. Re-run with --yes to apply.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// shared
// ---------------------------------------------------------------------------

/// Where generated INFs live. Under the ksx config root so it survives a
/// reinstall and so `release` can find the filename it published.
fn inf_dir() -> anyhow::Result<std::path::PathBuf> {
    let root = ksx_config::ConfigRoot::discover()
        .map(|r| r.dir().to_path_buf())
        .unwrap_or_else(|_| std::env::temp_dir().join("ksx"));
    Ok(root.join("winusb"))
}

fn is_elevated() -> bool {
    // `None` (the question could not be answered) is treated as "not elevated":
    // telling a non-admin they are admin wastes a UAC-less failure, and the
    // recovery from a refused claim is one command.
    ksx_platform::process::is_elevated().unwrap_or(false)
}

fn refuse(refusal: &Refusal, json: bool) -> ! {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&refusal.to_json()).unwrap_or_default()
        );
    } else {
        eprintln!("REFUSED: {refusal}\n\n{}", refusal.advice());
    }
    std::process::exit(EXIT_REFUSED);
}

fn apply_failed(err: &winusb::ApplyError, json: bool) -> ! {
    let hint = err.hint().unwrap_or("");
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "error": { "code": "pnputil-failed", "message": err.to_string(), "hint": hint }
            }))
            .unwrap_or_default()
        );
    } else {
        eprintln!("FAILED: {err}");
        if !hint.is_empty() {
            eprintln!("\n{hint}");
        }
    }
    std::process::exit(EXIT_APPLY_FAILED);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksx_platform::winusb::DeviceNode;

    fn node(id: &str, class: &str, service: &str, desc: &str, prefix: Option<&str>) -> DeviceNode {
        DeviceNode::new(
            id,
            Some(class.to_owned()),
            Some(service.to_owned()),
            Some(desc.to_owned()),
            prefix.map(str::to_owned),
        )
    }

    const HID: &str = "{745a17a0-74d3-11d0-b6fe-00a0c90f57da}";

    fn one_keyboard_only() -> Survey {
        Survey::from_nodes(&[
            node(
                r"USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000",
                HID,
                "HidUsb",
                "@input.inf,%hid.devicedesc%;USB Input Device",
                Some("8&2a0d0500&0"),
            ),
            node(
                r"HID\VID_D209&PID_0430&MI_00\8&2a0d0500&0&0000",
                winusb::KEYBOARD_CLASS_GUID,
                "kbdhid",
                "@keyboard.inf,%hid.keyboarddevice%;HID Keyboard Device",
                None,
            ),
        ])
    }

    #[test]
    fn exit_codes_match_the_rest_of_the_cli() {
        assert_eq!(EXIT_REFUSED, crate::run::EXIT_CANNOT_START);
        assert_eq!(EXIT_REFUSED, crate::install::EXIT_REFUSED);
        assert_eq!(EXIT_APPLY_FAILED, 3);
    }

    /// `status` is the command a user runs *before* they can hurt themselves,
    /// so the one-keyboard warning has to be there and has to say why.
    #[test]
    fn status_warns_loudly_when_there_is_only_one_keyboard() {
        let text = render_status(&one_keyboard_only());
        assert!(text.contains("Only one keyboard"), "{text}");
        assert!(text.contains("REFUSE"), "{text}");
        assert!(text.contains("lock screen"), "{text}");
        assert!(text.contains("different port"), "{text}");
    }

    /// Status must print the instance path in the **canonical (uppercase)**
    /// form — it is the argument for every other verb, the string
    /// docs/RECOVERY.md tells people to copy, and the value that ends up in
    /// `[[device]] id`.
    ///
    /// The registry returns this path in mixed case. `ksx devices` and
    /// `ksx_capture::winusb::enumerate` canonicalize to uppercase, and config
    /// matching (`run::plan`, `capture::build`) is byte-exact — so printing the
    /// raw casing here would show one interface two different ways in two ksx
    /// commands, and a config built from this screen would not match the
    /// backend's id. Uppercase costs nothing because `Survey::resolve` folds
    /// case, so `ksx winusb claim` still accepts what is printed.
    #[test]
    fn status_prints_the_instance_path_the_other_verbs_take() {
        let survey = one_keyboard_only();
        let text = render_status(&survey);
        let canonical = r"USB\VID_D209&PID_0430&MI_00\7&25EEA38C&0&0000";
        assert!(text.contains(canonical), "{text}");
        assert!(
            !text.contains(r"7&25eea38c&0&0000"),
            "the non-canonical casing must not be what a user copies: {text}"
        );
        // ...and what was printed still resolves, so the copy-paste works.
        assert!(
            survey.resolve(canonical).is_ok(),
            "the canonical form must round-trip through the claim lookup"
        );
        assert!(
            text.contains(r"HID\VID_D209&PID_0430&MI_00\8&2a0d0500&0&0000"),
            "the HID keyboard child is what `ksx devices` shows: {text}"
        );
        assert!(text.contains("CLAIMABLE"), "{text}");
        assert!(text.contains("Ultimarc"), "{text}");
        // And the trade-off is stated on the screen a user actually reads.
        assert!(text.contains("only while ksx is running"), "{text}");
    }

    #[test]
    fn status_says_nothing_was_opened() {
        assert!(render_status(&Survey::default()).contains("nothing was opened"));
    }

    /// The refusal and the screen must agree about what a keyboard is.
    ///
    /// A paired-but-disconnected Bluetooth keyboard is *present* all day. If
    /// `status` counted it, the user would read "2 keyboards", claim the panel,
    /// and discover the second one was in a drawer with no batteries. So it is
    /// listed — hiding it would be its own lie — but under a heading that says
    /// it does not count, and the warning fires anyway.
    #[test]
    fn status_does_not_count_a_keyboard_that_cannot_type() {
        let bt = r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002045E_PID&0800\7&2A0B8CBA&0&001BDC0F1FE7_C00000000";
        let mut nodes = vec![
            node(
                r"USB\VID_D209&PID_0430&MI_00\7&25eea38c&0&0000",
                HID,
                "HidUsb",
                "@input.inf,%hid.devicedesc%;USB Input Device",
                Some("8&2a0d0500&0"),
            ),
            node(
                r"HID\VID_D209&PID_0430&MI_00\8&2a0d0500&0&0000",
                winusb::KEYBOARD_CLASS_GUID,
                "kbdhid",
                "@keyboard.inf,%hid.keyboarddevice%;HID Keyboard Device",
                None,
            ),
        ];
        nodes.push(
            node(
                bt,
                winusb::KEYBOARD_CLASS_GUID,
                "kbdhid",
                "@keyboard.inf,%hid.keyboarddevice%;Bluetooth Keyboard",
                None,
            )
            .with_status(winusb::NodeStatus {
                started: false,
                problem: winusb::CM_PROB_DEVICE_NOT_CONNECTED,
            }),
        );
        let survey = Survey::from_nodes(&nodes);
        let text = render_status(&survey);

        assert!(
            text.contains("keyboards that can type right now: 1"),
            "{text}"
        );
        assert!(text.contains("not counted"), "{text}");
        assert!(text.contains("not connected"), "{text}");
        assert!(
            text.contains("Only one keyboard"),
            "the warning must still fire: {text}"
        );

        // ...and the claim itself is refused, which is the whole point.
        let refusal = winusb::plan_claim(&survey, "MI_00", std::path::Path::new("."))
            .expect_err("the panel is the only keyboard that can type");
        assert_eq!(refusal.code(), "last-keyboard");
    }

    /// The last-keyboard refusal is exit **2**, like every other "nothing was
    /// changed" answer. Scripts and the runbook both key on it.
    #[test]
    fn the_last_keyboard_refusal_exits_two() {
        let refusal = winusb::plan_claim(&one_keyboard_only(), "MI_00", std::path::Path::new("."))
            .expect_err("one keyboard, and it is the panel");
        assert_eq!(refusal.code(), "last-keyboard");
        assert_eq!(EXIT_REFUSED, 2);
        assert!(refusal.advice().contains("second keyboard"), "{refusal}");
    }
}
