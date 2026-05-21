// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Adapts a layered [`Connect`] service into a hyper-compatible
//! [`tower::Service<Uri>`].

use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

use http::Uri;
use http_extensions::{HttpError, Result};
use templated_uri::BaseUri;
use tower::Service;

use crate::connection::connect::Connect;
use crate::connection::io::HyperIo;

/// Wraps a [`Connect`] so it can be passed to hyper-util's `legacy::Client`.
pub(crate) struct HyperConnectorAdapter<C, S>(C, PhantomData<fn() -> S>);

impl<C, S> HyperConnectorAdapter<C, S> {
    pub(crate) fn new(connector: C) -> Self {
        Self(connector, PhantomData)
    }
}

impl<C: Clone, S> Clone for HyperConnectorAdapter<C, S> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), PhantomData)
    }
}

impl<C, S> Service<Uri> for HyperConnectorAdapter<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    type Response = S;
    type Error = HttpError;
    type Future = Pin<Box<dyn Future<Output = Result<S>> + Send + 'static>>;

    // `Poll::from(Ok(()))` is constructed identically to `Poll::Ready(Ok(()))`,
    // so the mutation produces an equivalent program.
    #[cfg_attr(test, mutants::skip)]
    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Uri) -> Self::Future {
        let connector = self.0.clone();

        Box::pin(async move {
            let base_uri = BaseUri::try_from(&req)?;
            connector.execute(base_uri).await
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::task::Waker;

    use bytes::Bytes;
    use tick::Clock;

    use super::*;
    use crate::testing::FakeConnector;

    #[test]
    fn poll_ready_is_immediately_ready() {
        let mut adapter = HyperConnectorAdapter::new(FakeConnector::new_success(Bytes::new(), Clock::new_frozen()));
        let mut cx = Context::from_waker(Waker::noop());
        let poll = adapter.poll_ready(&mut cx);
        assert!(matches!(poll, Poll::Ready(Ok(()))));
    }

    #[tokio::test]
    async fn call_translates_uri_into_base_uri_and_invokes_connector() {
        let mut adapter = HyperConnectorAdapter::new(FakeConnector::new_success(
            Bytes::from_static(b""),
            tick::ClockControl::new().auto_advance_timers(true).to_clock(),
        ));
        adapter.call(Uri::from_static("https://example.com/")).await.unwrap();
    }

    #[tokio::test]
    async fn call_propagates_invalid_uri_error() {
        let mut adapter = HyperConnectorAdapter::new(FakeConnector::new_success(
            Bytes::from_static(b""),
            tick::ClockControl::new().auto_advance_timers(true).to_clock(),
        ));
        // A relative URI (no scheme/authority) is not a valid BaseUri.
        adapter
            .call(Uri::from_static("/relative/path"))
            .await
            .expect_err("relative URI should not parse as a BaseUri");
    }
}
