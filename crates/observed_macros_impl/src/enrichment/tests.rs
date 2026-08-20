// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Unit tests for the `#[derive(Enrichment)]` expansion.
//!
//! insta cannot run under miri: `insta::_macro_support::get_cargo_workspace` reads from disk and
//! miri's isolation rejects that, so this module is declared `#[cfg(all(test, not(miri)))]`.

use super::*;

fn parse_and_generate(input: &str) -> String {
    let input: DeriveInput = syn::parse_str(input).expect("failed to parse input");
    let tokens = derive_enrichment(&input).expect("failed to derive");
    let file = syn::parse2(tokens).expect("failed to parse generated code");
    prettyplease::unparse(&file)
}

fn parse_and_expect_error(input: &str) -> String {
    let input: DeriveInput = syn::parse_str(input).expect("failed to parse input");
    derive_enrichment(&input).expect_err("expected derive to fail").to_string()
}

#[test]
fn test_basic_enrichment() {
    let output = parse_and_generate(
        r"
        struct RequestContext {
            method: ClassifiedString,
            #[unredacted]
            status: i64,
        }
        ",
    );
    insta::assert_snapshot!(output);
}

#[test]
fn test_enrichment_with_rename() {
    let output = parse_and_generate(
        r#"
        struct RequestContext {
            #[dimension(log = "http.method")]
            method: ClassifiedString,
            #[unredacted]
            status: i64,
        }
        "#,
    );
    insta::assert_snapshot!(output);
}

#[test]
fn test_enrichment_all_attributes() {
    let output = parse_and_generate(
        r#"
        struct RequestContext {
            #[dimension(log = "http.method")]
            method: ClassifiedString,
            #[dimension(log = exclude)]
            request_id: ClassifiedString,
            #[unredacted]
            status: i64,
        }
        "#,
    );
    insta::assert_snapshot!(output);
}

#[test]
fn test_error_enum() {
    let err = parse_and_expect_error(
        r"
        enum BadEnrichment {
            A,
            B,
        }
        ",
    );
    assert!(err.contains("structs"), "error should mention structs: {err}");
}

#[test]
fn test_enrichment_exclude_from_logs() {
    let output = parse_and_generate(
        r"
        struct RequestContext {
            method: ClassifiedString,
            #[dimension(log = exclude)]
            #[unredacted]
            internal_flag: i64,
        }
        ",
    );
    insta::assert_snapshot!(output);
}

#[test]
fn test_enrichment_with_data_class() {
    let output = parse_and_generate(
        r"
        struct RequestContext {
            #[data_class(DataTaxonomy::Euii)]
            username: String,
            #[unredacted]
            status: i64,
        }
        ",
    );
    insta::assert_snapshot!(output);
}

#[test]
fn test_error_data_class_and_unredacted() {
    let err = parse_and_expect_error(
        r"
        struct BadEnrichment {
            #[data_class(Euii)]
            #[unredacted]
            x: String,
        }
        ",
    );
    assert!(err.contains("mutually exclusive"), "error should mention mutually exclusive: {err}");
}

#[test]
fn test_enrichment_with_lifetime() {
    let output = parse_and_generate(
        r"
        struct BorrowedCtx<'a> {
            #[unredacted]
            label: &'a str,
        }
        ",
    );
    insta::assert_snapshot!(output);
}

#[test]
fn test_enrichment_with_type_parameter() {
    let output = parse_and_generate(
        r"
        struct GenericCtx<T> {
            #[unredacted]
            value: T,
        }
        ",
    );
    insta::assert_snapshot!(output);
}

#[test]
fn test_enrichment_with_lifetime_and_type_parameter() {
    let output = parse_and_generate(
        r"
        struct MixedCtx<'a, T> {
            #[unredacted]
            label: &'a str,
            #[unredacted]
            value: T,
        }
        ",
    );
    insta::assert_snapshot!(output);
}

#[test]
fn test_error_duplicate_log_setting() {
    let err = parse_and_expect_error(
        r#"
        struct BadEnrichment {
            #[dimension(log = "a", log = "b")]
            x: String,
        }
        "#,
    );
    assert!(err.contains("duplicate `log`"), "{err}");
}

#[test]
fn test_error_duplicate_exclude_setting() {
    let err = parse_and_expect_error(
        r"
        struct BadEnrichment {
            #[dimension(log = exclude, log = exclude)]
            x: String,
        }
        ",
    );
    assert!(err.contains("duplicate `log`"), "{err}");
}

#[test]
fn test_error_unredacted_with_args() {
    let err = parse_and_expect_error(
        r"
        struct BadEnrichment {
            #[unredacted(foo)]
            x: String,
        }
        ",
    );
    assert!(err.contains("does not accept arguments"), "{err}");
}

/// `#[unredacted]` is a marker, so a name-value form must be rejected
/// rather than silently selecting the privacy bypass.
#[test]
fn test_error_unredacted_with_value() {
    for payload in ["false", "\"no\"", "0"] {
        let err = parse_and_expect_error(&format!(
            r"
            struct BadEnrichment {{
                #[unredacted = {payload}]
                x: String,
            }}
            ",
        ));
        assert!(err.contains("does not accept arguments"), "{err}");
    }
}

#[test]
fn test_error_unit_struct() {
    let err = parse_and_expect_error(
        r"
        struct Empty;
        ",
    );
    assert!(err.contains("named fields"), "error should mention named fields: {err}");
}

#[test]
fn test_enrichment_option_field_filled_when_none() {
    // An `Option<T>` field: by default a `None` value is filled with `"n/a"`.
    let output = parse_and_generate(
        r"
        struct RequestContext {
            tenant: ClassifiedString,
            user_agent: Option<ClassifiedString>,
        }
        ",
    );
    insta::assert_snapshot!(output);
}

#[test]
fn test_enrichment_option_field_drop_when_none() {
    // `#[if_none(drop)]` pushes the entry only when `Some(..)`.
    let output = parse_and_generate(
        r"
        struct RequestContext {
            tenant: ClassifiedString,
            #[if_none(drop)]
            user_agent: Option<ClassifiedString>,
        }
        ",
    );
    insta::assert_snapshot!(output);
}

#[test]
fn test_error_if_none_on_non_option() {
    let err = parse_and_expect_error(
        r"
        struct RequestContext {
            #[if_none(drop)]
            tenant: ClassifiedString,
        }
        ",
    );
    assert!(err.contains("only valid on `Option<T>`"), "{err}");
}

#[test]
fn test_enrichment_with_dimension() {
    // The keyed forms route the log and metric signals independently; a bare
    // `metric` opts in under the field name and `log = "..."` renames the
    // log key only (no metric dimension).
    let output = parse_and_generate(
        r#"
        struct RequestContext {
            #[dimension(metric = "region")]
            #[unredacted]
            region: i64,
            #[dimension(metric)]
            #[unredacted]
            shard: i64,
            #[dimension(metric = "http.method")]
            method: ClassifiedString,
            #[dimension(log = "tenant")]
            tenant: ClassifiedString,
            #[dimension(log = "http.status", metric = "status")]
            #[unredacted]
            status: i64,
        }
        "#,
    );
    insta::assert_snapshot!(output);
}

#[test]
fn test_error_empty_dimension() {
    let err = parse_and_expect_error(
        r"
        struct BadEnrichment {
            #[dimension()]
            x: String,
        }
        ",
    );
    assert!(err.contains("requires at least one"), "{err}");
}

#[test]
fn union_is_rejected() {
    let err = parse_and_expect_error("union U { a: i64 }");
    assert!(err.contains("unions"), "{err}");
}
