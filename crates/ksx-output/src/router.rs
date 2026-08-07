//! Backend selection: which driver stack materializes a slot.
//!
//! # The rule
//!
//! **Config expresses intent, not plumbing.** A slot says
//! `persona = "dualsense"`; it never says `backend = "hidmaestro"`. The backend
//! is *derived* from the persona ([`ksx_core::Persona::backend`]), for a reason
//! that is not stylistic: the two are not independent. `persona = "xbox360",
//! backend = "hidmaestro"` would be a configuration a user could write, could
//! not get working, and would then debug — so it must not be expressible.
//!
//! Concretely (`docs/ENHANCEMENTS.md` E1/E4):
//! - `xbox360`, `playstation` → **ViGEmBus, always**. Slots 1–4 never migrate:
//!   HIDMaestro's XInput is a synthesis layer and the direct cause of its WGI
//!   double-input bug, while ViGEm's X360 target is Microsoft's own
//!   `xusb22.sys`. There is nothing to gain and a known bug to inherit.
//! - `dualsense`, `switchpro`, `xboxseries` → **HIDMaestro**, because ViGEmBus
//!   cannot express them at all and never will (the project is frozen).
//!
//! # Laziness is part of the rule
//!
//! [`RoutedBackend`] only builds the HIDMaestro backend when a slot actually
//! asks for it. A cabinet running four X360 pads and four DS4 pads — every
//! configuration ksx ships today — never constructs it, never probes for the
//! driver, and cannot be broken by its absence.

use ksx_core::{PadBackend, PadState, Persona};

use crate::backend::{Feedback, PadHandle, VirtualPadBackend};
use crate::error::OutputError;

/// How a [`RoutedBackend`] obtains the HIDMaestro backend, the first time a
/// persona needs it.
///
/// A boxed factory rather than an eagerly-built backend so that the ordinary
/// cabinet never pays for a driver it does not use.
pub type HidMaestroFactory =
    Box<dyn FnMut() -> Result<Box<dyn VirtualPadBackend>, OutputError> + Send>;

struct Entry {
    which: PadBackend,
    inner: PadHandle,
}

/// Routes each pad to the backend its persona requires.
///
/// Implements [`VirtualPadBackend`] itself, so the engine's output thread holds
/// one `Box<dyn VirtualPadBackend>` exactly as it always did and knows nothing
/// about there being two stacks.
///
/// Handles are re-issued from the router's own monotonic counter, never passed
/// through: two backends would otherwise both hand out `PadHandle(0)` and the
/// router could not tell them apart. Router handles are never reused, so a
/// stale handle always resolves to [`OutputError::UnknownHandle`] rather than
/// aliasing a live pad on the *other* stack — the worst possible failure here.
pub struct RoutedBackend {
    vigem: Box<dyn VirtualPadBackend>,
    hidmaestro: Option<Box<dyn VirtualPadBackend>>,
    factory: Option<HidMaestroFactory>,
    /// Whether [`Persona::can_plug`] is enforced before anything is dispatched.
    ///
    /// True for the routers ksx builds for itself, where the answer is a fact
    /// about this binary and there is nothing to gain by handing a persona to a
    /// stack that cannot create it. False for [`RoutedBackend::with_hidmaestro`]:
    /// a caller who supplies a backend has supplied the very thing the build
    /// fact says is missing, and that is how the two-stack dispatch below stays
    /// under test while the second stack has no production driver.
    enforce_build_capability: bool,
    pads: std::collections::BTreeMap<u32, Entry>,
    next_id: u32,
}

impl RoutedBackend {
    /// A router with only ViGEmBus.
    pub fn vigem_only(vigem: Box<dyn VirtualPadBackend>) -> Self {
        Self {
            vigem,
            hidmaestro: None,
            factory: None,
            enforce_build_capability: true,
            pads: Default::default(),
            next_id: 0,
        }
    }

    /// The production router: ViGEmBus now, HIDMaestro if and when a slot asks
    /// for a persona that needs it **and this build can create one**.
    ///
    /// This is the one every entry point should use. It costs nothing on a
    /// cabinet that uses neither the DualSense, Switch Pro, nor Xbox Series
    /// persona: [`HidMaestroBackend::connect`](crate::HidMaestroBackend::connect)
    /// is never called, so the driver is never probed for and its absence
    /// cannot fail a run.
    ///
    /// The factory is wired even though today's build gate refuses every
    /// persona that would reach it. That is deliberate and it is the point:
    /// flipping [`ksx_core::PadBackend::is_implemented`] in the commit that
    /// lands a real driver lights this path up with nothing else to change.
    pub fn standard(vigem: Box<dyn VirtualPadBackend>) -> Self {
        Self {
            vigem,
            hidmaestro: None,
            factory: Some(Box::new(|| {
                crate::HidMaestroBackend::connect()
                    .map(|b| Box::new(b) as Box<dyn VirtualPadBackend>)
            })),
            enforce_build_capability: true,
            pads: Default::default(),
            next_id: 0,
        }
    }

    /// A router that will build the HIDMaestro backend on first need, from a
    /// factory the caller vouches for.
    ///
    /// Does **not** apply the build-capability gate — see
    /// [`RoutedBackend::enforce_build_capability`]. Use
    /// [`RoutedBackend::standard`] for anything a user's config reaches.
    pub fn with_hidmaestro(vigem: Box<dyn VirtualPadBackend>, factory: HidMaestroFactory) -> Self {
        Self {
            vigem,
            hidmaestro: None,
            factory: Some(factory),
            enforce_build_capability: false,
            pads: Default::default(),
            next_id: 0,
        }
    }

    /// Whether the HIDMaestro backend has actually been constructed.
    ///
    /// Diagnostic, and the thing the laziness test asserts on.
    pub fn hidmaestro_started(&self) -> bool {
        self.hidmaestro.is_some()
    }

    /// Which backend a persona routes to. Restates nothing — it is
    /// [`Persona::backend`].
    pub fn backend_for(persona: Persona) -> PadBackend {
        persona.backend()
    }

    fn stack(&mut self, which: PadBackend) -> Result<&mut Box<dyn VirtualPadBackend>, OutputError> {
        match which {
            PadBackend::Vigem => Ok(&mut self.vigem),
            PadBackend::HidMaestro => {
                if self.hidmaestro.is_none() {
                    let factory = self
                        .factory
                        .as_mut()
                        .ok_or(OutputError::BackendUnavailable(PadBackend::HidMaestro))?;
                    self.hidmaestro = Some(factory()?);
                }
                Ok(self.hidmaestro.as_mut().expect("just built"))
            }
        }
    }

    fn entry(&self, handle: PadHandle) -> Result<&Entry, OutputError> {
        self.pads
            .get(&handle.0)
            .ok_or(OutputError::UnknownHandle(handle))
    }

    fn resolve(
        &mut self,
        handle: PadHandle,
    ) -> Result<(&mut Box<dyn VirtualPadBackend>, PadHandle), OutputError> {
        let entry = *self
            .pads
            .get(&handle.0)
            .map(|e| (e.which, e.inner))
            .as_ref()
            .ok_or(OutputError::UnknownHandle(handle))?;
        let (which, inner) = entry;
        let stack = match which {
            PadBackend::Vigem => &mut self.vigem,
            // Already built: a handle cannot exist for a backend that was never
            // constructed.
            PadBackend::HidMaestro => self
                .hidmaestro
                .as_mut()
                .ok_or(OutputError::UnknownHandle(handle))?,
        };
        Ok((stack, inner))
    }
}

impl VirtualPadBackend for RoutedBackend {
    fn plug(&mut self) -> Result<PadHandle, OutputError> {
        self.plug_persona(Persona::default())
    }

    fn plug_persona(&mut self, persona: Persona) -> Result<PadHandle, OutputError> {
        // Checked before the machine is consulted, because the machine is the
        // wrong witness. Reaching the factory instead would probe for
        // HIDMaestro and report whatever it found — so on a machine where the
        // driver IS installed the failure would arrive as a driver error, from
        // a stack that could not have created the pad either way.
        if self.enforce_build_capability && !persona.can_plug() {
            return Err(OutputError::PersonaNotImplemented(persona));
        }
        let which = Self::backend_for(persona);
        let inner = self.stack(which)?.plug_persona(persona)?;
        let id = self.next_id;
        self.next_id += 1;
        self.pads.insert(id, Entry { which, inner });
        Ok(PadHandle(id))
    }

    fn persona(&self, handle: PadHandle) -> Option<Persona> {
        let entry = self.pads.get(&handle.0)?;
        match entry.which {
            PadBackend::Vigem => self.vigem.persona(entry.inner),
            PadBackend::HidMaestro => self.hidmaestro.as_ref()?.persona(entry.inner),
        }
    }

    fn user_index(&self, handle: PadHandle) -> Option<u8> {
        let entry = self.pads.get(&handle.0)?;
        match entry.which {
            PadBackend::Vigem => self.vigem.user_index(entry.inner),
            PadBackend::HidMaestro => self.hidmaestro.as_ref()?.user_index(entry.inner),
        }
    }

    fn update(&mut self, handle: PadHandle, state: &PadState) -> Result<(), OutputError> {
        let (stack, inner) = self.resolve(handle)?;
        stack.update(inner, state)
    }

    fn poll_feedback(&mut self, handle: PadHandle) -> Option<Feedback> {
        let (stack, inner) = self.resolve(handle).ok()?;
        stack.poll_feedback(inner)
    }

    fn unplug(&mut self, handle: PadHandle) -> Result<(), OutputError> {
        // Look up first, remove only on the way out: a failed unplug must not
        // orphan the pad in the underlying backend with no handle left to
        // retry through.
        let _ = self.entry(handle)?;
        let (stack, inner) = self.resolve(handle)?;
        let result = stack.unplug(inner);
        self.pads.remove(&handle.0);
        result
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use ksx_core::pad::XButtons;

    use super::*;
    use crate::MockBackend;

    fn router() -> (RoutedBackend, Arc<AtomicUsize>) {
        let builds = Arc::new(AtomicUsize::new(0));
        let counter = builds.clone();
        let router = RoutedBackend::with_hidmaestro(
            Box::new(MockBackend::new()),
            Box::new(move || {
                counter.fetch_add(1, Ordering::Relaxed);
                Ok(Box::new(MockBackend::new()) as Box<dyn VirtualPadBackend>)
            }),
        );
        (router, builds)
    }

    #[test]
    fn the_rule_is_derived_from_the_persona_and_nothing_else() {
        assert_eq!(
            RoutedBackend::backend_for(Persona::Xbox360),
            PadBackend::Vigem
        );
        assert_eq!(
            RoutedBackend::backend_for(Persona::PlayStation),
            PadBackend::Vigem
        );
        for p in [Persona::DualSense, Persona::SwitchPro, Persona::XboxSeries] {
            assert_eq!(RoutedBackend::backend_for(p), PadBackend::HidMaestro, "{p}");
        }
    }

    /// The cabinet's own configuration must never touch HIDMaestro — not even
    /// to probe for it.
    #[test]
    fn a_vigem_only_cabinet_never_constructs_the_hidmaestro_backend() {
        let (mut r, builds) = router();
        for _ in 0..4 {
            r.plug_persona(Persona::Xbox360).unwrap();
        }
        for _ in 0..4 {
            r.plug_persona(Persona::PlayStation).unwrap();
        }
        assert_eq!(builds.load(Ordering::Relaxed), 0);
        assert!(!r.hidmaestro_started());
    }

    #[test]
    fn the_hidmaestro_backend_is_built_once_on_first_need() {
        let (mut r, builds) = router();
        r.plug_persona(Persona::Xbox360).unwrap();
        assert_eq!(builds.load(Ordering::Relaxed), 0);
        r.plug_persona(Persona::DualSense).unwrap();
        assert_eq!(builds.load(Ordering::Relaxed), 1);
        r.plug_persona(Persona::SwitchPro).unwrap();
        assert_eq!(builds.load(Ordering::Relaxed), 1, "built once, reused");
        assert!(r.hidmaestro_started());
    }

    /// Handles from the two stacks must never collide. Both mocks hand out
    /// `PadHandle(0)` first, so without re-issuing, an update meant for the
    /// DualSense would land on the X360 pad.
    #[test]
    fn handles_from_the_two_stacks_never_alias() {
        let (mut r, _) = router();
        let x = r.plug_persona(Persona::Xbox360).unwrap();
        let d = r.plug_persona(Persona::DualSense).unwrap();
        assert_ne!(x.raw(), d.raw());
        assert_eq!(r.persona(x), Some(Persona::Xbox360));
        assert_eq!(r.persona(d), Some(Persona::DualSense));

        let pressed = PadState {
            buttons: XButtons::A,
            ..PadState::default()
        };
        r.update(d, &pressed).unwrap();
        // The X360 pad must have received nothing.
        assert_eq!(r.persona(x), Some(Persona::Xbox360));
        r.unplug(d).unwrap();
        assert!(matches!(
            r.update(d, &pressed).unwrap_err(),
            OutputError::UnknownHandle(_)
        ));
        // ...and the surviving X360 pad is untouched by its neighbour's death.
        r.update(x, &pressed).unwrap();
    }

    #[test]
    fn a_stale_handle_never_aliases_a_later_pad_on_either_stack() {
        let (mut r, _) = router();
        let first = r.plug_persona(Persona::DualSense).unwrap();
        r.unplug(first).unwrap();
        let second = r.plug_persona(Persona::Xbox360).unwrap();
        assert_ne!(first.raw(), second.raw());
        assert_eq!(r.persona(first), None);
    }

    #[test]
    fn without_a_factory_a_hidmaestro_persona_is_refused_not_downgraded() {
        let mut r = RoutedBackend::vigem_only(Box::new(MockBackend::new()));
        let err = r.plug_persona(Persona::DualSense).unwrap_err();
        // The build gate answers first, and it is the more fundamental fact:
        // "no factory is wired up" invites someone to wire one, and there is
        // nothing to wire.
        assert!(
            matches!(err, OutputError::PersonaNotImplemented(Persona::DualSense)),
            "{err}"
        );
        // A ViGEm persona still works on the same router.
        assert!(r.plug_persona(Persona::PlayStation).is_ok());
    }

    /// The regression this whole change exists for.
    ///
    /// A production router must refuse the three HIDMaestro personas **without
    /// consulting the machine**, because the machine's answer is the wrong one:
    /// on a box where HIDMaestro is installed the probe says yes, and the pad
    /// still cannot be created. A gate that flips with an install would offer a
    /// persona that plugs no better than before.
    #[test]
    fn the_production_router_refuses_unbuildable_personas_without_probing() {
        let mut r = RoutedBackend::standard(Box::new(MockBackend::new()));
        for persona in [Persona::DualSense, Persona::SwitchPro, Persona::XboxSeries] {
            let err = r.plug_persona(persona).unwrap_err();
            assert!(
                matches!(err, OutputError::PersonaNotImplemented(p) if p == persona),
                "{persona}: {err}"
            );
            assert!(!err.is_hidmaestro_missing(), "{persona}: {err}");
            assert!(err.is_not_implemented(), "{persona}: {err}");
        }
        // Nothing was constructed, so nothing was probed — the refusal is the
        // same on a machine with HIDMaestro installed as on one without.
        assert!(!r.hidmaestro_started());
        // ...and the cabinet's own personas are untouched.
        assert!(r.plug_persona(Persona::Xbox360).is_ok());
        assert!(r.plug_persona(Persona::PlayStation).is_ok());
    }

    #[test]
    fn a_failing_factory_surfaces_its_own_error_verbatim() {
        let mut r = RoutedBackend::with_hidmaestro(
            Box::new(MockBackend::new()),
            Box::new(|| {
                Err(OutputError::HidMaestroMissing {
                    probe: "looked for X and found none".into(),
                })
            }),
        );
        let err = r.plug_persona(Persona::XboxSeries).unwrap_err();
        assert!(
            matches!(err, OutputError::HidMaestroMissing { .. }),
            "{err}"
        );
        // And it retries next time rather than caching the failure — the user
        // may have installed the driver in between.
        assert!(r.plug_persona(Persona::XboxSeries).is_err());
        assert!(!r.hidmaestro_started());
    }

    /// `standard()` must be as lazy as the hand-wired router: a run that
    /// touches only ViGEm personas has to succeed whatever else is missing.
    #[test]
    fn the_standard_router_still_runs_a_full_cabinet_without_hidmaestro() {
        let mut r = RoutedBackend::standard(Box::new(MockBackend::new()));
        for _ in 0..4 {
            r.plug_persona(Persona::Xbox360).unwrap();
        }
        for _ in 0..4 {
            r.plug_persona(Persona::PlayStation).unwrap();
        }
        assert!(!r.hidmaestro_started());
    }

    #[test]
    fn feedback_is_drained_from_the_stack_that_owns_the_pad() {
        let (mut r, _) = router();
        let x = r.plug_persona(Persona::Xbox360).unwrap();
        let d = r.plug_persona(Persona::DualSense).unwrap();
        // Nothing scripted: both must simply be silent, not panic or cross over.
        assert_eq!(r.poll_feedback(x), None);
        assert_eq!(r.poll_feedback(d), None);
    }

    #[test]
    fn user_index_comes_from_the_owning_stack() {
        let (mut r, _) = router();
        let a = r.plug_persona(Persona::Xbox360).unwrap();
        let b = r.plug_persona(Persona::Xbox360).unwrap();
        assert_eq!(r.user_index(a), Some(0));
        assert_eq!(r.user_index(b), Some(1));
        // A HID persona takes no XInput slot on either stack.
        let ps = r.plug_persona(Persona::PlayStation).unwrap();
        assert_eq!(r.user_index(ps), None);
    }
}
