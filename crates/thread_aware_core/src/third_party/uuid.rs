// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ThreadAware` impls for [`uuid`](::uuid) (1.x) types.
//!
//! Enable with the `uuid` Cargo feature.

impl_noop_thread_aware!(::uuid::Uuid);

#[cfg(test)]
mod tests {
    use ::uuid::Uuid;
    use static_assertions::assert_impl_all;

    use crate::{Affinity, ThreadAware};

    assert_impl_all!(Uuid: ThreadAware, Send, Sync, Copy);

    #[test]
    fn uuid_relocate_is_noop() {
        let affinities = [Affinity::new(0, 0, 2, 1), Affinity::new(1, 0, 2, 1)];
        let mut value = Uuid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788);
        let expected = value;
        value.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(value, expected);
    }
}
