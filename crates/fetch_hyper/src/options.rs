// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Configuration types still exposed alongside the transport builder.
//!
//! Hyper-level knobs (pool size/idle timeout, HTTP/2 keep-alive,
//! `http2_initial_max_send_streams`, …) are configured via
//! [`HyperTransportBuilder::configure_hyper`](crate::HyperTransportBuilder::configure_hyper);
//! only the settings that drive our own logic (`TLS` connector wiring,
//! connect timeout, pool aging) live here.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// Controls whether the transport accepts plain HTTP, HTTPS, or both.
#[derive(Debug, Clone, Default, PartialEq, Eq, Copy)]
pub enum RequestFilter {
    /// Only HTTPS requests are accepted. Plain HTTP requests fail.
    #[default]
    Https,
    /// Both HTTP and HTTPS requests are accepted.
    HttpAndHttps,
}

/// Maximum wall-clock lifetime applied to each pooled connection.
///
/// When the configured cap is exceeded, the connection is poisoned and removed
/// from the pool after its in-flight request completes.
#[derive(Clone, Default)]
pub enum ConnectionLifetime {
    /// Connections may live indefinitely.
    #[default]
    Unlimited,
    /// Connections expire after a fixed duration.
    Fixed(Duration),
    /// The duration is computed per-connection via the closure.
    PerConnection(Arc<dyn Fn() -> Option<Duration> + Send + Sync + 'static>),
}

impl ConnectionLifetime {
    /// Resolves the cap for a freshly established connection.
    #[must_use]
    pub fn resolve(&self) -> Option<Duration> {
        match self {
            Self::Unlimited => None,
            Self::Fixed(duration) => Some(*duration),
            Self::PerConnection(generator) => generator(),
        }
    }
}

impl fmt::Debug for ConnectionLifetime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unlimited => f.write_str("Unlimited"),
            Self::Fixed(duration) => f.debug_tuple("Fixed").field(duration).finish(),
            Self::PerConnection(_) => f.debug_tuple("PerConnection").field(&format_args!("<closure>")).finish(),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn request_filter_default_is_https_only() {
        assert_eq!(RequestFilter::default(), RequestFilter::Https);
    }

    #[test]
    fn unlimited_resolves_to_none() {
        assert!(ConnectionLifetime::Unlimited.resolve().is_none());
        assert!(ConnectionLifetime::default().resolve().is_none());
    }

    #[test]
    fn fixed_resolves_to_duration() {
        let d = Duration::from_secs(5);
        assert_eq!(ConnectionLifetime::Fixed(d).resolve(), Some(d));
    }

    #[test]
    fn per_connection_invokes_closure_each_call() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);
        let lifetime = ConnectionLifetime::PerConnection(Arc::new(move || {
            let n = counter_clone.fetch_add(1, Ordering::Relaxed);
            Some(Duration::from_secs(u64::try_from(n + 1).unwrap()))
        }));

        assert_eq!(lifetime.resolve(), Some(Duration::from_secs(1)));
        assert_eq!(lifetime.resolve(), Some(Duration::from_secs(2)));
        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn debug_renders_per_connection_without_closure_fmt_panic() {
        let lifetime = ConnectionLifetime::PerConnection(Arc::new(|| None));
        insta::assert_debug_snapshot!(lifetime);
    }

    #[test]
    fn debug_renders_unlimited() {
        insta::assert_debug_snapshot!(ConnectionLifetime::Unlimited);
    }

    #[test]
    fn debug_renders_fixed() {
        insta::assert_debug_snapshot!(ConnectionLifetime::Fixed(Duration::from_secs(7)));
    }

    #[test]
    fn per_connection_can_return_none() {
        let lifetime = ConnectionLifetime::PerConnection(Arc::new(|| None));
        assert!(lifetime.resolve().is_none());
    }

    #[test]
    fn clone_preserves_variant() {
        let unlimited = ConnectionLifetime::Unlimited;
        assert!(matches!(unlimited, ConnectionLifetime::Unlimited));

        let fixed = ConnectionLifetime::Fixed(Duration::from_secs(3));
        match fixed {
            ConnectionLifetime::Fixed(d) => assert_eq!(d, Duration::from_secs(3)),
            _ => panic!("expected Fixed"),
        }

        let per_conn = ConnectionLifetime::PerConnection(Arc::new(|| Some(Duration::from_secs(1))));
        match per_conn {
            ConnectionLifetime::PerConnection(f) => assert_eq!(f(), Some(Duration::from_secs(1))),
            _ => panic!("expected PerConnection"),
        }
    }
}
