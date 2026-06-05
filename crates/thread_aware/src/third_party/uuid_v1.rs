// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ThreadAware` impls for [`uuid`] (1.x) types.
//!
//! Enable with the `uuid_v1` Cargo feature.

impl_noop_thread_aware!(::uuid::Uuid);

#[cfg(test)]
mod tests {
    use ::uuid::Uuid;
    use static_assertions::assert_impl_all;

    use crate::ThreadAware;
    #[cfg(feature = "threads")]
    use crate::affinity::pinned_affinities;

    assert_impl_all!(Uuid: ThreadAware, Send, Sync, Copy);

    #[test]
    #[cfg(feature = "threads")]
    fn uuid_relocate_is_noop() {
        let affinities = pinned_affinities(&[2]);
        let mut value = Uuid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788);
        let expected = value;
        value.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(value, expected);
    }
}
