// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ThreadAware` impls for [`time`] types.
//!
//! Enable with the `time` Cargo feature.
//!
//! `time::Instant` is intentionally omitted because it requires `time`'s `std`
//! feature, which is not enabled by default in this workspace.

use ::time::{Date, Duration, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset, Weekday};

impl_noop_thread_aware!(Date, Time, PrimitiveDateTime, OffsetDateTime, UtcOffset, Duration, Weekday, Month,);

#[cfg(test)]
mod tests {
    use ::time::{Date, Duration, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset, Weekday};
    use static_assertions::assert_impl_all;

    use crate::ThreadAware;
    use crate::affinity::pinned_affinities;

    assert_impl_all!(Date: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(Time: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(PrimitiveDateTime: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(OffsetDateTime: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(UtcOffset: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(Duration: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(Weekday: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(Month: ThreadAware, Send, Sync, Copy);

    #[test]
    #[cfg(feature = "threads")]
    fn offset_date_time_relocate_is_noop() {
        let affinities = pinned_affinities(&[2]);
        let mut value = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let expected = value;
        value.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(value, expected);
    }
}
