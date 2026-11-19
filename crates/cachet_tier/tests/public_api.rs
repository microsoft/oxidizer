// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache tier errors, shared state, and mock failures through public APIs.

#[cfg(feature = "test-util")]
mod dynamic {
    use cachet_tier::{CacheEntry, CacheTier, DynamicCache, InsertOutcome, MockCache};

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn clone_shares_state() {
        let cache = MockCache::<String, i32>::new();
        let dynamic = DynamicCache::new(cache);
        let clone = dynamic.clone();

        let outcome = dynamic.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        assert_eq!(outcome, InsertOutcome::Accepted);

        let entry = clone.get(&"key".to_string()).await.unwrap().unwrap();
        assert_eq!(*entry.value(), 42);
    }
}

mod error {
    use std::error::Error as _;
    use std::io::{self, ErrorKind};

    use cachet_tier::{Error, Result, SizeError, SizeErrorKind};
    use recoverable::{Recovery, RecoveryInfo};

    #[test]
    fn error_debug_contains_cause_message() {
        let error = Error::from_message("test error message");
        let debug_str = format!("{error:?}");
        assert!(
            debug_str.contains("test error message"),
            "debug output should contain the cause message, got: {debug_str}"
        );
    }

    #[test]
    fn error_display_contains_cause_message() {
        let error = Error::from_message("display test");
        let display_str = format!("{error}");
        assert!(
            display_str.contains("display test"),
            "display output should contain the cause message, got: {display_str}"
        );
    }

    #[test]
    fn result_type_alias_propagates_errors() {
        fn returns_err() -> Result<i32> {
            Err(Error::from_message("expected failure"))
        }

        let err = returns_err().unwrap_err();
        assert!(format!("{err}").contains("expected failure"));
    }

    #[test]
    fn from_source_preserves_error_type() {
        let io_err = io::Error::new(ErrorKind::ConnectionRefused, "connection refused");
        let error = Error::from_source(io_err);

        assert!(error.is_source::<io::Error>());
        let extracted = error.source_as::<io::Error>().unwrap();
        assert_eq!(extracted.kind(), ErrorKind::ConnectionRefused);
    }

    #[test]
    fn is_source_returns_false_for_wrong_type() {
        let io_err = io::Error::new(ErrorKind::NotFound, "not found");
        let error = Error::from_source(io_err);

        assert!(error.is_source::<io::Error>());
        assert!(!error.is_source::<std::fmt::Error>());
    }

    #[test]
    fn source_as_returns_none_for_wrong_type() {
        let io_err = io::Error::new(ErrorKind::NotFound, "not found");
        let error = Error::from_source(io_err);

        assert!(error.source_as::<io::Error>().is_some());
        assert!(error.source_as::<std::fmt::Error>().is_none());
    }

    #[test]
    fn source_as_returns_none_for_message_only_error() {
        let error = Error::from_message("just a message");

        assert!(!error.is_source::<io::Error>());
        assert!(error.source_as::<io::Error>().is_none());
    }

    #[test]
    fn error_is_clone() {
        let io_err = io::Error::new(ErrorKind::TimedOut, "timeout");
        let error = Error::from_source(io_err);
        let cloned = error.clone();

        // Both should have the same source type
        assert!(error.is_source::<io::Error>());
        assert!(cloned.is_source::<io::Error>());

        // Both should display the same message
        assert_eq!(error.to_string(), cloned.to_string());
    }

    #[test]
    fn error_extract_and_match_on_kind() {
        let io_err = io::Error::new(ErrorKind::PermissionDenied, "access denied");
        let error = Error::from_source(io_err);

        assert_eq!(
            error.source_as::<io::Error>().map(io::Error::kind),
            Some(ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn with_recovery_attaches_recovery_info() {
        let error = Error::from_message("temporary failure").with_recovery(RecoveryInfo::retry());
        assert_eq!(error.recovery(), RecoveryInfo::retry());
    }

    #[test]
    fn recovery_returns_never_by_default() {
        let error = Error::from_message("permanent failure");
        assert_eq!(error.recovery(), RecoveryInfo::never());
    }

    #[test]
    fn failed_size_error_from_error() {
        let error = Error::from_message("permanent failure");
        let len_err: SizeError = error.clone().into();
        assert_eq!(SizeErrorKind::Failed, len_err.kind);
        assert_eq!(error.to_string(), len_err.source().unwrap().to_string());
    }

    #[test]
    fn unsupported_size_error_has_unsupported_kind() {
        let len_err = SizeError::unsupported();
        assert_eq!(SizeErrorKind::Unsupported, len_err.kind);
    }

    #[test]
    fn size_error_converts_to_error() {
        let len_err = SizeError::unsupported();
        let error: Error = len_err.into();
        assert!(error.is_source::<SizeError>());
    }
}

#[cfg(feature = "test-util")]
mod testing {
    use cachet_tier::{CacheEntry, CacheOp, CacheTier, MockCache};

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn insert_failure() {
        let cache = MockCache::<String, i32>::new();
        cache.fail_when(|op| matches!(op, CacheOp::Insert { .. }));
        let result = cache.insert("key".to_string(), CacheEntry::new(42)).await;
        result.unwrap_err();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn invalidate_failure() {
        let cache = MockCache::<String, i32>::new();
        cache.fail_when(|op| matches!(op, CacheOp::Invalidate(_)));
        let result = cache.invalidate(&"key".to_string()).await;
        result.unwrap_err();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn clear_failure() {
        let cache = MockCache::<String, i32>::new();
        cache.fail_when(|op| matches!(op, CacheOp::Clear));
        let result = cache.clear().await;
        result.unwrap_err();
    }
}
