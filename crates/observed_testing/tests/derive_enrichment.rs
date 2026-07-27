// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the `#[derive(Enrichment)]` macro.
//!
//! Verifies that the derive macro correctly generates `Enrichment` trait
//! and `IntoIterator` implementations, and that all field-level annotations
//! work as expected:
//! - `#[dimension(log = "...")]` / `#[dimension(log = exclude)]` (field rename / exclude from logs)
//! - `#[unredacted]` / `#[data_class(...)]` (redaction control)
//!
//! These tests do **not** use `emit!` - they exercise only the derived trait.

use observed::Enrichment;
use observed::enrichment::Enrichment as _;
use observed_testing::ExpectedEnrichmentEntry;
use observed_testing::types::PublicString;

#[test]
fn into_entries_returns_all_fields() {
    #[derive(Debug, Enrichment)]
    struct SimpleCtx {
        #[unredacted]
        tenant_id: i64,
        #[unredacted]
        is_active: bool,
    }

    let ctx = SimpleCtx {
        tenant_id: 42,
        is_active: true,
    };
    let entries = ctx.into_entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0], ExpectedEnrichmentEntry::new("tenant_id", 42i64));
    assert_eq!(entries[1], ExpectedEnrichmentEntry::new("is_active", true));
}

// ================================================================================================
// Field rename: #[dimension(log = "...")]
// ================================================================================================

#[test]
fn renamed_entries_have_new_keys() {
    #[derive(Debug, Enrichment)]
    struct RenamedCtx {
        #[dimension(log = "http.method")]
        method: PublicString,
        #[dimension(log = "http.status_code")]
        #[unredacted]
        status: i64,
        #[unredacted]
        untouched: bool,
    }

    let ctx = RenamedCtx {
        method: PublicString("GET".into()),
        status: 200,
        untouched: true,
    };
    let entries = ctx.into_entries();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0], ExpectedEnrichmentEntry::new("http.method", "GET"));
    assert_eq!(entries[1], ExpectedEnrichmentEntry::new("http.status_code", 200i64));
    assert_eq!(entries[2], ExpectedEnrichmentEntry::new("untouched", true));
}

// ================================================================================================
// Exclude flags: exclude_from_logs
// ================================================================================================

#[test]
fn exclude_flags_on_all_entries() {
    #[derive(Debug, Enrichment)]
    struct ExcludeCtx {
        #[dimension(log = exclude)]
        #[unredacted]
        metric_only: i64,
        #[unredacted]
        log_only: i64,
        #[unredacted]
        both: i64,
    }

    let ctx = ExcludeCtx {
        metric_only: 1,
        log_only: 2,
        both: 3,
    };
    let entries = ctx.into_entries();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0], ExpectedEnrichmentEntry::new("metric_only", 1i64).exclude_from_logs());
    assert_eq!(entries[1], ExpectedEnrichmentEntry::new("log_only", 2i64));
    assert_eq!(entries[2], ExpectedEnrichmentEntry::new("both", 3i64));
}

// ================================================================================================
// Metric dimension opt-in: #[dimension(metric = "...")]
// ================================================================================================

#[test]
fn dimension_opts_field_into_metric_dimension() {
    #[derive(Debug, Enrichment)]
    struct MetricCtx {
        #[dimension(metric = "plain_dim")]
        #[unredacted]
        plain_dim: i64,
        #[dimension(metric = "ctx.renamed")]
        #[unredacted]
        renamed_dim: i64,
        #[dimension(log = exclude, metric = "metric_only")]
        #[unredacted]
        metric_only: i64,
        #[unredacted]
        log_only: i64,
    }

    let ctx = MetricCtx {
        plain_dim: 1,
        renamed_dim: 2,
        metric_only: 3,
        log_only: 4,
    };
    let entries = ctx.into_entries();
    assert_eq!(entries.len(), 4);
    // #[dimension(metric = "plain_dim")]: dimension keyed by the field name.
    assert_eq!(entries[0], ExpectedEnrichmentEntry::new("plain_dim", 1i64).metric_dimension());
    // #[dimension(metric = ...)]: the dimension key is renamed, log key unchanged.
    assert_eq!(
        entries[1],
        ExpectedEnrichmentEntry::new("renamed_dim", 2i64).metric_dimension_named("ctx.renamed")
    );
    // Metric-only enrichment: excluded from logs but still a dimension.
    assert_eq!(
        entries[2],
        ExpectedEnrichmentEntry::new("metric_only", 3i64)
            .exclude_from_logs()
            .metric_dimension()
    );
    // No #[dimension]: not a dimension.
    assert_eq!(entries[3], ExpectedEnrichmentEntry::new("log_only", 4i64));
}

// ================================================================================================
// Combined: all field attributes together
// ================================================================================================

#[test]
fn combined_entries() {
    #[derive(Debug, Enrichment)]
    struct FullCtx {
        #[dimension(log = "ctx.trace_id")]
        #[unredacted]
        trace_id: i64,
        #[dimension(log = "ctx.request_id")]
        request_id: PublicString,
        #[dimension(log = exclude)]
        #[unredacted]
        internal_tag: i64,
        #[unredacted]
        plain: bool,
    }

    let ctx = FullCtx {
        trace_id: 1,
        request_id: PublicString("abc".into()),
        internal_tag: 2,
        plain: true,
    };
    let entries = ctx.into_entries();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0], ExpectedEnrichmentEntry::new("ctx.trace_id", 1i64));
    assert_eq!(entries[1], ExpectedEnrichmentEntry::new("ctx.request_id", "abc"));
    assert_eq!(entries[2], ExpectedEnrichmentEntry::new("internal_tag", 2i64).exclude_from_logs());
    assert_eq!(entries[3], ExpectedEnrichmentEntry::new("plain", true));
}

// ================================================================================================
// Primitive vs classified value types
// ================================================================================================

#[test]
fn mixed_value_types_in_entries() {
    use observed_testing::types::PiiString;

    #[derive(Debug, Enrichment)]
    struct MixedCtx {
        #[unredacted]
        count: i64,
        #[unredacted]
        ratio: f64,
        #[unredacted]
        flag: bool,
        label: PublicString,
        user_email: PiiString,
    }

    let ctx = MixedCtx {
        count: 5,
        ratio: 0.75,
        flag: true,
        label: PublicString("prod".into()),
        user_email: PiiString("user@example.com".into()),
    };
    let entries = ctx.into_entries();
    assert_eq!(entries.len(), 5);
    assert_eq!(entries[0], ExpectedEnrichmentEntry::new("count", 5i64));
    assert_eq!(entries[1], ExpectedEnrichmentEntry::new("ratio", 0.75));
    assert_eq!(entries[2], ExpectedEnrichmentEntry::new("flag", true));
    assert_eq!(entries[3], ExpectedEnrichmentEntry::new("label", "prod"));
    assert_eq!(entries[4], ExpectedEnrichmentEntry::new("user_email", "user@example.com"));
}

// ================================================================================================
// Empty struct
// ================================================================================================

#[test]
fn empty_enrichment_produces_no_entries() {
    #[derive(Debug, Enrichment)]
    #[expect(
        clippy::empty_structs_with_brackets,
        reason = "the Enrichment derive only supports structs with named fields"
    )]
    struct EmptyCtx {}

    let ctx = EmptyCtx {};
    let entries = ctx.into_entries();
    assert!(entries.is_empty());
}

// ================================================================================================
// Lifetime support
// ================================================================================================

#[test]
fn borrowed_enrichment_single_lifetime() {
    #[derive(Debug, Enrichment)]
    struct BorrowedCtx<'a> {
        #[unredacted]
        label: &'a str,
        #[unredacted]
        count: i64,
    }

    let label = String::from("borrowed");
    let ctx = BorrowedCtx { label: &label, count: 7 };
    let entries = ctx.into_entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0], ExpectedEnrichmentEntry::new("label", "borrowed"));
    assert_eq!(entries[1], ExpectedEnrichmentEntry::new("count", 7i64));
}

#[test]
fn borrowed_enrichment_multiple_lifetimes() {
    #[derive(Debug, Enrichment)]
    struct MultiCtx<'a, 'b> {
        #[unredacted]
        first: &'a str,
        #[unredacted]
        second: &'b str,
    }

    let a = String::from("alpha");
    let b = String::from("beta");
    let ctx = MultiCtx { first: &a, second: &b };
    let entries = ctx.into_entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0], ExpectedEnrichmentEntry::new("first", "alpha"));
    assert_eq!(entries[1], ExpectedEnrichmentEntry::new("second", "beta"));
}
