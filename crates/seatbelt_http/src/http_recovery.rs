// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::sync::Arc;

use http_extensions::ResponseExt;
use seatbelt::{Recovery, RecoveryInfo};
use tick::Clock;

use crate::HttpResponse;

/// Configuration for classifying the recovery information of HTTP responses.
///
/// The default ([`HttpRecovery::default`]) treats the following as
/// recoverable:
///
/// - 5xx status codes (server errors)
/// - `429 Too Many Requests`
/// - Request timeouts
///
/// Customize via [`custom`][HttpRecovery::custom] or, for clock-aware
/// `Retry-After` parsing, [`custom_with_clock`][HttpRecovery::custom_with_clock].
/// Transient failures should return [`RecoveryInfo::retry`].
pub struct HttpRecovery(Inner);

impl Debug for HttpRecovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let variant = match &self.0 {
            Inner::Default => "Default",
            Inner::Custom(_) => "Custom",
        };
        f.debug_tuple("HttpRecovery").field(&variant).finish()
    }
}

impl Default for HttpRecovery {
    fn default() -> Self {
        Self(Inner::Default)
    }
}

impl HttpRecovery {
    /// Creates a custom recovery configuration.
    ///
    /// The function receives only the HTTP response. Use
    /// [`custom_with_clock`][Self::custom_with_clock] if you also need a
    /// [`Clock`] (e.g. for clock-aware `Retry-After` parsing).
    #[must_use]
    pub fn custom(recovery: impl Fn(&HttpResponse) -> RecoveryInfo + Send + Sync + 'static) -> Self {
        Self(Inner::Custom(Arc::new(move |response, _clock| recovery(response))))
    }

    /// Creates a custom recovery configuration that also receives the
    /// [`Clock`].
    ///
    /// Useful for clock-aware `Retry-After` parsing (see
    /// [`ResponseExt::recovery_with_clock`][http_extensions::ResponseExt::recovery_with_clock]).
    #[must_use]
    pub fn custom_with_clock(recovery: impl Fn(&HttpResponse, &Clock) -> RecoveryInfo + Send + Sync + 'static) -> Self {
        Self(Inner::Custom(Arc::new(recovery)))
    }

    pub(crate) fn recovery(&self, response: &HttpResponse, clock: &Clock) -> RecoveryInfo {
        match &self.0 {
            Inner::Default => response.recovery_with_clock(clock),
            Inner::Custom(f) => f(response, clock),
        }
    }
}

/// Classifies the recovery info of an HTTP result (response or error).
///
/// Shared by [`retry`][super::retry] and [`breaker`][super::breaker] modules.
pub(super) fn detect_recovery(result: &http_extensions::Result<HttpResponse>, recovery: &HttpRecovery, clock: &Clock) -> RecoveryInfo {
    match result {
        Ok(response) => recovery.recovery(response, clock),
        Err(error) => error.recovery(),
    }
}

impl<F> From<F> for HttpRecovery
where
    F: Fn(&HttpResponse) -> RecoveryInfo + Send + Sync + 'static,
{
    fn from(f: F) -> Self {
        Self::custom(f)
    }
}

type CustomDelegate = Arc<dyn Fn(&HttpResponse, &Clock) -> RecoveryInfo + Send + Sync>;

enum Inner {
    Default,
    Custom(CustomDelegate),
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use http::StatusCode;
    use http_extensions::HttpResponseBuilder;
    use seatbelt::RecoveryKind;

    use super::*;

    #[test]
    fn default_recovery() {
        let http_recovery = HttpRecovery::default();
        let mut response = HttpResponseBuilder::new_fake().build().unwrap();
        let clock = Clock::new_frozen();

        *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        assert_eq!(http_recovery.recovery(&response, &clock).kind(), RecoveryKind::Retry);

        *response.status_mut() = StatusCode::BAD_REQUEST;
        assert_eq!(http_recovery.recovery(&response, &clock).kind(), RecoveryKind::Never);
    }

    #[test]
    fn custom_recovery() {
        let http_recovery = HttpRecovery::from(|response: &HttpResponse| {
            if response.status() == StatusCode::BAD_REQUEST {
                RecoveryInfo::retry()
            } else {
                RecoveryInfo::never()
            }
        });
        let response = HttpResponseBuilder::new_fake().status(StatusCode::BAD_REQUEST).build().unwrap();
        assert_eq!(http_recovery.recovery(&response, &Clock::new_frozen()).kind(), RecoveryKind::Retry);
    }

    #[test]
    fn custom_recovery_with_clock() {
        let http_recovery = HttpRecovery::custom_with_clock(http_extensions::ResponseExt::recovery_with_clock);
        let response = HttpResponseBuilder::new_fake()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header("Retry-After", "42")
            .build()
            .unwrap();
        let clock = Clock::new_frozen();
        let recovery = http_recovery.recovery(&response, &clock);

        assert_eq!(recovery.kind(), RecoveryKind::Retry);
        assert_eq!(recovery.get_delay(), Some(Duration::from_secs(42)));
    }

    /// Verifies that `custom_with_clock` actually stores the provided closure
    /// rather than falling back to default behavior. A 400 Bad Request is
    /// `RecoveryKind::Never` under the default policy; the custom closure
    /// overrides it to `Retry`, proving the closure is invoked.
    #[test]
    fn custom_with_clock_uses_provided_closure() {
        let http_recovery = HttpRecovery::custom_with_clock(|response, _clock| {
            if response.status() == StatusCode::BAD_REQUEST {
                RecoveryInfo::retry()
            } else {
                RecoveryInfo::never()
            }
        });

        let response = HttpResponseBuilder::new_fake().status(StatusCode::BAD_REQUEST).build().unwrap();
        let clock = Clock::new_frozen();

        // Default would classify 400 as `Never`; our custom closure classifies it as `Retry`.
        assert_eq!(http_recovery.recovery(&response, &clock).kind(), RecoveryKind::Retry);
    }

    #[test]
    fn debug_distinguishes_default_and_custom() {
        assert_eq!(format!("{:?}", HttpRecovery::default()), "HttpRecovery(\"Default\")");
        assert_eq!(
            format!("{:?}", HttpRecovery::custom(|_| RecoveryInfo::never())),
            "HttpRecovery(\"Custom\")"
        );
        assert_eq!(
            format!("{:?}", HttpRecovery::custom_with_clock(|_, _| RecoveryInfo::never())),
            "HttpRecovery(\"Custom\")"
        );
    }

    #[test]
    fn default_recovery_respects_retry_after() {
        let http_recovery = HttpRecovery::default();
        let response = HttpResponseBuilder::new_fake()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header("Retry-After", "10")
            .build()
            .unwrap();
        let clock = Clock::new_frozen();
        let recovery = http_recovery.recovery(&response, &clock);

        assert_eq!(recovery.kind(), RecoveryKind::Retry);
        assert_eq!(recovery.get_delay(), Some(Duration::from_secs(10)));
    }
}
