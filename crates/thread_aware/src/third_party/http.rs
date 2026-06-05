// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ThreadAware` impls for [`http`] types.
//!
//! Enable with the `http` Cargo feature.
//!
//! Only the fully inert value types are covered. `HeaderName`, `HeaderValue`,
//! `Uri`, etc. internally hold `Bytes` (and therefore an `Arc`) and deserve
//! separate, more nuanced handling rather than a blanket no-op.

use ::http::{Method, StatusCode, Version};

impl_noop_thread_aware!(StatusCode, Version, Method);

#[cfg(test)]
mod tests {
    use ::http::{Method, StatusCode, Version};
    use static_assertions::assert_impl_all;

    use crate::ThreadAware;
    use crate::affinity::pinned_affinities;

    assert_impl_all!(StatusCode: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(Version: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(Method: ThreadAware, Send, Sync);

    #[test]
    #[cfg(feature = "threads")]
    fn method_relocate_is_noop() {
        let affinities = pinned_affinities(&[2]);
        let mut value = Method::POST;
        value.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(value, Method::POST);
    }
}
