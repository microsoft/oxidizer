// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Reachability suite: everything here drives the crate's real public API.
//!
//! `derive_error`, `enrich_err` and `error` are the whole public surface, and they take exactly
//! what the proc-macro shim hands them. Any behaviour that cannot be provoked from here cannot be
//! provoked by a user of `ohno` either, which is what makes an uncovered line evidence of dead
//! code rather than of a missing test.
//!
//! Every assertion is an insta snapshot of the pretty-printed expansion, so a test records what
//! the macro *writes* rather than merely that some substring survived. Related inputs share one
//! snapshot, sitting side by side under `// === label ===` headers. That keeps the number of
//! snapshot files near the number of behaviours instead of the number of inputs, and it puts the
//! variants of one behaviour on screen together.
//!
//! insta reads the workspace from disk, which miri's isolation rejects, so the whole file is
//! `not(miri)`. CI also excludes this crate from the miri job.

#![cfg(not(miri))]

use ohno_macros_impl::{derive_error, enrich_err, error};
use proc_macro2::TokenStream;
use quote::quote;
use testing_aids::{render_expansion, render_tokens_lossy};

/// One case body: the source a user would write, then what the macro turns it into.
///
/// The input is recorded beside the expansion so a snapshot reads on its own. Reviewing a change
/// otherwise means holding the test file open next to the snapshot to learn what produced it.
///
/// The source side is rendered leniently: several cases deliberately feed a macro something that
/// is not an item at all, and that input is still worth showing. The expansion side is rendered
/// strictly, because a macro that emits unparsable tokens is broken.
fn case(source: &TokenStream, expanded: &TokenStream) -> String {
    format!(
        "{}\n// ---- expands to ----\n\n{}",
        render_tokens_lossy(source).trim_end(),
        render_expansion(expanded).trim_end()
    )
}

/// A `#[derive(Error)]` case.
fn derived(input: TokenStream) -> String {
    // `quote!` interpolates through a borrow, so the source view is built first and the input is
    // then handed to the macro by value. That keeps the helper free of clones.
    let source = quote!(#[derive(Error)] #input);
    let expanded = derive_error(input);
    case(&source, &expanded)
}

/// An `#[ohno::error]` case.
fn attributed(item: TokenStream) -> String {
    let source = quote!(#[ohno::error] #item);
    let expanded = error(TokenStream::new(), item);
    case(&source, &expanded)
}

/// An `#[ohno::error(...)]` case, for the arguments the attribute does not accept.
fn attributed_with(args: TokenStream, item: TokenStream) -> String {
    let source = quote!(#[ohno::error(#args)] #item);
    let expanded = error(args, item);
    case(&source, &expanded)
}

/// An `#[enrich_err(...)]` case.
fn enriched(args: TokenStream, item: TokenStream) -> String {
    let source = if args.is_empty() {
        quote!(#[enrich_err] #item)
    } else {
        quote!(#[enrich_err(#args)] #item)
    };
    let expanded = enrich_err(args, item);
    case(&source, &expanded)
}

/// A `#[display(...)]` case over a fixed struct.
fn displayed(attribute: &TokenStream) -> String {
    derived(quote! {
        #attribute
        struct T { path: String, other: u32, inner: ohno::OhnoCore }
    })
}

/// Joins labelled cases into one snapshot body.
///
/// Callers bind the result to a local named `output` and pass that to `assert_snapshot!`. The
/// macro has to sit in the test function itself — insta names a snapshot after the function that
/// calls it, so asserting from inside a shared helper would name every snapshot after the helper.
/// Binding to a local also keeps the recorded `expression` header short, so reformatting a test
/// does not invalidate its snapshot.
fn cases<I: IntoIterator<Item = (&'static str, String)>>(rendered: I) -> String {
    let mut output = String::new();
    for (label, body) in rendered {
        output.push_str("// ======== ");
        output.push_str(label);
        output.push_str(" ========\n\n");
        output.push_str(body.trim_end());
        output.push_str("\n\n");
    }
    output
}

// ---------------------------------------------------------------------------
// `#[derive(Error)]` — the shape of the input
// ---------------------------------------------------------------------------

#[test]
fn every_struct_shape_generates_its_items() {
    let output = cases([
        (
            "named struct",
            derived(quote! {
                struct T { path: String, inner: ohno::OhnoCore }
            }),
        ),
        (
            "tuple struct",
            derived(quote!(
                struct T(String, ohno::OhnoCore);
            )),
        ),
        (
            "a lone core takes no constructor parameters",
            derived(quote! {
                struct T { inner: ohno::OhnoCore }
            }),
        ),
        (
            "a core in the middle keeps declaration order",
            derived(quote! {
                struct T { first: String, inner: ohno::OhnoCore, last: u32 }
            }),
        ),
        (
            "a marked core need not be named OhnoCore",
            derived(quote! {
                struct T { path: String, #[error] mine: Renamed }
            }),
        ),
    ]);
    insta::assert_snapshot!(output);
}

#[test]
fn an_input_that_cannot_hold_a_core_is_rejected() {
    let output = cases([
        (
            "unit struct",
            derived(quote!(
                struct T;
            )),
        ),
        (
            "enum",
            derived(quote! {
                enum T { A, B }
            }),
        ),
        (
            "union",
            derived(quote! {
                union T { a: u32 }
            }),
        ),
        (
            "no field holds a core",
            derived(quote! {
                struct T { path: String }
            }),
        ),
        (
            "two marked cores",
            derived(quote! {
                struct T { #[error] a: ohno::OhnoCore, #[error] b: ohno::OhnoCore }
            }),
        ),
        (
            "two unmarked cores",
            derived(quote! {
                struct T { a: ohno::OhnoCore, b: ohno::OhnoCore }
            }),
        ),
        (
            "one field marked twice",
            derived(quote! {
                struct T { #[error] #[error] inner: ohno::OhnoCore }
            }),
        ),
    ]);
    insta::assert_snapshot!(output);
}

#[test]
fn generics_thread_through_every_impl() {
    let output = cases([
        (
            "type parameters",
            derived(quote! {
                struct T<A, B> { a: A, b: B, inner: ohno::OhnoCore }
            }),
        ),
        (
            "lifetime",
            derived(quote! {
                struct T<'a> { path: &'a str, inner: ohno::OhnoCore }
            }),
        ),
        (
            "where clause",
            derived(quote! {
                struct T<A> where A: Clone { a: A, inner: ohno::OhnoCore }
            }),
        ),
    ]);
    insta::assert_snapshot!(output);
}

#[test]
fn the_suppressing_flags_remove_their_items() {
    let output = cases([
        (
            "no_debug drops the Debug impl",
            derived(quote! {
                #[no_debug]
                struct T { inner: ohno::OhnoCore }
            }),
        ),
        (
            "no_constructors drops new and caused_by",
            derived(quote! {
                #[no_constructors]
                struct T { inner: ohno::OhnoCore }
            }),
        ),
        (
            "both together",
            derived(quote! {
                #[no_debug]
                #[no_constructors]
                struct T { inner: ohno::OhnoCore }
            }),
        ),
    ]);
    insta::assert_snapshot!(output);
}

#[test]
fn a_suppressing_flag_takes_no_arguments() {
    let output = cases([
        (
            "no_debug with a list",
            derived(quote! {
                #[no_debug(yes)]
                struct T { inner: ohno::OhnoCore }
            }),
        ),
        (
            "no_constructors with a value",
            derived(quote! {
                #[no_constructors = 1]
                struct T { inner: ohno::OhnoCore }
            }),
        ),
    ]);
    insta::assert_snapshot!(output);
}

// ---------------------------------------------------------------------------
// `#[display(...)]` — templates
// ---------------------------------------------------------------------------

#[test]
fn a_template_lowers_to_a_literal_or_a_format_call() {
    let output = cases([
        (
            "no placeholders lowers to a literal",
            displayed(&quote!(#[display("nothing interpolated")])),
        ),
        (
            "escapes are resolved for a literal",
            displayed(&quote!(#[display("braces {{ and }}")])),
        ),
        (
            "a named placeholder becomes a field access",
            displayed(&quote!(#[display("failed for {path}")])),
        ),
        ("a format spec survives", displayed(&quote!(#[display("padded {path:>8}")]))),
        (
            "a tuple field is named by index",
            derived(quote! {
                #[display("failed for {0}")]
                struct T(String, ohno::OhnoCore);
            }),
        ),
        (
            "a raw identifier keeps its prefix",
            derived(quote! {
                #[display("failed for {r#type}")]
                struct T { r#type: String, inner: ohno::OhnoCore }
            }),
        ),
    ]);
    insta::assert_snapshot!(output);
}

#[test]
fn a_faulty_template_is_reported() {
    let output = cases([
        (
            "unknown placeholder lists the available fields",
            displayed(&quote!(#[display("failed for {missing}")])),
        ),
        (
            "nothing is referenceable when every field is generated",
            derived(quote! {
                #[display("failed for {missing}")]
                struct T {
                    #[doc = " ohno::generated-core@7f3d9c2a"]
                    inner: ohno::OhnoCore
                }
            }),
        ),
        ("stray closing brace", displayed(&quote!(#[display("stray } brace")]))),
        ("unbalanced opening brace", displayed(&quote!(#[display("unbalanced { brace")]))),
        (
            "two display attributes",
            derived(quote! {
                #[display("first")]
                #[display("second")]
                struct T { inner: ohno::OhnoCore }
            }),
        ),
        ("a non-literal template", displayed(&quote!(#[display(not_a_literal)]))),
    ]);
    insta::assert_snapshot!(output);
}

// ---------------------------------------------------------------------------
// `#[display(...)]` — positional arguments
// ---------------------------------------------------------------------------

#[test]
fn a_positional_argument_is_scoped_to_self() {
    let forms: [(&'static str, TokenStream); 8] = [
        ("bare name", quote!(path)),
        ("field access", quote!(path.inner)),
        ("method call", quote!(path.len())),
        ("index", quote!(path[0])),
        ("cast", quote!(path as u64)),
        ("binary operator", quote!(path * 2)),
        ("try", quote!(path?)),
        ("range", quote!(path..10)),
    ];

    let output = cases(forms.map(|(label, argument)| (label, displayed(&quote!(#[display("value {}", #argument)])))));
    insta::assert_snapshot!(output);
}

#[test]
fn a_numeric_argument_roots_at_a_tuple_field() {
    let forms: [(&'static str, TokenStream); 3] = [
        ("integer literal", quote!(0)),
        ("float literal is a nested tuple access", quote!(0.1)),
        ("method on an integer literal", quote!(0.abs())),
    ];

    let output = cases(forms.map(|(label, argument)| {
        (
            label,
            derived(quote! {
                #[display("value {}", #argument)]
                struct T(String, ohno::OhnoCore);
            }),
        )
    }));
    insta::assert_snapshot!(output);
}

#[test]
fn an_argument_may_call_a_method_of_self() {
    let output = cases([
        ("bare call", displayed(&quote!(#[display("value {}", describe())]))),
        ("await", displayed(&quote!(#[display("value {}", path.await)]))),
        (
            "trailing comma after the arguments",
            displayed(&quote!(#[display("value {}", path,)])),
        ),
    ]);
    insta::assert_snapshot!(output);
}

#[test]
fn a_faulty_positional_argument_is_reported() {
    let output = cases([
        ("a self. prefix", displayed(&quote!(#[display("value {}", self.path)]))),
        ("an unknown root", displayed(&quote!(#[display("value {}", missing.len())]))),
        (
            "an argument with no placeholder",
            displayed(&quote!(#[display("no placeholder", path)])),
        ),
        (
            "more placeholders than arguments",
            displayed(&quote!(#[display("{} and {}", path)])),
        ),
        (
            "every fault in one template together",
            displayed(&quote!(#[display("{missing} {} {}", self.path)])),
        ),
    ]);
    insta::assert_snapshot!(output);
}

#[test]
fn an_argument_that_cannot_follow_self_is_reported() {
    // Each of these takes a different path through the root walk: an associated path, a literal
    // receiver, a qualified path, a suffixed literal (which is not a tuple index), a parenthesized
    // or unary expression, and a range with no start.
    let forms: [(&'static str, TokenStream); 13] = [
        ("associated constant", quote!(Self::LABEL.len())),
        ("string literal receiver", quote!("prefix".len())),
        ("fully qualified call", quote!(std::mem::size_of::<u8>())),
        ("qualified path", quote!(<T>::VALUE)),
        ("qualified path with a trait", quote!(<T as Trait>::VALUE)),
        ("parenthesized", quote!((path))),
        ("unary operator", quote!(-path)),
        ("range with no start", quote!(..10)),
        ("char literal", quote!('c')),
        ("suffixed integer", quote!(0u8)),
        ("method on a suffixed integer", quote!(1usize.abs())),
        ("suffixed float", quote!(0.1f32)),
        ("suffixed float with a trailing zero", quote!(2.0f64)),
    ];

    let output = cases(forms.map(|(label, argument)| (label, displayed(&quote!(#[display("value {}", #argument)])))));
    insta::assert_snapshot!(output);
}

// ---------------------------------------------------------------------------
// `#[from(...)]`
// ---------------------------------------------------------------------------

#[test]
fn a_from_attribute_generates_its_conversions() {
    let output = cases([
        (
            "one source type",
            derived(quote! {
                #[from(std::io::Error)]
                struct T { source: String, inner: ohno::OhnoCore }
            }),
        ),
        (
            "several source types",
            derived(quote! {
                #[from(std::io::Error, std::fmt::Error)]
                struct T { source: String, inner: ohno::OhnoCore }
            }),
        ),
        (
            "a generic source keeps its arguments",
            derived(quote! {
                #[from(Wrapper<u8, u16>)]
                struct T { source: String, inner: ohno::OhnoCore }
            }),
        ),
        (
            "a named field override",
            derived(quote! {
                #[from(std::io::Error(path: "unknown".to_owned()))]
                struct T { path: String, inner: ohno::OhnoCore }
            }),
        ),
        (
            "a tuple index override",
            derived(quote! {
                #[from(std::io::Error(0: "unknown".to_owned()))]
                struct T(String, ohno::OhnoCore);
            }),
        ),
        (
            "fields with no override default",
            derived(quote! {
                #[from(std::io::Error)]
                struct T { path: String, other: u32, inner: ohno::OhnoCore }
            }),
        ),
    ]);
    insta::assert_snapshot!(output);
}

#[test]
fn a_faulty_from_attribute_is_reported() {
    let output = cases([
        (
            "not a parenthesized list",
            derived(quote! {
                #[from = 1]
                struct T { inner: ohno::OhnoCore }
            }),
        ),
        (
            "an empty list",
            derived(quote! {
                #[from()]
                struct T { inner: ohno::OhnoCore }
            }),
        ),
        (
            "an entry that opens with a named override list",
            derived(quote! {
                #[from((path: 1))]
                struct T { path: String, inner: ohno::OhnoCore }
            }),
        ),
        (
            "an entry that opens with an indexed override list",
            derived(quote! {
                #[from((0: 1))]
                struct T(String, ohno::OhnoCore);
            }),
        ),
        (
            "an override naming an unknown field",
            derived(quote! {
                #[from(std::io::Error(missing: 1))]
                struct T { path: String, inner: ohno::OhnoCore }
            }),
        ),
        (
            "a named key on a tuple struct",
            derived(quote! {
                #[from(std::io::Error(path: 1))]
                struct T(String, ohno::OhnoCore);
            }),
        ),
        (
            "a key naming the core",
            derived(quote! {
                #[from(std::io::Error(inner: 1))]
                struct T { path: String, inner: ohno::OhnoCore }
            }),
        ),
    ]);
    insta::assert_snapshot!(output);
}

// ---------------------------------------------------------------------------
// `#[ohno::error]`
// ---------------------------------------------------------------------------

#[test]
fn the_error_attribute_adds_a_core_to_every_struct_shape() {
    let output = cases([
        (
            "named struct",
            attributed(quote! {
                struct T { path: String }
            }),
        ),
        (
            "a colliding core name is numbered",
            attributed(quote! {
                struct T { ohno_core: u32, ohno_core_1: u32 }
            }),
        ),
        (
            "tuple struct gains a trailing core",
            attributed(quote!(
                struct T(String);
            )),
        ),
        (
            "unit struct becomes a tuple struct",
            attributed(quote!(
                struct T;
            )),
        ),
    ]);
    insta::assert_snapshot!(output);
}

#[test]
fn the_error_attribute_leaves_the_authors_own_attributes_alone() {
    let output = cases([
        (
            "docs and derives on the struct survive",
            attributed(quote! {
                /// Documentation for T.
                #[derive(Clone)]
                #[display("failed for {path}")]
                struct T { path: String }
            }),
        ),
        (
            "an ordinary doc comment on a field survives",
            attributed(quote! {
                struct T {
                    /// Where the failure happened.
                    path: String,
                }
            }),
        ),
        (
            "a doc comment that merely mentions the marker is not the marker",
            attributed(quote! {
                struct T {
                    /// The ohno generated core field is not this one.
                    path: String,
                }
            }),
        ),
        (
            "a non-doc attribute carrying the marker text is not the marker",
            attributed(quote! {
                struct T {
                    #[other = " ohno::generated-core@7f3d9c2a"]
                    path: String,
                }
            }),
        ),
        (
            "a doc attribute with a non-string value is not the marker",
            attributed(quote! {
                struct T {
                    #[doc = 1]
                    path: String,
                }
            }),
        ),
        (
            "a doc attribute built by a macro is not the marker",
            attributed(quote! {
                struct T {
                    #[doc = concat!(" ohno::generated-core@7f3d9c2a")]
                    path: String,
                }
            }),
        ),
    ]);
    insta::assert_snapshot!(output);
}

#[test]
fn the_error_attribute_reports_what_it_cannot_rewrite() {
    let output = cases([
        (
            "a field already marked with #[error], and nothing is re-emitted",
            attributed(quote! {
                struct T { path: String, #[error] mine: ohno::OhnoCore }
            }),
        ),
        (
            "a hand-written reserved marker",
            attributed(quote! {
                struct T {
                    path: String,
                    #[doc = " ohno::generated-core@7f3d9c2a"]
                    mine: ohno::OhnoCore,
                }
            }),
        ),
        (
            "no_constructors, which would leave the added field uninitialized",
            attributed(quote! {
                #[no_constructors]
                struct T { path: String }
            }),
        ),
        (
            "a non-struct",
            attributed(quote!(
                enum T {
                    A,
                }
            )),
        ),
        (
            "an argument, which the attribute does not take",
            attributed_with(
                quote!(anything),
                quote! {
                    struct T { path: String }
                },
            ),
        ),
        ("an unparsable item", attributed(quote!(1 + 1))),
    ]);
    insta::assert_snapshot!(output);
}

// ---------------------------------------------------------------------------
// `#[enrich_err(...)]`
// ---------------------------------------------------------------------------

#[test]
fn the_enrich_attribute_wraps_the_body_and_enriches_the_error() {
    let output = cases([
        (
            "a bare attribute names the function",
            enriched(
                TokenStream::new(),
                quote! {
                    fn load() -> Result<(), MyError> { inner() }
                },
            ),
        ),
        (
            "a literal message renders without format",
            enriched(
                quote!("could not load"),
                quote! {
                    fn load() -> Result<(), MyError> { inner() }
                },
            ),
        ),
        (
            "an inline capture goes through format",
            enriched(
                quote!("could not load {path}"),
                quote! {
                    fn load(path: &str) -> Result<(), MyError> { inner() }
                },
            ),
        ),
        (
            "arguments are passed through unchanged",
            enriched(
                quote!("could not load {}", path),
                quote! {
                    fn load(path: &str) -> Result<(), MyError> { inner() }
                },
            ),
        ),
        (
            "arguments force a format call even with no placeholder",
            enriched(
                quote!("plain message", path),
                quote! {
                    fn load(path: &str) -> Result<(), MyError> { inner() }
                },
            ),
        ),
        (
            "a self-prefixed argument is left alone",
            enriched(
                quote!("could not load {}", self.path),
                quote! {
                    fn load(&self) -> Result<(), MyError> { inner() }
                },
            ),
        ),
        (
            "the signature survives untouched",
            enriched(
                TokenStream::new(),
                // Deliberately loaded with the rarer signature components. The wrapper re-emits
                // the whole `syn::Signature` and only swaps the block, so an omission anywhere in
                // it — a dropped `unsafe`, ABI, receiver or doc attribute — is a silent
                // regression unless the snapshot carries one of each.
                quote! {
                    /// Documented.
                    pub(crate) unsafe extern "C" fn load<A: Clone>(
                        &mut self,
                        path: &str,
                        count: usize,
                    ) -> Result<A, MyError>
                    where
                        A: Send,
                    {
                        inner()
                    }
                },
            ),
        ),
        (
            "an async function awaits an async block",
            enriched(
                TokenStream::new(),
                quote! {
                    async fn load() -> Result<(), MyError> { inner().await }
                },
            ),
        ),
    ]);
    insta::assert_snapshot!(output);
}

#[test]
fn the_enrich_attribute_reports_what_it_cannot_enrich() {
    let output = cases([
        (
            "a non-function",
            enriched(
                TokenStream::new(),
                quote! {
                    struct T;
                },
            ),
        ),
        (
            "a function with no return type",
            enriched(
                TokenStream::new(),
                quote! {
                    fn load() { inner(); }
                },
            ),
        ),
        (
            "a non-literal first argument",
            enriched(
                quote!(not_a_literal),
                quote! {
                    fn load() -> Result<(), MyError> { inner() }
                },
            ),
        ),
        ("an unparsable item", enriched(TokenStream::new(), quote!(1 + 1))),
    ]);
    insta::assert_snapshot!(output);
}

// ---------------------------------------------------------------------------
// Inputs no entry point can parse
// ---------------------------------------------------------------------------

#[test]
fn an_unparsable_derive_input_is_reported() {
    let output = derived(quote!(
        fn not_a_type() {}
    ));
    insta::assert_snapshot!(output);
}
