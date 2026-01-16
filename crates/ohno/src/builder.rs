use std::error::Error as StdError;

use crate::OhnoCore;
use crate::source::Source;

/// Policy for capturing backtraces in errors.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum BacktracePolicy {
    /// Automatically decide based on the RUST_BACKTRACE environment variable.
    #[default]
    Auto,
    /// Force backtrace capture even if the RUST_BACKTRACE environment variable is not set or set to 0.
    Forced,
    /// Never capture backtraces, regardless of the RUST_BACKTRACE environment variable.
    Never,
}

/// Builder for creating [`OhnoCore`] instances with custom configurations.
#[derive(Debug)]
pub struct OhnoCoreBuilder {
    pub(crate) backtrace_policy: BacktracePolicy,
    pub(crate) error: Source,
}

impl Default for OhnoCoreBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl OhnoCoreBuilder {
    /// Creates a new [`OhnoCoreBuilder`] with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            backtrace_policy: BacktracePolicy::Auto,
            error: Source::None,
        }
    }

    /// Sets the backtrace capture policy.
    #[must_use]
    pub fn backtrace_policy(mut self, policy: BacktracePolicy) -> Self {
        self.backtrace_policy = policy;
        self
    }

    /// Sets the main error for the [`OhnoCore`] instance.
    #[must_use]
    pub fn error<E>(mut self, error: E) -> Self
    where
        E: Into<Box<dyn StdError + Send + Sync + 'static>>,
    {
        self.error = if is_string_error(&error) {
            Source::Transparent(error.into().into())
        } else {
            Source::Error(error.into().into())
        };
        self
    }

    /// Transparent errors delegate [`source`](StdError::source) calls to the inner error.
    ///
    /// Typically it's used with private error types that are not exposed to the users of the
    /// library.
    #[must_use]
    pub fn transparent_error<E>(mut self, error: E) -> Self
    where
        E: Into<Box<dyn StdError + Send + Sync + 'static>>,
    {
        self.error = Source::Transparent(error.into().into());
        self
    }

    /// Builds the [`OhnoCore`] instance.
    #[must_use]
    pub fn build(self) -> OhnoCore {
        OhnoCore::from_builder(self)
    }
}

const STR_TYPE_IDS: [typeid::ConstTypeId; 3] = [
    typeid::ConstTypeId::of::<&str>(),
    typeid::ConstTypeId::of::<String>(),
    typeid::ConstTypeId::of::<std::borrow::Cow<'_, str>>(),
];

fn is_string_error<T>(_: &T) -> bool {
    let typeid_of_t = typeid::of::<T>();
    STR_TYPE_IDS.iter().any(|&id| id == typeid_of_t)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    #[test]
    fn is_string_error_test() {
        assert!(is_string_error(&"a string slice"));
        assert!(is_string_error(&String::from("a string")));
        assert!(is_string_error(&Cow::Borrowed("a string slice")));
        assert!(is_string_error(&Cow::<'static, str>::Owned(String::from("a string"))));
        assert!(!is_string_error(&std::io::Error::other("an io error")));
    }
}
