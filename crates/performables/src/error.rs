// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

/// An opaque ownership or synchronization error.
///
/// Predicate methods distinguish failure conditions without exposing the
/// error's representation. Failed channel sends retain their rejected value;
/// poisoned lock acquisitions retain their acquired guard. These errors use
/// `Error<T>` with the recovery value or guard as `T`. Other errors use
/// `Error`, whose recovery value is `()`.
///
/// A recovered guard still holds its lock. Repair the protected value and call
/// the lock's `clear_poison` method before resuming normal use. Dropping the
/// guard alone does not clear poisoning.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Error<T = ()> {
    kind: ErrorKind,
    value: T,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ErrorKind {
    ForeignOwner,
    Duplicate,
    MissingCurrent,
    Poisoned,
    Full,
    Empty,
    Closed,
    SendClosed,
    Timeout,
}

impl<T> Error<T> {
    pub(crate) const fn with_value(kind: ErrorKind, value: T) -> Self {
        Self {
            kind: match kind {
                ErrorKind::Closed => ErrorKind::SendClosed,
                kind => kind,
            },
            value,
        }
    }

    pub(crate) const fn poisoned(guard: T) -> Self {
        Self::with_value(ErrorKind::Poisoned, guard)
    }

    /// Returns whether a supplied thread belongs to a different runtime owner.
    #[must_use]
    pub const fn is_foreign_owner(&self) -> bool {
        matches!(self.kind, ErrorKind::ForeignOwner)
    }

    /// Returns whether multiple values were supplied for the same affinity partition.
    #[must_use]
    pub const fn is_duplicate(&self) -> bool {
        matches!(self.kind, ErrorKind::Duplicate)
    }

    /// Returns whether no value was supplied for the current affinity partition.
    #[must_use]
    pub const fn is_missing_current(&self) -> bool {
        matches!(self.kind, ErrorKind::MissingCurrent)
    }

    /// Returns whether a lock was acquired after another thread poisoned it.
    #[must_use]
    pub const fn is_poisoned(&self) -> bool {
        matches!(self.kind, ErrorKind::Poisoned)
    }

    /// Returns whether a bounded channel had no available capacity.
    #[must_use]
    pub const fn is_full(&self) -> bool {
        matches!(self.kind, ErrorKind::Full)
    }

    /// Returns whether a channel had no value available without waiting.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        matches!(self.kind, ErrorKind::Empty)
    }

    /// Returns whether the channel no longer accepts the operation.
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        matches!(self.kind, ErrorKind::Closed | ErrorKind::SendClosed)
    }

    /// Returns whether a blocking channel operation reached its deadline.
    #[must_use]
    pub const fn is_timeout(&self) -> bool {
        matches!(self.kind, ErrorKind::Timeout)
    }

    /// Returns a shared reference to the rejected value or acquired guard.
    ///
    /// Errors without a recovery value return a reference to `()`.
    #[must_use]
    pub const fn get_ref(&self) -> &T {
        &self.value
    }

    /// Returns a mutable reference to the rejected value or acquired guard.
    ///
    /// Errors without a recovery value return a reference to `()`.
    #[must_use]
    pub const fn get_mut(&mut self) -> &mut T {
        &mut self.value
    }

    /// Consumes the error and returns the rejected value or acquired guard.
    ///
    /// Errors without a recovery value return `()`.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.value
    }

    /// Consumes the error and returns its recovery value or guard, if present.
    #[must_use]
    pub fn into_value(self) -> Option<T> {
        match self.kind {
            ErrorKind::Poisoned | ErrorKind::Full | ErrorKind::SendClosed => Some(self.value),
            _ => None,
        }
    }
}

impl Error {
    pub(crate) const fn without_value(kind: ErrorKind) -> Self {
        Self { kind, value: () }
    }
}

impl<T> fmt::Display for Error<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.kind {
            ErrorKind::ForeignOwner => "a supplied thread belongs to a different runtime owner",
            ErrorKind::Duplicate => "multiple values were supplied for the same affinity partition",
            ErrorKind::MissingCurrent => "no value was supplied for the current affinity partition",
            ErrorKind::Poisoned => "poisoned lock: another thread panicked while holding the lock",
            ErrorKind::Full => "channel has no available capacity",
            ErrorKind::Empty => "channel has no value available",
            ErrorKind::Closed | ErrorKind::SendClosed => "channel is closed",
            ErrorKind::Timeout => "channel operation timed out",
        })
    }
}

impl<T> fmt::Debug for Error<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error").finish_non_exhaustive()
    }
}

impl<T> std::error::Error for Error<T> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn classifications_and_messages_distinguish_failures() {
        let cases = [
            (ErrorKind::ForeignOwner, "a supplied thread belongs to a different runtime owner", 0),
            (
                ErrorKind::Duplicate,
                "multiple values were supplied for the same affinity partition",
                1,
            ),
            (
                ErrorKind::MissingCurrent,
                "no value was supplied for the current affinity partition",
                2,
            ),
            (
                ErrorKind::Poisoned,
                "poisoned lock: another thread panicked while holding the lock",
                3,
            ),
            (ErrorKind::Full, "channel has no available capacity", 4),
            (ErrorKind::Empty, "channel has no value available", 5),
            (ErrorKind::Closed, "channel is closed", 6),
            (ErrorKind::SendClosed, "channel is closed", 6),
            (ErrorKind::Timeout, "channel operation timed out", 7),
        ];

        for (kind, message, predicate) in cases {
            let error = Error::without_value(kind);
            let flags = [
                error.is_foreign_owner(),
                error.is_duplicate(),
                error.is_missing_current(),
                error.is_poisoned(),
                error.is_full(),
                error.is_empty(),
                error.is_closed(),
                error.is_timeout(),
            ];
            assert_eq!(flags, std::array::from_fn(|index| index == predicate));
            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn recovery_value_can_be_inspected_repaired_and_consumed() {
        let mut error = Error::poisoned(String::from("value"));
        assert_eq!(error.get_ref(), "value");
        error.get_mut().push_str("-repaired");
        assert_eq!(error.into_inner(), "value-repaired");
    }

    #[test]
    fn optional_recovery_distinguishes_unit_values_from_no_value() {
        assert_eq!(Error::with_value(ErrorKind::Closed, ()).into_value(), Some(()));
        assert_eq!(Error::without_value(ErrorKind::Closed).into_value(), None);
        assert_eq!(Error::with_value(ErrorKind::Full, 7).into_value(), Some(7));
        assert_eq!(Error::poisoned(11).into_value(), Some(11));
    }

    #[test]
    fn debug_does_not_disclose_the_recovery_value() {
        struct NotDebug;
        let error = Error::with_value(ErrorKind::Full, NotDebug);
        assert_eq!(format!("{error:?}"), "Error { .. }");
        let error: &dyn std::error::Error = &error;
        assert!(error.source().is_none());
    }
}
