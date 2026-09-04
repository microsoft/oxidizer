// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![forbid(unsafe_code)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/metabench_macros/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/metabench_macros/favicon.ico"
)]

//! Procedural macros for the [`metabench`](https://docs.rs/metabench) crate.
//!
//! **Do not depend on this crate directly.** Use the re-exports from `metabench` instead.

use proc_macro::TokenStream;

/// Declares an inherent impl block containing explicitly marked benchmarks.
///
/// # Example
///
/// In a downstream benchmark target that depends on `metabench`:
///
/// ```ignore
/// struct SortingBenchmarks;
///
/// #[metabench::benchmarks(name = "sorting")]
/// impl SortingBenchmarks {
///     #[metabench::benchmark]
///     fn sort_small() {
///         let mut values = [3, 1, 2];
///         values.sort();
///     }
/// }
/// ```
///
/// See the [compile-checked downstream fixture] for the complete runtime
/// surface used by this expansion.
///
/// [compile-checked downstream fixture]: https://github.com/microsoft/oxidizer/blob/main/crates/metabench_macros/tests/ui/pass/fixtures.rs
#[proc_macro_attribute]
#[cfg_attr(test, mutants::skip)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn benchmarks(arguments: TokenStream, input: TokenStream) -> TokenStream {
    match metabench_macros_impl::benchmarks(arguments.into(), input.into()) {
        Ok(expansion) => expansion,
        Err(error) => error.into_compile_error(),
    }
    .into()
}

/// Configures one method inside a [`benchmarks`] impl block.
///
/// This attribute is consumed by `benchmarks` and cannot be used on its own.
/// The common form has no arguments:
///
/// ```ignore
/// #[metabench::benchmark]
/// fn sort_small() {
///     let mut values = [3, 1, 2];
///     values.sort();
/// }
/// ```
///
/// This uses the method identifier (`sort_small`) as the benchmark name and
/// inherits the engines selected by the enclosing [`benchmarks`] attribute.
/// Every benchmark method must carry this attribute.
///
/// # Grammar
///
/// ```text
/// benchmark-attribute =
///     "#[metabench::benchmark]"
///   | "#[metabench::benchmark(" benchmark-options ")]" ;
///
/// benchmark-options =
///     benchmark-option ("," benchmark-option)* ","? ;
///
/// benchmark-option =
///     "name" "=" STRING_LITERAL
///   | "engines" "=" RUST_EXPRESSION ;
/// ```
///
/// Each option may appear at most once and may be written in either order.
/// `name` overrides the method identifier and must be non-empty and contain no
/// `/`. `engines` overrides the engine selection inherited from the enclosing
/// `benchmarks` attribute.
///
/// # Configured example
///
/// In a downstream benchmark target that depends on `metabench`:
///
/// ```ignore
/// struct SortingBenchmarks;
///
/// #[metabench::benchmarks]
/// impl SortingBenchmarks {
///     #[metabench::benchmark(
///         name = "small",
///         engines = metabench::Engines::CRITERION,
///     )]
///     fn sort_small() {
///         let mut values = [3, 1, 2];
///         values.sort();
///     }
/// }
/// ```
///
/// See the [compile-checked method-configuration fixture] for this attribute
/// in its required `benchmarks` context.
///
/// [compile-checked method-configuration fixture]: https://github.com/microsoft/oxidizer/blob/main/crates/metabench_macros/tests/ui/pass/fixtures.rs
#[proc_macro_attribute]
#[cfg_attr(test, mutants::skip)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn benchmark(_arguments: TokenStream, input: TokenStream) -> TokenStream {
    match metabench_macros_impl::benchmark(input.into()) {
        Ok(expansion) => expansion,
        Err(error) => error.into_compile_error(),
    }
    .into()
}
