// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ThreadAware` implementations for selected 3rd-party crate types.
//!
//! Each submodule is gated behind a Cargo feature named after the wrapped
//! crate (and its major / 0.x minor where applicable): `bytes`, `http`,
//! `jiff02`, `tokio`, `uuid`. Enabling a feature pulls in that crate as a
//! dependency and exposes `ThreadAware` impls for inert, self-contained
//! types from it.
//! By default no such features are enabled, so this crate does not pull in
//! any of these wrapped crates as additional dependencies.
//!
//! Naming follows a convention agreed during PR review (see this crate's
//! `Cargo.toml` for the full rules): stable `1.x` crates get their bare
//! name (`bytes`, `http`, `uuid`); pre-`1.0` crates encode their `0.x`
//! version (`jiff02`); and a future major (e.g. `bytes 2.0`) would be
//! added additively as a separate feature (e.g. `bytes2`), avoiding a
//! breaking release of this crate purely because of an upstream major bump.
//!
//! Tests in this module are compiled and run as part of `cargo test` without
//! needing the features enabled — the wrapped crates are also listed as
//! unconditional `dev-dependencies`, and the submodules below are gated on
//! `any(test, feature = "...")`.
//!
//! See this crate's `Cargo.toml` for the exact versions used.

/// Generates a no-op [`ThreadAware`](crate::ThreadAware) impl for each listed type.
///
/// The bodies of the implementations are empty because the listed types are
/// inert value types: they hold no thread-local state, perform no I/O, and
/// participate in no cross-thread sharing that would benefit from relocation.
#[cfg(any(test, feature = "bytes", feature = "http", feature = "jiff02", feature = "uuid",))]
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

#[cfg(any(test, feature = "bytes"))]
mod bytes;

#[cfg(any(test, feature = "http"))]
mod http;

#[cfg(any(test, feature = "jiff02"))]
mod jiff02;

#[cfg(any(test, feature = "tokio"))]
mod tokio;

#[cfg(any(test, feature = "uuid"))]
mod uuid;
