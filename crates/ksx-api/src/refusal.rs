//! One refusal type, replacing the bare `String` errors the surfaces used to
//! pass around (docs/M9-DECISION.md §6).
//!
//! The point is not tidiness. CONTROL-SURFACE's invariant — **"a surface that
//! cannot act must SAY so, per click"** — was a review checklist item as long
//! as a refusal was a `String`: nothing in the type system asked whether the
//! sentence named the way out. [`Refusal::remedy`] makes it a field, so a
//! surface that wants to print "…and here is the command that works anyway"
//! has somewhere to read it from, and a caller that forgets is visibly
//! forgetting something.

use serde::{Deserialize, Serialize};

/// The stable refusal words. Every one of them is already a `code` on the pipe
/// or a `--json` `code` from the CLI; this module exists so a caller reaches
/// for a constant instead of retyping a string literal that only *looks* like
/// the one the exit codes derive from.
///
/// Codes are not invented per caller. A code that is not here came off the
/// wire from a daemon newer than this binary, and [`Refusal::from_wire`] is
/// the one path that carries it.
pub mod codes {
    /// No daemon answered the control channel.
    pub const NO_CHANNEL: &str = "no-channel";
    /// The control channel was there and the conversation broke.
    pub const PIPE_ERROR: &str = "pipe-error";
    /// The daemon refused and gave no code of its own.
    pub const REFUSED: &str = "refused";
    /// A cross-slot key conflict (`map`).
    pub const CONFLICT: &str = "conflict";
    /// No preset of that name is on disk.
    pub const UNKNOWN_PRESET: &str = "unknown-preset";
    /// No games.toml profile of that title (`slot-assign --profile`).
    pub const UNKNOWN_PROFILE: &str = "unknown-profile";
    /// A slot number outside 1..=`ksx_core::MAX_SLOTS` (`slot-assign`).
    pub const BAD_SLOT: &str = "bad-slot";
    /// No `[macros.<name>]` table of that name in that preset.
    pub const UNKNOWN_MACRO: &str = "unknown-macro";
    /// The macro body did not validate.
    pub const MACRO_INVALID: &str = "macro-invalid";
    /// A restore mode / verb argument this build does not know.
    pub const BAD_REQUEST: &str = "bad-request";
    /// The operation exists, but not on this surface — the remedy names the
    /// one that has it. This is the honest answer a defaulted trait method
    /// gives, and it is never a silent no-op.
    pub const NOT_HERE: &str = "not-here";
}

/// A refusal: what was refused, why, and — when one exists — the command that
/// works anyway.
///
/// The decision document spells `code` as `&'static str`, and the intent
/// behind that (never invent a code per caller) is kept by the constructors,
/// which all take `&'static str`. The field itself is a `String` for exactly
/// one reason: a daemon NEWER than this binary can answer with a code this
/// build has never heard of, and dropping it — or mapping it to "refused" —
/// would throw away the only machine-readable half of the answer.
/// [`Refusal::from_wire`] is the only path that produces one of those.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    /// The stable code (see [`codes`]). Same word the CLI's `--json` prints
    /// and the exit codes derive from.
    pub code: String,
    /// One sentence naming what was refused and why. This is what a page
    /// flashes; [`std::fmt::Display`] is exactly this string and nothing else,
    /// so wrapping a refusal in `format!` cannot silently change the wording a
    /// test pinned.
    pub message: String,
    /// The command that works anyway, when one exists — the `ksx map`
    /// one-liner the "No daemon" banner already prints, the `ksx daemon` line
    /// that would start the channel that just failed to answer.
    pub remedy: Option<String>,
}

impl Refusal {
    /// A refusal with a code this build knows.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
            remedy: None,
        }
    }

    /// The same, plus the command that works anyway.
    pub fn with_remedy(
        code: &'static str,
        message: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
            remedy: Some(remedy.into()),
        }
    }

    /// A refusal read off the wire: the daemon's own code and sentence,
    /// carried verbatim. `code` is [`codes::REFUSED`] when the answer had
    /// none — never a guess at which one it meant.
    pub fn from_wire(code: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            code: code
                .filter(|c| !c.trim().is_empty())
                .unwrap_or(codes::REFUSED)
                .to_owned(),
            message: message.into(),
            remedy: None,
        }
    }

    /// The answer a defaulted trait method gives: this surface cannot do it,
    /// and here is the one that can. Never a silent no-op — the whole reason
    /// the defaults are worded (docs/M9-DECISION.md §6).
    pub fn not_here(what: &str, remedy: impl Into<String>) -> Self {
        let remedy = remedy.into();
        Self {
            code: codes::NOT_HERE.to_owned(),
            message: format!("{what} is not available on this surface — {remedy}"),
            remedy: Some(remedy),
        }
    }

    /// Attach (or replace) the way out.
    #[must_use]
    pub fn remedy(mut self, remedy: impl Into<String>) -> Self {
        self.remedy = Some(remedy.into());
        self
    }

    /// Is this the "nothing answered the pipe" refusal? The one code surfaces
    /// branch on, because it is the one that means *every* control is inert
    /// rather than this one call being wrong.
    pub fn is_no_channel(&self) -> bool {
        self.code == codes::NO_CHANNEL
    }
}

impl std::fmt::Display for Refusal {
    /// The message, and only the message.
    ///
    /// Deliberately NOT `message + remedy`: the remedy is a second line a
    /// surface places where it has room for it (a banner, a toast's action
    /// row), and folding it into `Display` would rewrite every flash string
    /// the page tests pin the moment a caller adds one.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Refusal {}

/// `Result` with the refusal as the error — the shape every write verb of
/// [`crate::ControlSource`] returns.
pub type Refused<T> = Result<T, Refusal>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_the_message_alone_so_a_remedy_cannot_rewrite_a_flash() {
        let refusal = Refusal::with_remedy(codes::NO_CHANNEL, "no daemon", "run `ksx daemon`");
        assert_eq!(refusal.to_string(), "no daemon");
        assert_eq!(refusal.remedy.as_deref(), Some("run `ksx daemon`"));
        assert!(refusal.is_no_channel());
    }

    #[test]
    fn a_wire_refusal_keeps_a_code_this_build_has_never_heard_of() {
        let newer = Refusal::from_wire(Some("some-future-code"), "the daemon refused");
        assert_eq!(newer.code, "some-future-code");
        // ...and an answer with no code at all is not guessed at.
        assert_eq!(
            Refusal::from_wire(None, "the daemon refused").code,
            codes::REFUSED
        );
        assert_eq!(
            Refusal::from_wire(Some("  "), "the daemon refused").code,
            codes::REFUSED
        );
    }

    #[test]
    fn not_here_always_carries_the_surface_that_can() {
        let refusal = Refusal::not_here("claiming a panel", "run `ksx winusb claim <ID> --yes`");
        assert_eq!(refusal.code, codes::NOT_HERE);
        assert!(refusal.remedy.is_some(), "a refusal owes a way out");
        assert!(refusal.message.contains("ksx winusb claim"), "{refusal}");
    }
}
