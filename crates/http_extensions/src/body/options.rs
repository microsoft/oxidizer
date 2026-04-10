// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use thread_aware::ThreadAware;

/// Options for configuring body-level behavior.
///
/// This is passed to [`HttpBodyBuilder::body`] and [`HttpBodyBuilder::stream`] so that
/// the builder can apply body-specific policies such as an idle timeout.
///
/// Use [`Default::default()`] when no special behavior is needed.
///
/// # Example
///
/// ```
/// use std::time::Duration;
///
/// use http_extensions::BodyOptions;
///
/// let options = BodyOptions::default()
///     .timeout(Duration::from_secs(60));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ThreadAware)]
pub struct BodyOptions {
    pub(crate) timeout: Option<Duration>,
    pub(crate) buffer_limit: Option<usize>,
}

impl BodyOptions {
    /// Sets the body idle timeout.
    ///
    /// The timeout limits how long the consumer will wait between frames while
    /// polling the body. The timer resets every time the body yields a frame, so
    /// only idle periods (no progress) count toward the timeout.
    #[must_use]
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Sets the body buffer limit.
    ///
    /// This limits the maximum amount of memory that may be used when buffering
    /// the body via [`HttpBody::into_buffered`]. If the body exceeds this limit,
    /// an error is returned.
    #[must_use]
    pub const fn buffer_limit(mut self, limit: usize) -> Self {
        self.buffer_limit = Some(limit);
        self
    }

    /// Merges `self` with `other`, preferring values from `self` when both are set.
    pub(crate) fn merge(&self, other: &Self) -> Self {
        Self {
            timeout: self.timeout.or(other.timeout),
            buffer_limit: self.buffer_limit.or(other.buffer_limit),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn body_options_default_has_no_timeout() {
        let options = BodyOptions::default();
        assert_eq!(
            options,
            BodyOptions {
                timeout: None,
                buffer_limit: None,
            }
        );
    }

    #[test]
    fn body_options_with_timeout() {
        let options = BodyOptions::default().timeout(Duration::from_secs(60));
        assert_eq!(
            options,
            BodyOptions {
                timeout: Some(Duration::from_secs(60)),
                buffer_limit: None,
            }
        );
    }

    #[test]
    fn body_options_clone_and_copy() {
        let options = BodyOptions::default().timeout(Duration::from_secs(5));
        let cloned = options;
        let copied = options;

        assert_eq!(options, cloned);
        assert_eq!(options, copied);
    }

    #[test]
    fn body_options_debug_formatting() {
        let options = BodyOptions::default().timeout(Duration::from_secs(42));
        let debug = format!("{options:?}");
        assert!(debug.contains("BodyOptions"));
        assert!(debug.contains("42"));
    }

    #[test]
    fn body_options_with_buffer_limit() {
        let options = BodyOptions::default().buffer_limit(4096);
        assert_eq!(options.buffer_limit, Some(4096));
    }

    #[test]
    fn body_options_merge_prefers_self() {
        let a = BodyOptions::default().timeout(Duration::from_secs(10)).buffer_limit(100);
        let b = BodyOptions::default().timeout(Duration::from_secs(20)).buffer_limit(200);
        let merged = a.merge(&b);
        assert_eq!(merged, a);
    }

    #[test]
    fn body_options_merge_fills_gaps_from_other() {
        let a = BodyOptions::default().timeout(Duration::from_secs(10));
        let b = BodyOptions::default().buffer_limit(200);
        let merged = a.merge(&b);
        assert_eq!(merged, BodyOptions::default().timeout(Duration::from_secs(10)).buffer_limit(200));
    }
}
