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

/// Cloneable, thread-safe view of the devices a running backend can see.
///
/// [`PresenceHandle::unsupported`] is the "this backend cannot observe
/// hotplug" value: its snapshots are always `None`, so callers degrade to
/// never invalidating anything rather than to false removals.
#[derive(Clone, Debug)]
pub struct PresenceHandle(Option<Arc<Inner>>);

impl PresenceHandle {
    /// A live handle. Starts unpublished (`snapshot()` is `None`).
    pub fn new() -> Self {
        Self(Some(Arc::new(Inner {
            ids: ArcSwap::from_pointee(Vec::new()),
            published: AtomicBool::new(false),
        })))
    }

    /// A handle that never reports anything — for backends without hotplug
    /// visibility.
    pub fn unsupported() -> Self {
        Self(None)
    }

    pub fn is_supported(&self) -> bool {
        self.0.is_some()
    }

    /// Devices currently visible to the backend, or `None` when unsupported or
    /// nothing has been published yet. Lock-free.
    pub fn snapshot(&self) -> Option<Arc<Vec<DeviceId>>> {
        let inner = self.0.as_ref()?;
        inner
            .published
            .load(Ordering::Acquire)
            .then(|| inner.ids.load_full())
    }

    /// Publish a new device set. Backends call this from their cold path; tests
    /// call it to script hotplug through the real supervisor code path.
    pub fn publish(&self, ids: Vec<DeviceId>) {
        let Some(inner) = self.0.as_ref() else {
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
}
