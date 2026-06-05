// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ThreadAware` implementations for selected 3rd-party crate types.
//!
//! Each submodule is gated behind a Cargo feature named after the wrapped crate
//! (e.g., `uuid`, `chrono`, `time`, `jiff`, `http`, `bytes`). Enabling a
//! feature pulls in that crate as an optional dependency and exposes no-op
//! `ThreadAware` impls for inert, self-contained types from it. By default no
//! such features are enabled, so the crate stays dependency-free.
//!
//! See the workspace `Cargo.toml` for the exact versions used.

/// Generates a no-op [`ThreadAware`](crate::ThreadAware) impl for each listed type.
///
/// The bodies of the implementations are empty because the listed types are
/// inert value types: they hold no thread-local state, perform no I/O, and
/// participate in no cross-thread sharing that would benefit from relocation.
#[cfg(any(
    feature = "bytes",
    feature = "chrono",
    feature = "http",
    feature = "jiff",
    feature = "time",
    feature = "uuid",
))]
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

#[cfg(feature = "bytes")]
#[cfg_attr(docsrs, doc(cfg(feature = "bytes")))]
mod bytes;

#[cfg(feature = "chrono")]
#[cfg_attr(docsrs, doc(cfg(feature = "chrono")))]
mod chrono;

#[cfg(feature = "http")]
#[cfg_attr(docsrs, doc(cfg(feature = "http")))]
mod http;

#[cfg(feature = "jiff")]
#[cfg_attr(docsrs, doc(cfg(feature = "jiff")))]
mod jiff;

#[cfg(feature = "time")]
#[cfg_attr(docsrs, doc(cfg(feature = "time")))]
mod time;

#[cfg(feature = "uuid")]
#[cfg_attr(docsrs, doc(cfg(feature = "uuid")))]
mod uuid;
