// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Proc macros for streamlining the creation of fakeable structures.
//!
//! This crate provides the `fakeable` attribute macro that can be applied to structs
//! and their impl blocks to automatically generate a wrapper structure that can switch
//! between real and fake implementations at runtime. The fake implementation is only
//! compiled when the specified feature flag (default: "test-util") is enabled or during
//! unit test builds. Set `fakes_feature = false` for unit tests only.
//!
//! # Stability
//!
//! This crate is experimental. Its attribute arguments, supported Rust syntax,
//! and generated code may change as usage experience develops.
//!
//! The optional `mockall` integration requires particular care. It exposes
//! behavior generated jointly by `fakeable` and `mockall`, so changes in either
//! crate can affect generated names, signatures, and expectations. Avoid
//! exposing Mockall-generated types as a stable public API, and review upgrades
//! for backwards compatibility before adopting them.

use fakeable_impl::fakeable_impl;
use proc_macro::TokenStream;

/// Attribute macro for creating fakeable structs and their implementations.
///
/// This macro can be applied to both struct definitions and impl blocks to create
/// a wrapper structure that can switch between real and fake implementations at runtime.
/// This is particularly useful for dependency injection, testing, and creating mockable
/// services.
///
/// ```rust
/// // Define the struct with fake implementation
/// #[fakeable::fakeable(fake_impl = fakes::FakeUserService)]
/// #[derive(Clone)]
/// pub struct UserService {
///     database_url: String,
/// }
///
/// #[fakeable::fakeable]
/// impl UserService {
///     pub fn new(database_url: String) -> Self {
///         Self { database_url }
///     }
///
///     pub fn get_user(&self, id: u64) -> Option<String> {
///         // Real implementation would query database
///         Some(format!("User {}", id))
///     }
///
///     pub async fn save_user(&self, name: &str) -> Result<u64, String> {
///         // Real implementation would save to database
///         Ok(42)
///     }
/// }
///
/// // Manual fake implementation
/// #[cfg(any(feature = "test-util", test))]
/// pub mod fakes {
///     #[derive(Clone)]
///     pub struct FakeUserService;
///
///     impl FakeUserService {
///         pub fn get_user(&self, _id: u64) -> Option<String> {
///             Some("Fake User".to_string())
///         }
///
///         pub async fn save_user(&self, _name: &str) -> Result<u64, String> {
///             Ok(999)
///         }
///     }
/// }
/// ```
///
/// Fake constructors are enabled by a feature of the consuming crate, not a
/// feature inherited from `fakeable`. A consumer using the default name
/// declares and enables `test-util` for the test build:
///
/// ```toml
/// [features]
/// test-util = []
/// ```
///
/// With that consumer feature enabled, tests can construct the configured fake:
///
/// ```rust,ignore
/// let service = UserService::fake(fakes::FakeUserService);
/// assert_eq!(service.get_user(1), Some("Fake User".to_string()));
/// ```
///
/// # Configuration Options
///
/// ## For Structs
///
/// - **`fake_impl`** (required): Specifies the type of the fake implementation
/// - **`fake_constructor`** (optional): Name of the constructor method for fake instances (default: "fake")
/// - **`fakes_feature`** (optional): Consumer feature name for enabling fakes (default: "test-util"),
///   or `false` to enable fakes only under `cfg(test)`
///
/// ```rust
/// # #[cfg(feature = "mockall")]
/// # mod example {
/// #[fakeable::fakeable(
///     fake_impl = mocks::MockMyService,
///     fake_constructor = "create_fake",
///     fakes_feature = "testing"
/// )]
/// pub struct MyService {
///     // fields...
/// }
///
/// # #[fakeable::fakeable(
/// #    generate_mockall_fake = true,
/// #    mockall_fake_module = "mocks",
/// #    fakes_feature = "testing"
/// # )]
/// # impl MyService {
/// #    // methods...
/// # }
/// # }
/// ```
///
/// ## For Impl Blocks
///
/// - **`fakes_feature`** (optional): Consumer feature name for enabling fakes (default: "test-util"),
///   or `false` for unit tests only; use the same value on the struct and every impl
/// - **`generate_mockall_fake = true`** (optional): Generate a Mockall fake; requires the
///   `mockall` feature and a direct consumer dependency on `mockall`
/// - **`mockall_fake_module`** (optional): Module for the generated mock (default: `"fakes"`);
///   use `"."` to emit it in the current module
/// - **`mockall_fake_visibility`** (optional): Rust visibility string for the mock container
///   (default: `"pub"`); use `"pub(crate)"` for internal adapters or `""` for private mocks.
///   Requires `generate_mockall_fake = true`. For a named module it controls the module's
///   visibility; the mock type is public inside that module. With `"."`, it controls the
///   mock type directly. Relative visibility is resolved in the module containing the impl block.
///
/// `fakes_attribute` is accepted as a deprecated alias for `fakes_feature` for compatibility with
/// the imported experimental API.
///
/// # Optional mockall Mock Generation
///
/// This integration is experimental and should be used carefully. Generated
/// Mockall types are not a stable compatibility boundary: upgrading either
/// `fakeable` or `mockall` can change the generated API or behavior.
///
/// By specifying `generate_mockall_fake = true` in the attribute for the impl block, this macro
/// will generate a mock implementation using the `mockall` crate. The generated mock will be placed
/// in the specified module (default: "fakes"). This option requires the `fakeable` `mockall` Cargo
/// feature and a direct `mockall` dependency in the consuming crate.
///
/// ```rust
/// # #[cfg(feature = "mockall")]
/// # mod example {
/// #[fakeable::fakeable(
///     fake_impl = mocks::MockMyService,
/// )]
/// pub struct MyService {
///     // fields...
/// }
///
/// #[fakeable::fakeable(generate_mockall_fake = true, mockall_fake_module = "mocks")]
/// impl MyService {
///     // methods...
/// }
/// # }
/// ```
///
/// # Generated Code Structure
///
/// The macro generates:
/// 1. An internal enum that holds either the real or fake implementation
/// 2. A wrapper struct with the original name that contains the enum
/// 3. Delegation methods that route calls based on the current implementation
/// 4. A constructor method for creating fake instances (when `fake_impl` is specified)
/// 5. Optional mockall mock generation (when `generate_mockall_fake = true`)
///
/// # Feature Flags
///
/// Fake implementations are conditionally compiled based on feature flags:
/// - Code is included when the specified feature (default: "test-util") is enabled
/// - Code is also included in test builds (`#[cfg(test)]`)
/// - This allows fakes to be used in tests without needing to enable features in production
///
/// ## Unit Tests Only
///
/// Set `fakes_feature = false` on the struct and every annotated impl to emit only
/// `cfg(test)` gates, with no consumer feature declaration or cfg-checking workaround.
/// Integration tests and other dependent crates cannot access these fakes, even
/// when a consumer's `test-util` feature is enabled.
///
/// When `fakeable` is a dev-dependency, also use `cfg_attr(test, ...)` so the macro
/// itself is not needed in normal builds:
///
/// ```
/// #[cfg_attr(test, fakeable::fakeable(fake_impl = fakes::FakeReader, fakes_feature = false))]
/// struct Reader;
///
/// #[cfg_attr(test, fakeable::fakeable(fakes_feature = false))]
/// impl Reader {
///     pub(crate) fn new() -> Self {
///         Self {}
///     }
///     pub(crate) fn read(&self) -> u32 {
///         42
///     }
/// }
///
/// #[cfg(test)]
/// mod fakes {
///     pub(crate) struct FakeReader;
///     impl FakeReader {
///         pub(crate) fn read(&self) -> u32 {
///             7
///         }
///     }
/// }
///
/// assert_eq!(Reader::new().read(), 42);
/// ```
///
/// ## Internal Adapters with Mockall
///
/// Generated mocks support `pub` and `pub(crate)` methods, retaining their
/// visibility on the mock. Constructors still create real instances; private
/// helpers are not mocked. Enable the `mockall` feature and declare `mockall` as a
/// direct dev-dependency to use its argument matchers, call counts, ordered
/// sequences, and scripted results without writing a separate fake.
/// Set `mockall_fake_visibility` explicitly to keep the generated container internal.
/// Visibility is not inferred from methods: adding, removing, or gating a method must
/// not silently change the container's API. Choose a visibility that also permits use
/// in the wrapper's fake constructor; a public wrapper with a public fake constructor
/// needs an accessible fake type.
///
/// ```rust
/// # #[cfg(feature = "mockall")]
/// # mod example {
/// #[fakeable::fakeable(fake_impl = fakes::MockReader, fakes_feature = false)]
/// pub(crate) struct Reader;
///
/// #[fakeable::fakeable(
///     generate_mockall_fake = true,
///     fakes_feature = false,
///     mockall_fake_visibility = "pub(crate)"
/// )]
/// impl Reader {
///     pub(crate) fn read(&self, key: &str) -> usize {
///         key.len()
///     }
/// }
///
/// # #[cfg(test)]
/// # fn example() {
/// let mut mock = fakes::MockReader::new();
/// mock.expect_read()
///     .with(mockall::predicate::eq("answer"))
///     .times(1)
///     .return_const(7usize);
/// assert_eq!(Reader::fake(mock).read("answer"), 7);
/// # }
/// # }
/// ```
///
/// # Lints
///
/// Generated real impls allow `clippy::unused_self`: an otherwise stateless
/// operation may need a receiver to select its fake implementation. The allowance
/// covers the whole real impl, including private helpers that are not delegated.
/// Explicit lint attributes on the original impl or its methods take precedence.
/// This allowance applies only when the macro expands. With `cfg_attr(test, ...)`,
/// normal builds still lint the implementation without macro expansion.
///
/// # Limitations
///
/// - Mockall generation rejects mutable methods and method visibility other than `pub` or
///   `pub(crate)`; use a manual fake for other visibility scopes.
/// - Mockall generation rejects consuming receivers, const methods, methods returning `Self`, and
///   signatures with multiple nested elided references; use a manual fake for those method shapes.
/// - Mockall generation preserves method `cfg`/`cfg_attr` attributes and rejects nested elided
///   references beneath implicit higher-ranked function-pointer or trait-object binders.
///   Diagnostics for unsupported cfg-gated methods carry the same gating attributes.
///   Unsupported methods without cfg attributes produce errors even when fake storage is disabled.
/// - Generated Mockall signatures require async method futures to be `Send`; use a manual fake for
///   async methods whose futures are not `Send`.
/// - Mockall generation rejects generic impl blocks; use a manual fake for generic services.
/// - Mockall generation rejects trait impl blocks; use a manual fake for trait implementations.
/// - Unsafe impl blocks are rejected because the macro cannot establish their safety invariants for
///   the fake representation.
/// - Receiver-less methods must return `Self`; other associated functions cannot select a real or
///   fake implementation to delegate to.
/// - `mut self`, projected or nested `Self` types, `Self` parameters, and `Self` in method generic
///   bounds or where predicates are rejected because they cannot be translated across the wrapper
///   boundary.
/// - Typed receivers such as `self: Box<Self>` are rejected; use `self`, `&self`, or `&mut self`.
/// - Direct `#[cfg(...)]` attributes are supported on structs and impl blocks. A `cfg_attr` that
///   conditionally applies `cfg` is rejected because it cannot safely gate every generated item.
/// - A `cfg_attr` that conditionally applies `derive` is rejected; apply derives directly.
/// - Layout `repr` attributes are rejected because the generated wrapper has a different field
///   layout and ABI.
/// - Fake implementation paths beginning with `self` or `super`, and qualified impl targets, are
///   rejected because generated helper items live in a different module.
/// - Impl blocks containing `self::` or `super::` paths, or concrete service references in impl
///   generic bounds and where predicates, are rejected because the real impl is relocated.
/// - Struct declarations containing `self` or `super` paths, including generics, attributes, field
///   types, and type-macro payloads, are rejected because the real struct is relocated into that
///   helper module.
/// - Struct fields must be private because the visible wrapper does not preserve direct field
///   access or struct-literal construction.
/// - Public associated constants and types in inherent impls are rejected because the generated
///   wrapper cannot preserve them.
/// - Macros inside impl blocks are rejected because the macro cannot determine which public methods
///   they generate; expand them before applying `fakeable`.
/// - Attributes on method receivers, parameters, or generic parameters, `ref`/`ref mut` bindings,
///   and sub-patterns are rejected because forwarding could change under cfg or binding semantics.
/// - `impl Trait` return types are rejected because real and fake implementations may choose
///   different opaque concrete types.
/// - Unsafe methods and signatures that use the concrete service type across the wrapper boundary,
///   including qualified paths ending in the service name, are rejected; use safe methods and
///   direct `Self` returns.
/// - Trait impl paths cannot use `Self` or the concrete service type in generic arguments because
///   those names would resolve to different real and wrapper types.
/// - Trait associated items containing `Self` are rejected because copying them would give the
///   hidden real implementation and wrapper different associated types or values.
/// - Mockall generation rejects nested elided references beneath higher-ranked lifetime binders.
/// - Struct derives are copied to the wrapper and internal enum. The fake type must satisfy their
///   bounds (for example, `Clone`), and derives that depend on struct shape or an enum default
///   variant may be unsuitable.
/// - Complex parameter patterns in method signatures are not supported in public methods
/// - Generic impl blocks are supported with manual fakes; Mockall generation rejects them.
///
/// # Private Methods
///
/// Private methods (without `pub` visibility) are intentionally skipped by the macro and not
/// delegated to fake implementations. This allows you to:
/// - Use unsupported generic parameters or complex patterns in private helper methods
/// - Define private utility functions without a `self` parameter
/// - Keep internal implementation details separate from the fakeable public API
///
/// Private methods remain available only on the real implementation and cannot be called
/// through fake instances.
#[proc_macro_attribute]
#[cfg_attr(test, mutants::skip)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn fakeable(args: TokenStream, input: TokenStream) -> TokenStream {
    fakeable_impl(args.into(), input.into()).into()
}
