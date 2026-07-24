// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consumer-level compile-fail tests that pin the crate's serde safety contract.
//!
//! These exercise the *public* proc-macro entry points and trait bounds a
//! downstream crate would hit, so a regression that (for example) started
//! emitting a plain `Serialize`/`Deserialize` for [`Sym`](internity::Sym), or
//! silently accepted `skip_serializing_if`, would fail the build here even
//! though the positive round-trip tests keep passing.

#![cfg(feature = "serde")]

#[test]
#[cfg_attr(miri, ignore)]
fn compile_fail() {
    let t = trybuild::TestCases::new();

    // The `SerializeIn` derive must reject `skip_serializing_if` through the real
    // proc-macro entry point, not just its internal expansion function.
    t.compile_fail("tests/compile_fail/serialize_in_rejects_skip_serializing_if.rs");

    // A bare `Sym` must not implement Serde's `Serialize`/`Deserialize`: a lexicon-
    // local handle is a meaningless integer without its interner. This contract
    // applies to the `no_std + alloc + serde` configuration too, so the harness
    // requires only `serde` and this fixture runs regardless of `std`.
    t.compile_fail("tests/compile_fail/sym_is_not_serde.rs");

    // A live `ThreadedLexicon` must not be serialized directly (it would race the
    // writer and persist internal handle layout); serialize its frozen `Reader`.
    // `ThreadedLexicon` is `std`-only, so this fixture is gated on `std`.
    #[cfg(feature = "std")]
    t.compile_fail("tests/compile_fail/threaded_lexicon_is_not_serialize.rs");
}
