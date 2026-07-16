// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Authentication layer with its own telemetry events and metrics.
//!
//! All events in this module are emitted via `emit!()` and delivered to the
//! token issuer sink. Global enrichments are excluded from the lib
//! processor - only per-sink enrichments (like `token.issuer.version`)
//! are attached.

use data_privacy::{DataClass, classified};
use observed::enrichment::EnrichFnExt;
use observed::{Enrichment, Event, Sink, emit};

use crate::taxonomy::MicrosoftEnterpriseDataTaxonomy;

const DC: DataClass = DataClass::new("microsoft", "PublicNonPersonalData");

// ---------------------------------------------------------------------------
// Classified newtypes
// ---------------------------------------------------------------------------

/// Token type identifier (e.g. "bearer").
#[classified(MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData)]
#[derive(Clone)]
struct TokenType(&'static str);

/// Token type numeric identifier.
#[classified(MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData)]
#[derive(Clone)]
struct TokenTypeId(i64);

// ---------------------------------------------------------------------------
// Enrichment types
// ---------------------------------------------------------------------------

#[derive(Enrichment)]
struct TokenTypeEnrich {
    #[dimension(log = "token.type")]
    token_type: TokenType,
}

#[derive(Enrichment)]
struct TokenTypeIdEnrich {
    #[dimension(log = "token.type_id")]
    token_type_id: TokenTypeId,
}

// ---------------------------------------------------------------------------
// Token issuer events
// ---------------------------------------------------------------------------

/// A successfully validated authentication token.
///
/// Records token validation latency as a histogram metric.
#[derive(Event)]
#[event(name = "token.validated")]
#[log(severity = info, message = "Token validated successfully")]
#[metric(kind = histogram, field = validation_ms, name = "token.validation.duration_ms")]
struct TokenValidated {
    /// Token validation duration - recorded as a histogram metric.
    // TODO: replace #[unredacted] with classified type once metric fields support non-numeric Values
    #[unredacted]
    validation_ms: f64,

    /// Algorithm identifier (e.g. 1 = RS256, 2 = ES256).
    #[data_class(DC)]
    algorithm_id: i64,
}

/// A failed token validation attempt.
///
/// Records each failure as an up-down counter metric (increments on failure).
#[derive(Event)]
#[event(name = "token.validation_failed")]
#[log(severity = warn, message = "Token validation failed")]
#[metric(kind = updown_counter, field = failure_count, name = "token.validation.failures")]
struct TokenValidationFailed {
    /// Failure count - recorded as an up-down counter metric.
    // TODO: replace #[unredacted] with classified type once metric fields support non-numeric Values
    #[unredacted]
    failure_count: i64,

    /// Error code identifying the failure reason.
    #[data_class(DC)]
    error_code: i64,
}

/// A token issuance event (for refresh / new token flows).
///
/// Records token issuance latency as a histogram metric.
#[derive(Event)]
#[event(name = "token.issued")]
#[log(severity = info, message = "New token issued")]
#[metric(kind = histogram, field = issuance_ms, name = "token.issuance.duration_ms")]
struct TokenIssued {
    /// Issuance duration - recorded as a histogram metric.
    // TODO: replace #[unredacted] with classified type once metric fields support non-numeric Values
    #[unredacted]
    issuance_ms: f64,

    /// Token type identifier (e.g. 1 = access, 2 = refresh).
    #[data_class(DC)]
    token_type_id: i64,

    /// Token time-to-live in seconds.
    #[data_class(DC)]
    ttl_seconds: i64,
}

// ---------------------------------------------------------------------------
// Public API simulating token operations
// ---------------------------------------------------------------------------

/// Simulates validating an incoming bearer token.
///
/// Emits either a [`TokenValidated`] or [`TokenValidationFailed`] event
/// depending on whether the token is valid.
pub(crate) fn validate_token(sink: &Sink, valid: bool) {
    (|| {
        if valid {
            // Successful validation.
            emit!(
                sink,
                TokenValidated {
                    validation_ms: 0.8,
                    algorithm_id: 1, // RS256
                }
            );
        } else {
            // Validation failure.
            emit!(
                sink,
                TokenValidationFailed {
                    failure_count: 1,
                    error_code: 401, // expired
                }
            );
        }
    })
    .enrich(
        sink,
        TokenTypeEnrich {
            token_type: TokenType("bearer"),
        },
    )();
}

/// Simulates issuing a new access token.
///
/// Emits a [`TokenIssued`] event with issuance latency.
#[expect(dead_code, reason = "available for extended example scenarios")]
pub(crate) fn issue_token(sink: &Sink, token_type_id: i64) {
    (|| {
        emit!(
            sink,
            TokenIssued {
                issuance_ms: 2.3,
                token_type_id,
                ttl_seconds: 3600,
            }
        );
    })
    .enrich(
        sink,
        TokenTypeIdEnrich {
            token_type_id: TokenTypeId(token_type_id),
        },
    )();
}
