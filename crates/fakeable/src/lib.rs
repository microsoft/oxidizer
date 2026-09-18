// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    clippy::test_attr_in_doctest,
    reason = "doc examples show realistic test usage with #[test] attributes"
)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Proc macros for streamlining the creation of fakeable structures.
//!
//! This crate provides the `fakeable` attribute macro that can be applied to structs
//! and their impl blocks to automatically generate a wrapper structure that can switch
//! between real and fake implementations at runtime. The fake implementation is only
//! compiled when the specified feature flag (default: "test-util") is enabled or during
//! test builds.
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
///
/// // Usage in tests
/// #[cfg(test)]
/// mod tests {
///     use super::*;
///
///     #[test]
///     fn test_with_fake() {
///         let service = UserService::fake(fakes::FakeUserService);
///         assert_eq!(service.get_user(1), Some("Fake User".to_string()));
///     }
/// }
/// ```
///
/// # Configuration Options
///
/// ## For Structs
///
/// - **`fake_impl`** (required): Specifies the type of the fake implementation
/// - **`fake_constructor`** (optional): Name of the constructor method for fake instances (default: "fake")
/// - **`fakes_feature`** (optional): Feature flag name for enabling fakes (default: "test-util")
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
/// - **`fakes_feature`** (optional): Feature flag name for enabling fakes (default: "test-util")
///
/// # Optional mockall Mock Generation
///
/// This integration is experimental and should be used carefully. Generated
/// Mockall types are not a stable compatibility boundary: upgrading either
/// `fakeable` or `mockall` can change the generated API or behavior.
///
/// By specifying `generate_mockall_fake = true` in the attribute for the impl block, this macro
/// will generate a mock implementation using the `mockall` crate. The generated mock will be placed
/// in the specified module (default: "mocks"). This option requires the `mockall` Cargo feature.
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
/// # Limitations
///
/// - Mockall generation rejects mutable methods and methods with restricted visibility because
///   it cannot generate a fake that matches the wrapper's delegated method set.
/// - Receiver-less methods must return `Self`; other associated functions cannot select a real or
///   fake implementation to delegate to.
/// - Typed receivers such as `self: Box<Self>` are rejected; use `self`, `&self`, or `&mut self`.
/// - Complex parameter patterns in method signatures are not supported in public methods
/// - Generic types in impl blocks require careful handling
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
