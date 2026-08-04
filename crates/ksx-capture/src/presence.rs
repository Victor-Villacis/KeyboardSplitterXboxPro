//! Live device-presence publication from a *running* capture backend.
//!
//! Hotplug detection needs to answer one question while emulation runs: is the
//! device bound to slot N still there? Only the capture backend can answer it —
//! it is the thing holding the driver context. `devices()` is a `&mut self`
//! cold-path enumeration that happens before [`crate::CaptureBackend::run`]
//! consumes the backend, so the supervisor needs a handle that stays live
//! afterwards. This is that handle, shaped exactly like
//! [`crate::health::HealthHandle`]: cloneable, lock-free, grabbed before `run`.
//!
//! Publication is a **cold path**: the Interception backend republishes only
//! when the driver reports itself idle (no strokes pending), at most every few
//! seconds. It allocates; that is fine there and nowhere else.
//!
//! `snapshot()` returns `None` until the backend has published at least once,
//! so a supervisor can distinguish "no devices" from "not known yet" and never
//! invalidates a slot on a startup race.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use ksx_core::DeviceId;

#[derive(Debug)]
struct Inner {
    ids: ArcSwap<Vec<DeviceId>>,
    published: AtomicBool,
}

#[derive(Clone, Debug)]
enum Kind {
    /// A real backend publishing its own device set.
    Live(Arc<Inner>),
    /// A backend with no hotplug visibility.
    Unsupported,
    /// The union of several backends' views ([`PresenceHandle::merged`]).
    Merged(Arc<Vec<PresenceHandle>>),
}

/// Cloneable, thread-safe view of the devices a running backend can see.
///
/// [`PresenceHandle::unsupported`] is the "this backend cannot observe
/// hotplug" value: its snapshots are always `None`, so callers degrade to
/// never invalidating anything rather than to false removals.
#[derive(Clone, Debug)]
pub struct PresenceHandle(Kind);

impl PresenceHandle {
    /// A live handle. Starts unpublished (`snapshot()` is `None`).
    pub fn new() -> Self {
        Self(Kind::Live(Arc::new(Inner {
            ids: ArcSwap::from_pointee(Vec::new()),
            published: AtomicBool::new(false),
        })))
    }

    /// A handle that never reports anything — for backends without hotplug
    /// visibility.
    pub fn unsupported() -> Self {
        Self(Kind::Unsupported)
    }

    /// The union of several backends' views, for [`crate::CompositeBackend`].
    ///
    /// Merging rather than sharing is deliberate: health flags and the escape
    /// latch merge by OR-ing one shared cell, but two backends *publishing*
    /// device lists into one cell would each erase the other's — the WinUSB
    /// thread would republish `[board A]` and the supervisor would read that as
    /// "the desk keyboard was unplugged". So each child keeps its own handle and
    /// the union is computed on read.
    ///
    /// Reports `None` until **every** supported child has published at least
    /// once. A composite that answered "one device" while a second backend was
    /// still starting would tell the supervisor a bound board had vanished, and
    /// the supervisor would invalidate its slots.
    pub fn merged(children: Vec<PresenceHandle>) -> Self {
        Self(Kind::Merged(Arc::new(children)))
    }

    pub fn is_supported(&self) -> bool {
        match &self.0 {
            Kind::Live(_) => true,
            Kind::Unsupported => false,
            Kind::Merged(children) => children.iter().any(Self::is_supported),
        }
    }

    /// Devices currently visible to the backend, or `None` when unsupported or
    /// nothing has been published yet. Lock-free.
    pub fn snapshot(&self) -> Option<Arc<Vec<DeviceId>>> {
        match &self.0 {
            Kind::Live(inner) => inner
                .published
                .load(Ordering::Acquire)
                .then(|| inner.ids.load_full()),
            Kind::Unsupported => None,
            Kind::Merged(children) => {
                let mut union = Vec::new();
                let mut any = false;
                for child in children.iter() {
                    if !child.is_supported() {
                        continue; // contributes nothing, blocks nothing
                    }
                    any = true;
                    union.extend(child.snapshot()?.iter().cloned());
                }
                any.then(|| Arc::new(union))
            }
        }
    }

    /// Publish a new device set. Backends call this from their cold path; tests
    /// call it to script hotplug through the real supervisor code path.
    ///
    /// A no-op on unsupported and merged handles — a merged view has no storage
    /// of its own, by design.
    pub fn publish(&self, ids: Vec<DeviceId>) {
        let Kind::Live(inner) = &self.0 else {
            return;
        };
        inner.ids.store(Arc::new(ids));
        inner.published.store(true, Ordering::Release);
    }
}

impl Default for PresenceHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IPAC: &str = "HID\\VID_D209&PID_0430&REV_0056&MI_00";
    const DESK: &str = "HID\\VID_046D&PID_C31C&REV_6402&MI_00";

    #[test]
    fn unpublished_handle_reports_unknown_not_empty() {
        let h = PresenceHandle::new();
        assert!(h.is_supported());
        assert!(
            h.snapshot().is_none(),
            "before the first publish, presence is UNKNOWN — a supervisor must \
             not read that as 'every device was unplugged'"
        );
        h.publish(Vec::new());
        assert_eq!(h.snapshot().as_deref(), Some(&Vec::new()));
    }

    #[test]
    fn publication_is_visible_across_clones() {
        let h = PresenceHandle::new();
        let observer = h.clone();
        h.publish(vec![DeviceId::from(IPAC)]);
        assert_eq!(
            observer.snapshot().as_deref(),
            Some(&vec![DeviceId::from(IPAC)])
        );
        h.publish(Vec::new());
        assert_eq!(observer.snapshot().as_deref(), Some(&Vec::new()));
    }

    #[test]
    fn unsupported_handle_never_reports() {
        let h = PresenceHandle::unsupported();
        assert!(!h.is_supported());
        h.publish(vec![DeviceId::from(IPAC)]);
        assert!(h.snapshot().is_none());
    }

    #[test]
    fn merged_reports_the_union_only_once_every_child_has_spoken() {
        let a = PresenceHandle::new();
        let b = PresenceHandle::new();
        let merged = PresenceHandle::merged(vec![a.clone(), b.clone()]);
        assert!(merged.is_supported());

        a.publish(vec![DeviceId::from(IPAC)]);
        assert!(
            merged.snapshot().is_none(),
            "one backend still starting must read as UNKNOWN, never as \
             'the other board was unplugged'"
        );

        b.publish(vec![DeviceId::from(DESK)]);
        let snap = merged.snapshot().expect("both published");
        assert_eq!(snap.len(), 2);
        assert!(snap.contains(&DeviceId::from(IPAC)));
        assert!(snap.contains(&DeviceId::from(DESK)));

        // A removal on one child shows through the union.
        a.publish(Vec::new());
        assert_eq!(
            merged.snapshot().as_deref(),
            Some(&vec![DeviceId::from(DESK)])
        );
    }

    #[test]
    fn merged_ignores_unsupported_children_but_waits_for_live_ones() {
        let live = PresenceHandle::new();
        let merged = PresenceHandle::merged(vec![PresenceHandle::unsupported(), live.clone()]);
        assert!(merged.is_supported());
        assert!(merged.snapshot().is_none());
        live.publish(vec![DeviceId::from(IPAC)]);
        assert_eq!(
            merged.snapshot().as_deref(),
            Some(&vec![DeviceId::from(IPAC)])
        );
    }

    #[test]
    fn a_merge_of_nothing_reports_nothing() {
        let empty = PresenceHandle::merged(Vec::new());
        assert!(!empty.is_supported());
        assert!(empty.snapshot().is_none());
        // ...and publishing into a merged handle is a no-op, not a panic.
        empty.publish(vec![DeviceId::from(IPAC)]);
        assert!(empty.snapshot().is_none());

        let all_unsupported = PresenceHandle::merged(vec![PresenceHandle::unsupported()]);
        assert!(!all_unsupported.is_supported());
        assert!(all_unsupported.snapshot().is_none());
    }
}
