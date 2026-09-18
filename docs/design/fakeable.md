# Fakeable

The `fakeable` procedural macro turns a concrete service type into a wrapper
that can contain either its real implementation or a test implementation. It
is intended for dependencies whose production API should remain concrete while
tests need explicit, per-instance substitution without global state.

The crate is experimental. Its attribute arguments, supported syntax, and
generated code are not yet a stable compatibility contract.

## Package structure

- `fakeable` is the public procedural-macro crate.
- `fakeable_impl` contains parsing and code generation. It is published at the
  same version as `fakeable`, and `fakeable` pins it exactly.
- `fakeable_test` is a private workspace package that verifies generated code
  through end-to-end compilation and behavior tests.

## Generated model

Applying `#[fakeable::fakeable(fake_impl = path::ToFake)]` to a struct replaces
the visible struct with a wrapper around an internal enum. The enum contains
the original real type and, when fakes are enabled, the configured fake type.
The original struct's derives are retained on the generated wrapper and
internal storage.

Applying `#[fakeable::fakeable]` to an implementation block generates the real
implementation on the hidden real type and delegates supported methods from the
wrapper to the active enum variant. Constructors create the real variant.
Methods with a receiver delegate to the matching real or fake method, including
async methods and methods returning `Self`.

Only public or restricted inherent methods are delegated. Private helpers stay
on the real implementation. Trait implementations delegate all methods.
Methods without a receiver are supported only when they return `Self`.
Parameters must use identifier patterns.

## Fake availability

Fake storage and constructors compile under
`cfg(any(feature = "<fakes-feature>", test))`. The default feature name is
`test-util`, and consumers can select another name with `fakes_feature`.
Production builds therefore do not include the fake variant unless they
explicitly enable the configured feature.

To prevent accidental wrapper bloat, non-generic wrappers reject fake types
that are both larger than 256 bytes and at least twice the size of the real
type. Large fakes should be placed behind a pointer such as `Arc`.

## Mockall integration

The optional `mockall` feature enables
`generate_mockall_fake = true` on implementation blocks. The macro emits a
Mockall type in the configured module, excluding constructors, private methods,
and mutable-receiver methods. Async methods are represented as methods
returning `Future` so tests can provide asynchronous expectations.

Mockall remains optional because manually implemented fakes are the primary
mechanism and should not add a production dependency. The integration must be
adopted carefully: its generated API depends on both `fakeable` and Mockall, so
upgrading either can affect generated names, signatures, and expectations.
Mockall-generated types should not be exposed as a stable public API.
