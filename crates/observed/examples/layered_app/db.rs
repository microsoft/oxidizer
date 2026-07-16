// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Database layer with its own telemetry events and metrics.
//!
//! All events in this module are emitted via `emit!()` and delivered to the DB
//! sink. Global enrichments (like `service.name`) are excluded from the lib
//! processor - only per-sink enrichments (like `db.pool`) are attached.

use data_privacy::classified;
use observed::enrichment::EnrichFnExt;
use observed::{Enrichment, Event, Sink, emit};

use crate::taxonomy::MicrosoftEnterpriseDataTaxonomy;

// ---------------------------------------------------------------------------
// Classified newtypes
// ---------------------------------------------------------------------------

/// Database system identifier.
#[classified(MicrosoftEnterpriseDataTaxonomy::PublicNonPersonalData)]
#[derive(Clone)]
struct DbSystem(&'static str);

// ---------------------------------------------------------------------------
// Enrichment types
// ---------------------------------------------------------------------------

#[derive(Enrichment)]
struct DbSystemEnrich {
    #[dimension(log = "db.system")]
    system: DbSystem,
}

// ---------------------------------------------------------------------------
// Database events
// ---------------------------------------------------------------------------

/// A completed database query.
///
/// Produces both a log record and a histogram metric on query duration.
#[derive(Event)]
#[event(name = "db.query")]
#[log(severity = info, message = "Database query executed")]
#[metric(kind = histogram, field = query_ms, name = "db.query.duration_ms")]
pub(crate) struct DbQuery {
    /// Query execution time - recorded as a histogram metric.
    #[unredacted]
    pub query_ms: f64,

    /// Number of rows returned.
    #[unredacted]
    pub rows_returned: i64,

    /// Numeric table identifier.
    #[unredacted]
    pub table_id: i64,
}

/// A database connection pool status snapshot.
///
/// Uses a gauge metric to report the current number of active connections.
#[derive(Event)]
#[event(name = "db.pool.status")]
#[log(severity = debug, message = "Connection pool status")]
#[metric(kind = gauge, field = active_connections, name = "db.pool.active_connections")]
pub(crate) struct DbPoolStatus {
    /// Current active connections - recorded as a gauge metric.
    #[unredacted]
    pub active_connections: f64,

    /// Maximum pool size - informational, included in logs and metrics.
    #[unredacted]
    pub max_connections: i64,
}

/// A slow-query warning.
#[derive(Event)]
#[event(name = "db.query.slow")]
#[log(severity = warn, message = "Slow query detected")]
#[metric(kind = histogram, field = query_ms, name = "db.slow_query.duration_ms")]
pub(crate) struct DbSlowQuery {
    /// Query execution time that exceeded the threshold.
    #[unredacted]
    pub query_ms: f64,

    /// The threshold that was exceeded.
    #[unredacted]
    pub threshold_ms: f64,

    /// Numeric table identifier.
    #[unredacted]
    pub table_id: i64,
}

// ---------------------------------------------------------------------------
// Public API simulating database operations
// ---------------------------------------------------------------------------

/// Simulates executing a SQL query and emitting telemetry.
///
/// Returns the number of rows "found".
pub(crate) fn query_users(sink: &Sink, table_id: i64) -> i64 {
    (|| {
        let rows_returned = 42;
        let query_ms = 4.7;

        // Emit the query event to the DB sink only.
        emit!(
            sink,
            DbQuery {
                query_ms,
                rows_returned,
                table_id,
            }
        );

        // Check for slow queries.
        let slow_threshold = 100.0;
        if query_ms > slow_threshold {
            emit!(
                sink,
                DbSlowQuery {
                    query_ms,
                    threshold_ms: slow_threshold,
                    table_id,
                }
            );
        }

        // Report pool status as a gauge.
        emit!(
            sink,
            DbPoolStatus {
                active_connections: 3.0,
                max_connections: 20,
            }
        );

        rows_returned
    })
    .enrich(
        sink,
        DbSystemEnrich {
            system: DbSystem("postgresql"),
        },
    )()
}
