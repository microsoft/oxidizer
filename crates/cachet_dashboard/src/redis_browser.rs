// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Raw Redis helpers for browsing keys, values, and TTLs.

use redis::AsyncCommands;
use serde::Serialize;

/// Result of a paginated SCAN operation.
#[derive(Debug, Serialize)]
pub struct ScanResult {
    pub keys: Vec<KeyInfo>,
    pub next_cursor: u64,
}

/// Summary info for a single Redis key.
#[derive(Debug, Serialize)]
pub struct KeyInfo {
    pub key: String,
    pub key_type: String,
    pub ttl: i64,
}

/// Detailed info for a single key including its value.
#[derive(Debug, Serialize)]
pub struct KeyDetail {
    pub key: String,
    pub key_type: String,
    pub ttl: i64,
    pub value: Option<String>,
    /// If the value is a JSON-serialized `CacheEntry`, we extract metadata.
    pub cache_entry: Option<CacheEntryMeta>,
}

/// Extracted metadata from a `CacheEntry<serde_json::Value>` stored in Redis.
#[derive(Debug, Serialize)]
pub struct CacheEntryMeta {
    pub value: serde_json::Value,
    pub cached_at: Option<f64>,
    pub ttl_secs: Option<f64>,
}

/// SCAN keys matching `pattern`, returning up to `count` keys per batch.
///
/// Also pipelines TTL and TYPE queries for each returned key.
pub async fn scan_keys(
    conn: &mut redis::aio::ConnectionManager,
    pattern: &str,
    cursor: u64,
    count: usize,
) -> Result<ScanResult, redis::RedisError> {
    let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
        .arg(cursor)
        .arg("MATCH")
        .arg(pattern)
        .arg("COUNT")
        .arg(count)
        .query_async(conn)
        .await?;

    if keys.is_empty() {
        return Ok(ScanResult {
            keys: Vec::new(),
            next_cursor,
        });
    }

    // Pipeline TTL + TYPE for every key in the batch.
    let mut pipe = redis::pipe();
    for key in &keys {
        pipe.cmd("TTL").arg(key);
        pipe.cmd("TYPE").arg(key);
    }
    let results: Vec<redis::Value> = pipe.query_async(conn).await?;

    let mut infos = Vec::with_capacity(keys.len());
    for (i, key) in keys.into_iter().enumerate() {
        let ttl = match results.get(i * 2) {
            Some(redis::Value::Int(v)) => *v,
            _ => -1,
        };
        let key_type = match results.get(i * 2 + 1) {
            Some(redis::Value::SimpleString(s)) => s.clone(),
            Some(redis::Value::BulkString(b)) => String::from_utf8_lossy(b).into_owned(),
            _ => "unknown".to_string(),
        };
        infos.push(KeyInfo { key, key_type, ttl });
    }

    Ok(ScanResult {
        keys: infos,
        next_cursor,
    })
}

/// Get full details for a single key.
pub async fn get_key_detail(
    conn: &mut redis::aio::ConnectionManager,
    key: &str,
) -> Result<KeyDetail, redis::RedisError> {
    let key_type: String = redis::cmd("TYPE").arg(key).query_async(conn).await?;
    let ttl: i64 = conn.ttl(key).await?;

    let value: Option<String> = if key_type == "string" {
        conn.get(key).await?
    } else {
        None
    };

    let cache_entry = value.as_ref().and_then(|v| parse_cache_entry(v));

    Ok(KeyDetail {
        key: key.to_string(),
        key_type,
        ttl,
        value,
        cache_entry,
    })
}

/// Attempt to parse a JSON string as a `CacheEntry<Value>` and extract metadata.
fn parse_cache_entry(json: &str) -> Option<CacheEntryMeta> {
    #[derive(serde::Deserialize)]
    struct RawCacheEntry {
        value: serde_json::Value,
        cached_at: Option<RawSystemTime>,
        ttl: Option<RawDuration>,
    }

    #[derive(serde::Deserialize)]
    struct RawSystemTime {
        secs_since_epoch: u64,
        nanos_since_epoch: u32,
    }

    #[derive(serde::Deserialize)]
    struct RawDuration {
        secs: u64,
        nanos: u32,
    }

    let entry: RawCacheEntry = serde_json::from_str(json).ok()?;
    Some(CacheEntryMeta {
        value: entry.value,
        cached_at: entry.cached_at.map(|t| {
            #[expect(clippy::cast_precision_loss, reason = "timestamps are fine as f64")]
            let secs = t.secs_since_epoch as f64;
            secs + f64::from(t.nanos_since_epoch) / 1_000_000_000.0
        }),
        ttl_secs: entry.ttl.map(|d| {
            #[expect(clippy::cast_precision_loss, reason = "duration is fine as f64")]
            let secs = d.secs as f64;
            secs + f64::from(d.nanos) / 1_000_000_000.0
        }),
    })
}
