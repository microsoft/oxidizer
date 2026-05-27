// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bridge between an in-memory tier's eviction listener and cache telemetry.
//!
//! The cache builder is configured incrementally: storage is selected before
//! `name`/`enable_logs` may be called. We therefore install a stable listener
//! at storage-construction time that defers to a [`OnceLock`] populated when
//! the cache is finally built.

use std::sync::OnceLock;
use std::time::Duration;

use cachet_memory::RemovalCause;

use crate::cache::CacheName;
use crate::telemetry::CacheTelemetry;

/// Bridges moka crate's eviction listener to the cachet telemetry layer.
#[derive(Debug)]
pub(crate) struct EvictionHook {
    state: OnceLock<HookState>,
}

#[derive(Debug)]
struct HookState {
    telemetry: CacheTelemetry,
    name: CacheName,
}

impl EvictionHook {
    pub(crate) fn new() -> Self {
        Self { state: OnceLock::new() }
    }

    /// Binds the hook to a telemetry sink and cache name.
    ///
    /// Called once during `build_tier`. Subsequent calls are silently ignored
    /// because the hook is keyed to the first build of a builder.
    pub(crate) fn init(&self, telemetry: CacheTelemetry, name: CacheName) {
        let _ = self.state.set(HookState { telemetry, name });
    }

    /// Invoked by the in-memory tier on each removal.
    ///
    /// `Size` removals are reported as `cache.eviction` (capacity pressure) and
    /// `Expired` removals as `cache.expired` (TTL/TTI expiry) so consumers can
    /// distinguish the two. `Explicit` and `Replaced` are user-initiated and
    /// already accounted for by `cache.invalidated` / `cache.inserted`.
    pub(crate) fn handle(&self, cause: RemovalCause) {
        let Some(state) = self.state.get() else {
            return;
        };
        match cause {
            RemovalCause::Size => state.telemetry.cache_eviction(state.name, Duration::ZERO),
            RemovalCause::Expired => state.telemetry.cache_expired(state.name, Duration::ZERO),
            RemovalCause::Explicit | RemovalCause::Replaced => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use testing_aids::LogCapture;

    use super::*;
    use crate::telemetry::attributes;

    #[cfg_attr(miri, ignore)]
    #[test]
    fn handle_before_init_is_noop() {
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        let hook = EvictionHook::new();
        hook.handle(RemovalCause::Size);

        assert!(capture.output().is_empty(), "no event should fire before init");
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn handle_after_init_routes_by_cause() {
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        let hook = Arc::new(EvictionHook::new());
        hook.init(CacheTelemetry::with_logging(), "hook_test");

        hook.handle(RemovalCause::Explicit);
        hook.handle(RemovalCause::Replaced);
        assert!(
            !capture.output().contains(attributes::EVENT_EVICTION) && !capture.output().contains(attributes::EVENT_EXPIRED),
            "Explicit/Replaced must not emit eviction or expired events"
        );

        hook.handle(RemovalCause::Size);
        capture.assert_contains(attributes::EVENT_EVICTION);

        hook.handle(RemovalCause::Expired);
        capture.assert_contains(attributes::EVENT_EXPIRED);
    }
}
