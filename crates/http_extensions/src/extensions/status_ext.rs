// Copyright (c) Microsoft Corporation.

use http::{Response, StatusCode};
use recoverable::RecoveryInfo;

use crate::{HttpError, Result};

/// Status code validation and recovery classification.
///
/// Provides methods to validate status codes and determine recovery strategies.
/// Implemented for both [`StatusCode`] and [`Response<B>`].
pub trait StatusExt: sealed::Sealed
where
    Self: Sized,
{
    /// Ensures that the status code is a successful status code (2xx range).
    ///
    /// # Returns
    ///
    /// - `Ok(self)` if the status code indicates success
    /// - `Err(Error)` if the status code indicates failure
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use http_extensions::StatusExt;
    ///
    /// #[derive(Debug)]
    /// struct MyError(StatusCode);
    ///
    /// let status = StatusCode::BAD_REQUEST;
    /// let result = status.ensure_success();
    /// assert!(result.is_err());
    /// ```
    fn ensure_success(self) -> Result<Self>;

    /// Ensures that the status code is a successful status code (2xx range)
    /// and returns a custom error if it is not.
    ///
    /// This allows you to provide your own error type and construction logic.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use http_extensions::StatusExt;
    ///
    /// #[derive(Debug)]
    /// struct MyError(StatusCode);
    ///
    /// impl std::fmt::Display for MyError {
    ///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    ///         write!(f, "Request failed with status: {}", self.0)
    ///     }
    /// }
    ///
    /// impl std::error::Error for MyError {}
    ///
    /// let status = StatusCode::BAD_REQUEST;
    /// let result = status.ensure_success_with(|s| MyError(s));
    /// assert!(result.is_err());
    /// ```
    fn ensure_success_with<F, E>(self, factory: F) -> std::result::Result<Self, E>
    where
        F: FnOnce(StatusCode) -> E;

    /// Returns the recovery metadata for this status code.
    ///
    /// A status is considered recoverable when:
    ///
    /// - It is in the 5xx range (server errors), or
    /// - It is `429 Too Many Requests`, or
    /// - It is `408 Request Timeout`.
    ///
    /// Other statuses return [`RecoveryInfo::never`][recoverable::RecoveryInfo::never]
    ///
    /// # Examples
    ///
    /// With a status code:
    /// ```
    /// use http::StatusCode;
    /// use http_extensions::StatusExt;
    /// use recoverable::{Recovery, RecoveryKind};
    ///
    /// assert_eq!(
    ///     StatusCode::INTERNAL_SERVER_ERROR.recovery().kind(),
    ///     RecoveryKind::Retry
    /// ); // 5xx
    /// assert_eq!(
    ///     StatusCode::TOO_MANY_REQUESTS.recovery().kind(),
    ///     RecoveryKind::Retry
    /// ); // 429
    /// assert_eq!(
    ///     StatusCode::REQUEST_TIMEOUT.recovery().kind(),
    ///     RecoveryKind::Retry
    /// ); // 408
    ///
    /// assert_eq!(
    ///     StatusCode::BAD_REQUEST.recovery().kind(),
    ///     RecoveryKind::Never
    /// ); // 400
    /// assert_eq!(StatusCode::OK.recovery().kind(), RecoveryKind::Never); // 200
    /// ```
    ///
    /// With a response:
    /// ```
    /// use http::{Response, StatusCode};
    /// use http_extensions::StatusExt;
    /// use recoverable::RecoveryKind;
    ///
    /// let resp = Response::builder()
    ///     .status(StatusCode::SERVICE_UNAVAILABLE)
    ///     .body(())
    ///     .unwrap();
    /// assert_eq!(resp.recovery().kind(), RecoveryKind::Retry);
    /// ```
    fn recovery(&self) -> RecoveryInfo;
}

impl StatusExt for StatusCode {
    fn ensure_success(self) -> Result<Self> {
        if self.is_success() {
            Ok(self)
        } else {
            Err(HttpError::invalid_status_code(self, RecoveryInfo::never()))
        }
    }

    fn ensure_success_with<F, E>(self, factory: F) -> std::result::Result<Self, E>
    where
        F: FnOnce(Self) -> E,
    {
        if self.is_success() {
            Ok(self)
        } else {
            Err(factory(self))
        }
    }

    #[cfg_attr(test, mutants::skip)] // Causes test timeouts, but it's well tested.
    fn recovery(&self) -> RecoveryInfo {
        match self {
            s if s >= &Self::INTERNAL_SERVER_ERROR => RecoveryInfo::retry(),
            &Self::TOO_MANY_REQUESTS | &Self::REQUEST_TIMEOUT => RecoveryInfo::retry(),
            _ => RecoveryInfo::never(),
        }
    }
}

mod sealed {
    use super::*;

    #[expect(unnameable_types, reason = "intentional, sealed trait pattern")]
    pub trait Sealed {}
    impl Sealed for StatusCode {}
    impl<B> Sealed for Response<B> {}
}

impl<B> StatusExt for Response<B> {
    fn ensure_success(self) -> Result<Self> {
        match self.status().ensure_success() {
            Ok(_) => Ok(self),
            Err(e) => Err(e),
        }
    }

    fn ensure_success_with<F, E>(self, factory: F) -> std::result::Result<Self, E>
    where
        F: FnOnce(StatusCode) -> E,
    {
        match self.status().ensure_success_with(factory) {
            Ok(_) => Ok(self),
            Err(e) => Err(e),
        }
    }

    fn recovery(&self) -> RecoveryInfo {
        self.status().recovery()
    }
}

#[cfg(test)]
mod tests {
    use recoverable::Recovery;

    use super::*;

    #[test]
    fn test_ensure_success_with_2xx_status_returns_ok() {
        assert_eq!(StatusCode::OK.ensure_success().unwrap(), StatusCode::OK);
        assert_eq!(
            StatusCode::CREATED.ensure_success().unwrap(),
            StatusCode::CREATED
        );
        assert_eq!(
            StatusCode::ACCEPTED.ensure_success().unwrap(),
            StatusCode::ACCEPTED
        );
        assert_eq!(
            StatusCode::NO_CONTENT.ensure_success().unwrap(),
            StatusCode::NO_CONTENT
        );
    }

    #[test]
    fn test_ensure_success_with_4xx_status_fails() {
        let error = StatusCode::BAD_REQUEST.ensure_success().unwrap_err();
        assert!(format!("{error}").contains("400"));
        assert_eq!(error.recovery(), RecoveryInfo::never());
    }

    #[test]
    fn test_ensure_success_with_5xx_status_fails() {
        let error = StatusCode::INTERNAL_SERVER_ERROR
            .ensure_success()
            .unwrap_err();
        assert!(format!("{error}").contains("500"));
        assert_eq!(error.recovery(), RecoveryInfo::never());
    }

    #[test]
    fn test_ensure_success_with_3xx_status_fails() {
        StatusCode::MOVED_PERMANENTLY.ensure_success().unwrap_err();
    }

    #[test]
    fn test_ensure_success_with_custom_error_succeeds() {
        let result = StatusCode::OK.ensure_success_with(|s| format!("Failed: {s}"));
        assert_eq!(result.unwrap(), StatusCode::OK);
    }

    #[test]
    fn test_ensure_success_with_custom_error_fails() {
        #[derive(Debug, PartialEq)]
        struct CustomError(StatusCode);

        let error = StatusCode::BAD_REQUEST
            .ensure_success_with(CustomError)
            .unwrap_err();
        assert_eq!(error, CustomError(StatusCode::BAD_REQUEST));
    }

    #[test]
    fn test_ensure_success_with_string_error_fails() {
        let error = StatusCode::NOT_FOUND
            .ensure_success_with(|s| format!("Status {s}"))
            .unwrap_err();
        assert_eq!(error, "Status 404 Not Found");
    }

    #[test]
    fn test_response_ensure_success_with_2xx_returns_response() {
        let response = Response::builder()
            .status(StatusCode::OK)
            .body("success")
            .unwrap();
        let result = response.ensure_success().unwrap();
        assert_eq!(result.status(), StatusCode::OK);
        assert_eq!(result.body(), &"success");
    }

    #[test]
    fn test_response_ensure_success_with_4xx_fails() {
        let response = Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(())
            .unwrap();
        let error = response.ensure_success().unwrap_err();
        assert!(format!("{error}").contains("400"));
    }

    #[test]
    fn test_response_ensure_success_with_5xx_fails() {
        let response = Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body("server error")
            .unwrap();
        let error = response.ensure_success().unwrap_err();
        assert!(format!("{error}").contains("500"));
    }

    #[test]
    fn test_response_ensure_success_with_custom_error_succeeds() {
        let response = Response::builder()
            .status(StatusCode::ACCEPTED)
            .body("accepted")
            .unwrap();
        let result = response
            .ensure_success_with(|s| format!("Failed: {s}"))
            .unwrap();
        assert_eq!(result.status(), StatusCode::ACCEPTED);
        assert_eq!(result.body(), &"accepted");
    }

    #[test]
    fn test_response_ensure_success_with_custom_error_fails() {
        #[derive(Debug, PartialEq)]
        struct ResponseError(StatusCode);

        let response = Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body("forbidden")
            .unwrap();
        let error = response.ensure_success_with(ResponseError).unwrap_err();
        assert_eq!(error, ResponseError(StatusCode::FORBIDDEN));
    }

    #[test]
    fn test_ensure_success_with_1xx_status_fails() {
        StatusCode::CONTINUE.ensure_success().unwrap_err();
    }

    #[test]
    fn test_ensure_success_with_boundary_2xx_statuses() {
        StatusCode::from_u16(200).unwrap().ensure_success().unwrap();
        StatusCode::from_u16(299).unwrap().ensure_success().unwrap();
        StatusCode::from_u16(300)
            .unwrap()
            .ensure_success()
            .unwrap_err();
    }

    #[test]
    fn test_response_ensure_success_preserves_headers() {
        let response = Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .header("X-Custom", "test")
            .body("test")
            .unwrap();
        let result = response.ensure_success().unwrap();
        assert_eq!(
            result.headers().get("Content-Type").unwrap(),
            "application/json"
        );
        assert_eq!(result.headers().get("X-Custom").unwrap(), "test");
    }

    #[test]
    fn test_ensure_success_with_all_2xx_range_succeeds() {
        StatusCode::PARTIAL_CONTENT.ensure_success().unwrap();
        StatusCode::MULTI_STATUS.ensure_success().unwrap();
        StatusCode::ALREADY_REPORTED.ensure_success().unwrap();
        StatusCode::IM_USED.ensure_success().unwrap();
    }

    #[test]
    fn test_ensure_success_with_boundary_status_codes() {
        StatusCode::from_u16(199)
            .unwrap()
            .ensure_success()
            .unwrap_err();
        StatusCode::from_u16(200).unwrap().ensure_success().unwrap();
        StatusCode::from_u16(299).unwrap().ensure_success().unwrap();
        StatusCode::from_u16(300)
            .unwrap()
            .ensure_success()
            .unwrap_err();
    }

    #[test]
    fn test_ensure_success_with_uncommon_status_codes() {
        StatusCode::IM_A_TEAPOT.ensure_success().unwrap_err();
        StatusCode::UPGRADE_REQUIRED.ensure_success().unwrap_err();
        StatusCode::NETWORK_AUTHENTICATION_REQUIRED
            .ensure_success()
            .unwrap_err();
    }

    #[test]
    fn test_ensure_success_with_function_pointer() {
        fn create_error(status: StatusCode) -> String {
            format!("Error: {status}")
        }

        let error = StatusCode::UNAUTHORIZED
            .ensure_success_with(create_error)
            .unwrap_err();
        assert_eq!(error, "Error: 401 Unauthorized");
    }

    #[test]
    fn test_response_ensure_success_with_different_body_types() {
        let json_response = Response::builder()
            .status(StatusCode::OK)
            .body(b"[1,2,3]".to_vec())
            .unwrap();
        let result = json_response.ensure_success().unwrap();
        assert_eq!(result.body(), &vec![91, 49, 44, 50, 44, 51, 93]);

        let bytes_response = Response::builder()
            .status(StatusCode::ACCEPTED)
            .body(&b"binary data"[..])
            .unwrap();
        bytes_response.ensure_success().unwrap();
    }

    #[test]
    fn test_response_ensure_success_with_empty_body() {
        let response = Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Vec::<u8>::new())
            .unwrap();
        let result = response.ensure_success().unwrap();
        assert!(result.body().is_empty());
    }

    #[test]
    fn test_ensure_success_with_closure_capturing_context() {
        let context = "request_id_123";
        let error = StatusCode::SERVICE_UNAVAILABLE
            .ensure_success_with(|status| format!("{context}: {status}"))
            .unwrap_err();
        assert_eq!(error, "request_id_123: 503 Service Unavailable");
    }

    #[test]
    fn test_is_transient_for_5xx_and_specific_4xx() {
        use recoverable::RecoveryKind;

        // 5xx are transient
        assert_eq!(
            StatusCode::INTERNAL_SERVER_ERROR.recovery().kind(),
            RecoveryKind::Retry
        );
        assert_eq!(
            StatusCode::BAD_GATEWAY.recovery().kind(),
            RecoveryKind::Retry
        );
        assert_eq!(
            StatusCode::SERVICE_UNAVAILABLE.recovery().kind(),
            RecoveryKind::Retry
        );
        assert_eq!(
            StatusCode::GATEWAY_TIMEOUT.recovery().kind(),
            RecoveryKind::Retry
        );

        // Specific 4xx
        assert_eq!(
            StatusCode::TOO_MANY_REQUESTS.recovery().kind(),
            RecoveryKind::Retry
        );
        assert_eq!(
            StatusCode::REQUEST_TIMEOUT.recovery().kind(),
            RecoveryKind::Retry
        );

        // Common non-transient cases
        assert_eq!(
            StatusCode::BAD_REQUEST.recovery().kind(),
            RecoveryKind::Never
        );
        assert_eq!(
            StatusCode::UNAUTHORIZED.recovery().kind(),
            RecoveryKind::Never
        );
        assert_eq!(StatusCode::FORBIDDEN.recovery().kind(), RecoveryKind::Never);
        assert_eq!(StatusCode::NOT_FOUND.recovery().kind(), RecoveryKind::Never);
        assert_eq!(StatusCode::OK.recovery().kind(), RecoveryKind::Never);
    }

    #[test]
    fn test_is_transient_boundaries() {
        use recoverable::RecoveryKind;

        // 499 is not transient
        let s499 = StatusCode::from_u16(499).unwrap();
        assert_eq!(s499.recovery().kind(), RecoveryKind::Never);

        // 500 is transient
        let s500 = StatusCode::from_u16(500).unwrap();
        assert_eq!(s500.recovery().kind(), RecoveryKind::Retry);
    }

    #[test]
    fn test_response_is_transient_delegates() {
        use recoverable::RecoveryKind;

        let resp = Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(())
            .unwrap();
        assert_eq!(resp.recovery().kind(), RecoveryKind::Retry);

        let resp = Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(())
            .unwrap();
        assert_eq!(resp.recovery().kind(), RecoveryKind::Never);
    }
}
