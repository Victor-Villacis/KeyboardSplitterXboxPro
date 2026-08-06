//! The one adapter from typed verbs to a [`ControlSource`], over **either**
//! transport.
//!
//! This is the module the M9 decision is about (docs/M9-DECISION.md §6). A
//! surface is written once against [`ControlSource`]; a [`VerbSink`] decides
//! how the verb reaches the daemon:
//!
//! | sink | how | consumer |
//! |---|---|---|
//! | [`crate::PipeTransport`] | one `\\.\pipe\ksx-daemon` JSON line per call | Studio, `ksx session`, the E5 MCP shim, a shell in a SEPARATE process |
//! | a hosted supervisor | the daemon's own dispatch, called directly — no line, no parse | a UI hosted INSIDE the daemon process |
//!
//! The second row is E7's "no serialization tax, mapping 1:1 to
//! `DaemonCommand`", delivered as an implementation of a shared trait rather
//! than as a second copy of every verb. Nothing above this line knows which
//! row it is on, which is exactly why building the native shell later costs
//! the shell and not the verbs.

use crate::control::{
    map_request, BindOutcome, BindRequest, ControlSource, LearnView, MacroOutcome, MacroWrite,
    SessionView,
};
use crate::refusal::{codes, Refusal};
use crate::wire::{
    BackupView, BackupsRequest, ClearAllRequest, LearnResponse, Request, Response, RestoreMode,
    RestoreRequest,
};

/// Performs one typed verb. The whole seam between a surface and a daemon.
///
/// `Err` means the verb could not be ASKED (nothing answered, the line broke,
/// the answer was unreadable). A daemon that answered "no" is `Ok` carrying a
/// response whose `ok` is false — a refusal is data, not an error, because
/// every refusal this protocol produces has a sentence and often a structure
/// (the conflicting slot, the validator's rows) that a surface must show.
pub trait VerbSink: Send + Sync {
    fn call(&self, request: &Request) -> Result<Response, Refusal>;
}

/// Turns any [`VerbSink`] into the [`ControlSource`] every surface consumes.
pub struct Client<S: VerbSink> {
    sink: S,
    offline_profile: Option<Box<dyn Fn() -> Option<String> + Send + Sync>>,
}

impl<S: VerbSink> Client<S> {
    pub fn new(sink: S) -> Self {
        Self {
            sink,
            offline_profile: None,
        }
    }

    /// What to say the profile is when the daemon cannot be reached.
    ///
    /// The banner that appears when nothing answers has to print the command
    /// that would START a daemon, and on a games.toml cabinet that command
    /// needs its `--game` flag or it refuses. The pipe cannot supply it —
    /// it is the thing that just failed — so the caller supplies a reader that
    /// looks at the CONFIG instead. Without one the banner still prints, with
    /// the plain `ksx daemon`.
    #[must_use]
    pub fn with_offline_profile(
        mut self,
        profile: impl Fn() -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.offline_profile = Some(Box::new(profile));
        self
    }

    /// The sink, for a caller that needs the untyped edge (a diagnostic verb).
    pub fn sink(&self) -> &S {
        &self.sink
    }

    fn offline_profile(&self) -> Option<String> {
        self.offline_profile.as_ref().and_then(|read| read())
    }

    /// The remedy line a no-channel refusal carries: the exact command that
    /// starts a daemon for THIS cabinet.
    fn start_command(&self) -> String {
        match self.offline_profile() {
            Some(profile) => format!("ksx daemon --game \"{profile}\""),
            None => "ksx daemon".to_owned(),
        }
    }

    /// One verb, with the no-channel refusal decorated with the way out.
    fn call(&self, request: &Request) -> Result<Response, Refusal> {
        self.sink.call(request).map_err(|refusal| {
            if refusal.is_no_channel() {
                refusal.remedy(self.start_command())
            } else {
                refusal
            }
        })
    }

    /// The daemon answered a shape this verb never produces — a protocol
    /// mismatch, which is a bug rather than a refusal, and says so.
    fn mismatch(request: &Request) -> Refusal {
        Refusal::new(
            codes::PIPE_ERROR,
            format!(
                "the daemon answered `{}` with a different verb's shape",
                request.verb()
            ),
        )
    }

    /// The shared tail of `start` / `stop` / `reload` / the whole-preset
    /// writes: a sentence on success, a refusal on refusal.
    fn action(&self, request: Request) -> Result<String, Refusal> {
        match self.call(&request)? {
            Response::Action(answer) => match answer.refusal() {
                None => Ok(answer.message.unwrap_or_else(|| "done".to_owned())),
                Some(refusal) => Err(refusal),
            },
            Response::Restore(answer) => match answer.refusal() {
                None => Ok(answer.message.unwrap_or_else(|| "done".to_owned())),
                Some(refusal) => Err(refusal),
            },
            _ => Err(Self::mismatch(&request)),
        }
    }

    fn learn(&self, request: Request) -> LearnView {
        match self.call(&request) {
            Ok(Response::Learn(answer)) => learn_view(answer),
            Ok(_) => LearnView::unavailable(Self::mismatch(&request).message),
            Err(refusal) => LearnView::unavailable(refusal.message),
        }
    }
}

/// One learn answer → the mapper's view. An `ok:false` (the running-session
/// gate, or an old daemon's "unknown verb") is the honest `unavailable` with
/// the daemon's own words — which is exactly what flips the mapper read-only
/// with the reason on screen.
fn learn_view(answer: LearnResponse) -> LearnView {
    if !answer.ok {
        return LearnView::unavailable(
            answer
                .error
                .unwrap_or_else(|| "the daemon refused".to_owned()),
        );
    }
    LearnView {
        ok: true,
        state: if answer.state.is_empty() {
            "unknown".to_owned()
        } else {
            answer.state
        },
        remaining_ms: answer.remaining_ms,
        device: answer.device,
        key: answer.key,
        error: answer.error,
    }
}

impl<S: VerbSink> ControlSource for Client<S> {
    fn session(&self) -> SessionView {
        match self.call(&Request::Status) {
            Ok(Response::Status(status)) => SessionView::from(status),
            Ok(_) => SessionView::unreachable(Self::mismatch(&Request::Status).message),
            // No daemon: the page's banner still has to print the command that
            // would START one, and on a games.toml cabinet that command needs
            // its --game flag or it refuses. So the profile comes from the
            // CONFIG here, not from the pipe that just failed to answer.
            Err(refusal) => SessionView {
                profile: self.offline_profile(),
                ..SessionView::unreachable(refusal.message)
            },
        }
    }

    fn start(&self, profile: Option<&str>) -> Result<String, Refusal> {
        self.action(Request::Start {
            profile: profile.map(str::to_owned),
        })
    }

    fn stop(&self) -> Result<String, Refusal> {
        self.action(Request::Stop)
    }

    fn reload(&self) -> Result<String, Refusal> {
        self.action(Request::Reload)
    }

    fn learn_start(&self) -> LearnView {
        self.learn(Request::LearnKey)
    }

    fn learn_poll(&self) -> LearnView {
        self.learn(Request::LearnPoll)
    }

    fn learn_cancel(&self) -> LearnView {
        self.learn(Request::LearnCancel)
    }

    fn bind(&self, request: &BindRequest) -> BindOutcome {
        self.map(Request::Map(request.to_request()))
    }

    /// The control's WHOLE key list in ONE call — `bind_keys` is the seam, and
    /// this implementation is why the mapper's "Add another key" is atomic
    /// rather than read-modify-write.
    fn bind_keys(
        &self,
        preset: &str,
        function: &str,
        keys: &[String],
        force: bool,
        reload: bool,
        turbo_hz: Option<u32>,
    ) -> BindOutcome {
        self.map(Request::Map(map_request(
            preset, function, keys, force, reload, turbo_hz,
        )))
    }

    fn restore(&self, preset: &str, mode: RestoreMode) -> Result<String, Refusal> {
        self.action(Request::MapRestore(RestoreRequest {
            preset: preset.to_owned(),
            mode,
            // The daemon only applies to a RUNNING session; with nothing
            // running there is nothing to bounce and the next start reads it.
            reload: true,
        }))
    }

    fn clear_all(&self, preset: &str) -> Result<String, Refusal> {
        self.action(Request::MapClearAll(ClearAllRequest {
            preset: preset.to_owned(),
            reload: true,
        }))
    }

    fn backups(&self, preset: &str) -> Result<Vec<BackupView>, Refusal> {
        let request = Request::MapBackups(BackupsRequest {
            preset: preset.to_owned(),
        });
        match self.call(&request)? {
            Response::Backups(answer) => match answer.refusal() {
                None => Ok(answer.backups),
                Some(refusal) => Err(refusal),
            },
            _ => Err(Self::mismatch(&request)),
        }
    }

    fn save_macro(&self, request: &MacroWrite) -> MacroOutcome {
        let typed = match request.to_request() {
            Ok(typed) => typed,
            // Refused before a round trip, in the daemon's own words: a policy
            // this build cannot spell is a policy the daemon would refuse.
            Err(refusal) => return MacroOutcome::failed(refusal.message),
        };
        let request = Request::MapMacro(typed);
        match self.call(&request) {
            Ok(Response::Macro(answer)) => MacroOutcome {
                ok: answer.ok,
                message: answer.message,
                error: answer.error,
                code: answer.code,
                problems: answer.problems,
                warnings: answer.warnings,
                deleted: answer.deleted,
                enabled: answer.enabled,
                toggled: answer.toggled,
                backup: answer.backup.map(|backup| backup.label),
                reloaded: answer.reloaded,
            },
            Ok(_) => MacroOutcome::failed(Self::mismatch(&request).message),
            Err(refusal) => MacroOutcome::failed(refusal.message),
        }
    }
}

impl<S: VerbSink> Client<S> {
    /// The shared tail of `bind` and `bind_keys`.
    fn map(&self, request: Request) -> BindOutcome {
        match self.call(&request) {
            Ok(Response::Map(answer)) => BindOutcome::from(answer),
            Ok(_) => BindOutcome::failed(Self::mismatch(&request).message),
            Err(refusal) => BindOutcome::failed(refusal.message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::MacroStepView;
    use crate::wire::{ActionResponse, MacroResponse, MapResponse, StatusResponse};
    use std::sync::Mutex;

    /// A sink that records what it was asked and answers what it was told to
    /// — the fake every surface's tests can drive, with no daemon and no pipe.
    #[derive(Default)]
    struct Fake {
        asked: Mutex<Vec<Request>>,
        answer: Mutex<Option<Response>>,
        fail: Mutex<Option<Refusal>>,
    }

    impl Fake {
        fn answering(response: Response) -> Self {
            Self {
                answer: Mutex::new(Some(response)),
                ..Self::default()
            }
        }

        fn failing(refusal: Refusal) -> Self {
            Self {
                fail: Mutex::new(Some(refusal)),
                ..Self::default()
            }
        }

        fn last(&self) -> Request {
            self.asked.lock().unwrap().last().cloned().expect("asked")
        }
    }

    impl VerbSink for Fake {
        fn call(&self, request: &Request) -> Result<Response, Refusal> {
            self.asked.lock().unwrap().push(request.clone());
            if let Some(refusal) = self.fail.lock().unwrap().clone() {
                return Err(refusal);
            }
            self.answer
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| Refusal::new(codes::PIPE_ERROR, "the fake was given no answer"))
        }
    }

    #[test]
    fn a_key_list_edit_is_one_map_request_carrying_the_whole_list() {
        let client = Client::new(Fake::answering(Response::Map(MapResponse {
            ok: true,
            message: Some("\"IPAC P1\": A = S, Enter".into()),
            keys: vec!["S".into(), "Enter".into()],
            ..MapResponse::default()
        })));
        let outcome = client.bind_keys(
            "IPAC P1",
            "A",
            &["S".to_owned(), "Enter".to_owned()],
            false,
            true,
            None,
        );
        assert!(outcome.ok);
        let Request::Map(sent) = client.sink().last() else {
            panic!("one map request");
        };
        assert_eq!(sent.keys, ["S", "Enter"]);
        assert!(sent.key.is_none(), "one field, not both");
        assert!(sent.reload);
    }

    /// The no-channel refusal is the one every surface renders as "nothing
    /// works right now" — and it arrives carrying the command that fixes it,
    /// with the cabinet's own profile in it.
    #[test]
    fn no_daemon_gives_every_verb_the_same_worded_refusal_with_a_remedy() {
        let client = Client::new(Fake::failing(Refusal::with_remedy(
            codes::NO_CHANNEL,
            crate::pipe::NO_CHANNEL,
            "ksx daemon",
        )))
        .with_offline_profile(|| Some("Street Fighter".to_owned()));

        let refusal = client.stop().unwrap_err();
        assert!(refusal.is_no_channel());
        assert_eq!(refusal.message, crate::pipe::NO_CHANNEL);
        assert_eq!(
            refusal.remedy.as_deref(),
            Some(r#"ksx daemon --game "Street Fighter""#),
            "the banner must print a command that actually starts THIS cabinet"
        );

        // Every other verb says the same thing, in its own shape — no control
        // is ever a silent no-op.
        let session = client.session();
        assert!(!session.reachable);
        assert_eq!(session.line, crate::pipe::NO_CHANNEL);
        assert_eq!(session.profile.as_deref(), Some("Street Fighter"));

        let bind = client.bind_keys("P", "A", &["G".to_owned()], false, true, None);
        assert!(!bind.ok);
        assert_eq!(bind.error.as_deref(), Some(crate::pipe::NO_CHANNEL));

        let learn = client.learn_start();
        assert_eq!(learn.state, "unavailable");
        assert_eq!(learn.error.as_deref(), Some(crate::pipe::NO_CHANNEL));

        let macro_outcome = client.save_macro(&MacroWrite {
            preset: "P".into(),
            name: "m".into(),
            steps: vec![MacroStepView {
                hold: vec!["A".into()],
                ms: Some(50),
                ..MacroStepView::default()
            }],
            ..MacroWrite::default()
        });
        assert!(!macro_outcome.ok);
        assert_eq!(
            macro_outcome.error.as_deref(),
            Some(crate::pipe::NO_CHANNEL)
        );
    }

    #[test]
    fn an_action_answers_with_the_daemons_sentence_or_its_refusal() {
        let ok = Client::new(Fake::answering(Response::Action(ActionResponse {
            ok: true,
            message: Some("running (4 slot(s))".into()),
            ..ActionResponse::default()
        })));
        assert_eq!(
            ok.start(Some("Steam")).unwrap(),
            "running (4 slot(s))".to_owned()
        );
        assert_eq!(
            ok.sink().last(),
            Request::Start {
                profile: Some("Steam".into())
            }
        );

        let refused = Client::new(Fake::answering(Response::Action(ActionResponse {
            ok: false,
            error: Some("already running".into()),
            ..ActionResponse::default()
        })));
        let refusal = refused.start(None).unwrap_err();
        assert_eq!(refusal.message, "already running");
        assert_eq!(refusal.code, codes::REFUSED);
    }

    /// A macro save is ONE request carrying the whole table, and the answer's
    /// backup label comes back so the toast can say the road home exists
    /// without waiting for the next poll.
    #[test]
    fn a_macro_save_is_one_request_and_its_answer_names_the_undo() {
        let client = Client::new(Fake::answering(Response::Macro(MacroResponse {
            ok: true,
            message: Some("saved".into()),
            backup: Some(crate::wire::BackupView {
                stamp: "20260806-120000".into(),
                label: "2026-08-06 12:00:00 UTC".into(),
                path: "C:\\p.toml.bak-20260806-120000".into(),
            }),
            enabled: true,
            ..MacroResponse::default()
        })));
        let outcome = client.save_macro(&MacroWrite {
            preset: "SF P1".into(),
            name: "hadouken".into(),
            steps: vec![MacroStepView {
                hold: vec!["A".into()],
                ms: Some(50),
                ..MacroStepView::default()
            }],
            repeat: "while-held".into(),
            reload: true,
            ..MacroWrite::default()
        });
        assert!(outcome.ok, "{outcome:?}");
        assert_eq!(outcome.backup.as_deref(), Some("2026-08-06 12:00:00 UTC"));

        let Request::MapMacro(sent) = client.sink().last() else {
            panic!("one map-macro request");
        };
        // THE REGRESSION: the policy the card set is on the request.
        let body = sent.body();
        assert_eq!(body.repeat, ksx_core::Repeat::WhileHeld);
        assert_eq!(body.steps.len(), 1);
    }

    /// A daemon answering the wrong shape is a protocol bug, and it is named
    /// as one rather than rendered as an empty success.
    #[test]
    fn a_mismatched_answer_is_reported_not_swallowed() {
        let client = Client::new(Fake::answering(Response::Status(StatusResponse {
            ok: true,
            run: "running".into(),
            ..StatusResponse::default()
        })));
        let refusal = client.stop().unwrap_err();
        assert_eq!(refusal.code, codes::PIPE_ERROR);
        assert!(
            refusal.message.contains("different verb's shape"),
            "{refusal}"
        );
    }
}
