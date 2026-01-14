// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    reason = "Benchmarks don't require documentation and should fail fast on errors"
)]

use std::future::poll_fn;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering::Relaxed;
use std::task::Poll;

use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use layered::tower::{Adapter, tower_layer};
use layered::{Execute, Service, Stack as OxStack};
use pin_project_lite::pin_project;
use tower::{ServiceBuilder, service_fn};
use tower_layer::Layer;
use tower_service::Service as TowerService;

fn entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("tower");

    // Execute
    let mut service = service_fn(|v| async move { Ok::<_, ()>(v) });
    group.bench_function("execute_tower", |b| {
        b.iter(|| {
            block_on(async {
                poll_fn(|cx| service.poll_ready(cx)).await.unwrap();
                service.call(0).await.unwrap();
            });
        });
    });

    let service = Execute::new(|v| async move { Ok::<_, ()>(v) });
    group.bench_function("execute_oxidizer", |b| {
        b.iter(|| {
            block_on(async {
                service.execute(0).await.unwrap();
            });
        });
    });

    let service = Adapter(service_fn(|v| async move { Ok::<_, ()>(v) }));
    group.bench_function("execute_tower_with_adapter", |b| {
        b.iter(|| {
            block_on(async {
                service.execute(0).await.unwrap();
            });
        });
    });

    let mut service = Adapter(Execute::new(|v| async move { Ok::<_, ()>(v) }));
    group.bench_function("execute_oxidizer_with_adapter", |b| {
        b.iter(|| {
            block_on(async {
                poll_fn(|cx| service.poll_ready(cx)).await.unwrap();
                service.call(0).await.unwrap();
            });
        });
    });

    // With middleware
    let layer = Middleware::layer();

    let mut service = ServiceBuilder::new()
        .layer(layer.clone())
        .service_fn(|v: u32| async move { Ok::<_, ()>(v) });
    group.bench_function("middleware_tower", |b| {
        b.iter(|| {
            block_on(async {
                poll_fn(|cx| service.poll_ready(cx)).await.unwrap();
                service.call(0).await.unwrap();
            });
        });
    });

    let service = (layer.clone(), Execute::new(|v| async move { Ok::<_, ()>(v) })).build();
    group.bench_function("middleware_oxidizer", |b| {
        b.iter(|| {
            block_on(async {
                service.execute(0).await.unwrap();
            });
        });
    });

    let mut service = ServiceBuilder::new()
        .layer(tower_layer(layer.clone()))
        .service_fn(|v: u32| async move { Ok::<_, ()>(v) });
    group.bench_function("middleware_tower_with_adapter", |b| {
        b.iter(|| {
            block_on(async {
                poll_fn(|cx| service.poll_ready(cx)).await.unwrap();
                service.call(0).await.unwrap();
            });
        });
    });

    let service = (tower_layer(layer), Execute::new(|v| async move { Ok::<_, ()>(v) })).build();
    group.bench_function("middleware_oxidizer_with_adapter", |b| {
        b.iter(|| {
            block_on(async {
                service.execute(0).await.unwrap();
            });
        });
    });
}

criterion_group!(benches, entry);
criterion_main!(benches);

/// Simple middleware that counts the number of concurrent requests.
#[derive(Clone, Debug)]
struct Middleware<S> {
    executing: Arc<AtomicU32>,
    inner: S,
}

impl Middleware<()> {
    fn layer() -> MiddlewareLayer {
        MiddlewareLayer
    }
}

impl<S> Middleware<S> {
    fn new(inner: S) -> Self {
        Self {
            executing: Arc::new(AtomicU32::new(0)),
            inner,
        }
    }
}

#[derive(Clone)]
struct MiddlewareLayer;

impl<S> Layer<S> for MiddlewareLayer {
    type Service = Middleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Middleware::new(inner)
    }
}

impl<S, In: Send> Service<In> for Middleware<S>
where
    S: Service<In>,
{
    type Out = S::Out;

    async fn execute(&self, input: In) -> Self::Out {
        self.executing.fetch_add(1, Relaxed);
        let result = self.inner.execute(input).await;
        self.executing.fetch_sub(1, Relaxed);
        result
    }
}

impl<S, In> TowerService<In> for Middleware<S>
where
    S: TowerService<In>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = TowerFuture<S::Future>;
    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: In) -> Self::Future {
        self.executing.fetch_add(1, Relaxed);
        let fut = self.inner.call(req);

        TowerFuture {
            executing: Arc::clone(&self.executing),
            future: fut,
        }
    }
}

pin_project! {
    struct TowerFuture<F> {
        executing: Arc<AtomicU32>,
        #[pin]
        future: F,
    }
}

impl<F, T, E> Future for TowerFuture<F>
where
    F: Future<Output = Result<T, E>>,
{
    type Output = Result<T, E>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        match this.future.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(val) => {
                this.executing.fetch_sub(1, Relaxed);
                Poll::Ready(val)
            }
        }
    }
}
