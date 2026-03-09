// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared application state for the dashboard server.

use std::sync::Arc;

use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;

use crate::load_test::LoadTestMetrics;

/// Shared state passed to all axum handlers via `axum::extract::State`.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    /// Current Redis connection (set via `/api/connect`).
    connection: Mutex<Option<redis::aio::ConnectionManager>>,
    /// Handle + stop flag for the running load test, if any.
    load_test: Mutex<Option<LoadTestHandle>>,
    /// Broadcast channel for live metrics snapshots.
    metrics_tx: watch::Sender<Option<LoadTestMetrics>>,
    /// Receiver side — cloned into each SSE stream.
    metrics_rx: watch::Receiver<Option<LoadTestMetrics>>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

pub struct LoadTestHandle {
    pub join_handle: JoinHandle<()>,
    pub stop_flag: Arc<std::sync::atomic::AtomicBool>,
}

impl AppState {
    /// Creates a new `AppState` with no active connection.
    pub fn new() -> Self {
        let (metrics_tx, metrics_rx) = watch::channel(None);
        Self {
            inner: Arc::new(Inner {
                connection: Mutex::new(None),
                load_test: Mutex::new(None),
                metrics_tx,
                metrics_rx,
            }),
        }
    }

    /// Stores a new Redis connection.
    pub async fn set_connection(&self, conn: redis::aio::ConnectionManager) {
        *self.inner.connection.lock().await = Some(conn);
    }

    /// Returns a clone of the current connection, or `None`.
    pub async fn connection(&self) -> Option<redis::aio::ConnectionManager> {
        self.inner.connection.lock().await.clone()
    }

    /// Stores a running load test handle. Returns `false` if one is already running.
    ///
    /// Automatically clears a finished (but not yet stopped) previous test.
    pub async fn set_load_test(&self, handle: LoadTestHandle) -> bool {
        let mut guard = self.inner.load_test.lock().await;
        if let Some(existing) = guard.as_ref() {
            if !existing.join_handle.is_finished() {
                return false;
            }
            // Previous test finished naturally — clear it.
            guard.take();
        }
        *guard = Some(handle);
        true
    }

    /// Signals the running load test to stop and removes the handle.
    pub async fn stop_load_test(&self) -> bool {
        let mut guard = self.inner.load_test.lock().await;
        if let Some(handle) = guard.take() {
            handle
                .stop_flag
                .store(true, std::sync::atomic::Ordering::Relaxed);
            drop(guard);
            // Best-effort wait; don't block forever.
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle.join_handle)
                .await;
            true
        } else {
            false
        }
    }

    /// Returns the metrics watch sender (for the load test to broadcast snapshots).
    pub fn metrics_tx(&self) -> &watch::Sender<Option<LoadTestMetrics>> {
        &self.inner.metrics_tx
    }

    /// Returns a new receiver for the metrics watch channel.
    pub fn metrics_rx(&self) -> watch::Receiver<Option<LoadTestMetrics>> {
        self.inner.metrics_rx.clone()
    }
}
