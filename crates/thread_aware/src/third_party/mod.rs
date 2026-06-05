// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ThreadAware` implementations for selected 3rd-party crate types.
//!
//! Each submodule is gated behind a Cargo feature whose name embeds the major
//! (or 0.x minor) of the wrapped crate — for example `bytes_v1`, `http_v1`,
//! `jiff_v0_2`, `uuid_v1`. Enabling a feature pulls in that crate as a
//! dependency and exposes `ThreadAware` impls for inert, self-contained types
//! from it. By default no such features are enabled, so the crate stays
//! dependency-free.
//!
//! The version-suffixed naming lets us support a future major of any of these
//! crates additively: when, say, `bytes 2.0` ships we can add a `bytes_v2`
//! feature without removing `bytes_v1`, avoiding a breaking release of this
//! crate purely because of an upstream major bump.
//!
//! Tests in this module are compiled and run as part of `cargo test` without
//! needing the features enabled — the wrapped crates are also listed as
//! unconditional `dev-dependencies`, and the submodules below are gated on
//! `any(test, feature = "...")`.
//!
//! See the workspace `Cargo.toml` for the exact versions used.

/// Generates a no-op [`ThreadAware`](crate::ThreadAware) impl for each listed type.
///
/// The bodies of the implementations are empty because the listed types are
/// inert value types: they hold no thread-local state, perform no I/O, and
/// participate in no cross-thread sharing that would benefit from relocation.
macro_rules! impl_noop_thread_aware {
    ($($t:ty),+ $(,)?) => {
        $(
            impl $crate::ThreadAware for $t {
                fn relocate(
                    &mut self,
                    _source: ::core::option::Option<$crate::affinity::Affinity>,
                    _destination: $crate::affinity::Affinity,
                ) {}
            }
        )+
    };
}

#[cfg(any(test, feature = "bytes_v1"))]
#[cfg_attr(docsrs, doc(cfg(feature = "bytes_v1")))]
pub mod bytes_v1;

#[cfg(any(test, feature = "http_v1"))]
#[cfg_attr(docsrs, doc(cfg(feature = "http_v1")))]
pub mod http_v1;

#[cfg(any(test, feature = "jiff_v0_2"))]
#[cfg_attr(docsrs, doc(cfg(feature = "jiff_v0_2")))]
pub mod jiff_v0_2;

#[cfg(any(test, feature = "uuid_v1"))]
#[cfg_attr(docsrs, doc(cfg(feature = "uuid_v1")))]
pub mod uuid_v1;
