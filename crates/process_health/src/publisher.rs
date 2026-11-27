// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt::{self, Debug, Formatter};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use arc_swap::ArcSwapOption;

use crate::aggregator::next_id;
use crate::component::{ComponentDispatch, ComponentOwner, ComponentShared, Slot};
use crate::{Attribute, Health, Signal};

/// One independently owned writer of a [`Component`](crate::Component)'s health.
///
/// A publisher stores its current [`Signal`] locally and forwards it to the component only when
/// it changes. Dropping a publisher queues removal of its contribution without waiting for
/// aggregation. If that drop releases the final process owner, it waits for the worker thread to
/// stop. A publisher keeps its component registered until the publisher and all component handles
/// are dropped.
///
/// A clone represents a new publisher, not another handle to the original signal. It starts
/// `Healthy` with no attributes, even if the original is `Degraded`. If only the healthy clone
/// remains after the original drops, the component can become `Healthy`.
///
/// For one publisher per worker thread, pass a [`Component`](crate::Component) to each worker and create its
/// publisher there. The component implements [`thread_aware::ThreadAware`]; the publisher does not.
///
/// # Examples
///
/// To start with a signal that is not `Healthy`, use [`Component::publisher_with`](crate::Component::publisher_with). It admits the
/// signal and its readiness reservation together. Aggregation still runs in the background.
///
/// ```
/// use health::{Aggregator, Health};
/// use observed::Sink;
///
/// # futures::executor::block_on(async {
/// let aggregator = Aggregator::new(Sink::noop());
/// let mut startup = aggregator.component("startup");
///
/// let mut publisher = startup.publisher_with(Health::Unhealthy, [("phase", "starting")]);
/// loop {
///     if startup.report().await.publisher_count(Health::Unhealthy) == 1 {
///         break;
///     }
///     startup.changed().await;
/// }
///
/// // Initialization succeeded. Keep this publisher to report later changes.
/// publisher.publish(Health::Healthy, [("phase", "running")]);
/// while startup.state() != Health::Healthy {
///     startup.changed().await;
/// }
/// # });
/// ```
pub struct Publisher {
    signal: Signal,
    slot: Arc<Slot>,
    dispatch: Arc<ComponentDispatch>,
    owner: Option<Arc<ComponentOwner>>,
}

impl Publisher {
    fn register(component: &Arc<ComponentShared>) -> Option<Self> {
        Self::register_inner(component, None, Signal::healthy())
    }

    pub(crate) fn register_owned(owner: &Arc<ComponentOwner>, signal: Signal) -> Self {
        Self::register_inner(&owner.shared, Some(Arc::clone(owner)), signal).expect("the component handle keeps its registration alive")
    }

    fn register_inner(component: &Arc<ComponentShared>, owner: Option<Arc<ComponentOwner>>, signal: Signal) -> Option<Self> {
        let registration = component.dispatch.registration.load();
        let registration = registration.as_ref()?;
        let id = next_id(&component.next_publisher);
        let slot = Arc::new(Slot {
            id,
            pending: ArcSwapOption::from_pointee(signal.clone()),
            queued: AtomicBool::new(false),
            removed: AtomicBool::new(false),
        });
        registration.admit(signal.state());
        component.dispatch.enqueue(&slot);
        Some(Self {
            signal,
            slot,
            dispatch: Arc::clone(&component.dispatch),
            owner,
        })
    }

    /// Replaces the signal of this publisher.
    ///
    /// Aggregation runs in the background and can skip intermediate signals. Publishing a signal
    /// equal to the current one does nothing.
    pub fn publish(&mut self, state: Health, attributes: impl IntoIterator<Item = impl Into<Attribute>>) {
        self.replace(Signal::new(state, attributes.into_iter().map(Into::into).collect()));
    }

    /// Publishes `Degraded` with a single `reason` attribute.
    pub fn degraded(&mut self, reason: impl Into<Arc<str>>) {
        let reason: Arc<str> = reason.into();
        self.replace(Signal::new(Health::Degraded, Box::new([Attribute::from(("reason", reason))])));
    }

    #[cfg(test)]
    pub(crate) fn register_test(component: &Arc<ComponentShared>) -> Option<Self> {
        Self::register(component)
    }

    #[cfg(test)]
    pub(crate) fn register_with_signal_test(component: &Arc<ComponentShared>, signal: Signal) -> Option<Self> {
        Self::register_inner(component, None, signal)
    }

    #[cfg(test)]
    pub(crate) const fn owner_test(&self) -> Option<&Arc<ComponentOwner>> {
        self.owner.as_ref()
    }

    #[cfg(test)]
    pub(crate) const fn slot_test(&self) -> &Arc<Slot> {
        &self.slot
    }

    #[cfg(test)]
    pub(crate) const fn dispatch_test(&self) -> &Arc<ComponentDispatch> {
        &self.dispatch
    }

    #[cfg(test)]
    pub(crate) const fn signal_test(&self) -> &Signal {
        &self.signal
    }

    fn replace(&mut self, signal: Signal) {
        if signal == self.signal {
            return;
        }
        if let Some(registration) = self.dispatch.registration.load().as_ref() {
            registration.change_health(self.signal.state(), signal.state());
            self.slot.pending.store(Some(Arc::new(signal.clone())));
            self.dispatch.enqueue(&self.slot);
        }
        self.signal = signal;
    }
}

impl Clone for Publisher {
    fn clone(&self) -> Self {
        if let Some(owner) = &self.owner {
            return Self::register_owned(owner, Signal::healthy());
        }
        self.dispatch
            .component
            .upgrade()
            .and_then(|component| Self::register(&component))
            .unwrap_or_else(|| Self {
                signal: Signal::healthy(),
                // Removal is permanent. Neither clone forwards to the slot again.
                slot: Arc::clone(&self.slot),
                dispatch: Arc::clone(&self.dispatch),
                owner: None,
            })
    }
}

impl Debug for Publisher {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("signal", &self.signal)
            .finish_non_exhaustive()
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        if let Some(registration) = self.dispatch.registration.load().as_ref() {
            registration.leave(self.signal.state());
            // Release: the worker that observes removal must not apply an older pending signal.
            self.slot.removed.store(true, Ordering::Release);
            self.dispatch.enqueue(&self.slot);
        }
    }
}
