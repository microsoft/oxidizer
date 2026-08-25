// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ThreadAware` impls for [`jiff`](::jiff) (0.2.x) types.
//!
//! Enable with the `jiff02` Cargo feature.
//!
//! `jiff::Zoned` is intentionally omitted because it carries an internal
//! reference to a `TimeZone`, whose semantics around relocation deserve a
//! separate, deliberate design rather than a blanket no-op.

use ::jiff::civil::{Date, DateTime, ISOWeekDate, Time};
use ::jiff::{SignedDuration, Span, Timestamp};

impl_noop_thread_aware!(Timestamp, Span, SignedDuration, Date, Time, DateTime, ISOWeekDate,);

#[cfg(test)]
mod tests {
    use ::jiff::civil::{Date, DateTime, ISOWeekDate, Time};
    use ::jiff::{SignedDuration, Span, Timestamp};
    use static_assertions::assert_impl_all;

    use crate::ThreadAware;
    use crate::affinity::pinned_affinities;

    assert_impl_all!(Timestamp: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(SignedDuration: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(Date: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(Time: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(DateTime: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(ISOWeekDate: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(Span: ThreadAware, Send, Sync, Copy);

    #[test]
    fn timestamp_relocate_is_noop() {
        let affinities = pinned_affinities(&[2]);
        let mut value = Timestamp::from_second(1_700_000_000).unwrap();
        let expected = value;
        value.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(value, expected);
    }
}
