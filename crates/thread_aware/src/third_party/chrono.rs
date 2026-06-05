// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ThreadAware` impls for [`chrono`] types.
//!
//! Enable with the `chrono` Cargo feature.
//!
//! The listed types are inert: they store calendar/time values inline and do
//! not hold cross-thread state. `chrono::Local` is intentionally omitted
//! because it requires `chrono`'s `clock` feature, which is not enabled by
//! default in this workspace.

use ::chrono::{DateTime, FixedOffset, Month, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Utc, Weekday};

impl_noop_thread_aware!(
    NaiveDate,
    NaiveTime,
    NaiveDateTime,
    TimeDelta,
    Utc,
    FixedOffset,
    Weekday,
    Month,
    DateTime<Utc>,
    DateTime<FixedOffset>,
);

#[cfg(test)]
mod tests {
    use ::chrono::{DateTime, FixedOffset, Month, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Utc, Weekday};
    use static_assertions::assert_impl_all;

    use crate::ThreadAware;
    use crate::affinity::pinned_affinities;

    assert_impl_all!(NaiveDate: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(NaiveTime: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(NaiveDateTime: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(TimeDelta: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(Utc: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(FixedOffset: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(Weekday: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(Month: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(DateTime<Utc>: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(DateTime<FixedOffset>: ThreadAware, Send, Sync, Copy);

    #[test]
    #[cfg(feature = "threads")]
    fn datetime_relocate_is_noop() {
        let affinities = pinned_affinities(&[2]);
        let mut value: DateTime<Utc> = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let expected = value;
        value.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(value, expected);
    }
}
