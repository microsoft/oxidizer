// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ThreadAware` impls for [`bytes`](::bytes) (1.x) types.
//!
//! Enable with the `bytes` Cargo feature.
//!
//! `Bytes` and `BytesMut` internally use reference counting to share the
//! underlying allocation, but they expose an immutable / single-writer
//! interface. From the caller's perspective they behave as inert value types
//! and a no-op `relocate` is the right default: no thread-local resources are
//! associated with them. Callers that want a per-core allocation pattern can
//! still use an affinity-aware container in the higher-level
//! [`thread_aware`](https://docs.rs/thread_aware) crate.

use ::bytes::{Bytes, BytesMut};

impl_noop_thread_aware!(Bytes, BytesMut);

#[cfg(test)]
mod tests {
    use ::bytes::{Bytes, BytesMut};
    use static_assertions::assert_impl_all;

    use crate::{Affinity, ThreadAware};

    assert_impl_all!(Bytes: ThreadAware, Send, Sync);
    assert_impl_all!(BytesMut: ThreadAware, Send, Sync);

    #[test]
    fn bytes_relocate_is_noop() {
        let affinities = [Affinity::new(0, 0, 2, 1), Affinity::new(1, 0, 2, 1)];
        let mut value = Bytes::from_static(b"hello");
        value.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(&*value, b"hello");
    }
}
