// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Memory-provider propagation through the public custom-transport API.

use std::sync::{Arc, Mutex};

use bytesbuf::mem::{GlobalPool, HasMemory, Memory, OpaqueMemory};
use bytesbuf::{BytesBuf, BytesView};
use fetch::custom::{CustomContext, CustomDeps, Isolation, create_builder};
#[cfg(all(feature = "tokio", feature = "rustls"))]
use fetch::tokio::TokioDeps;
use fetch::{HttpBodyBuilder, HttpClient, HttpRequest, HttpResponse};
use futures::executor::block_on;
use http_extensions::HttpBodyOptions;
use layered::Service;
use ohno::Labeled as _;
use thread_aware::{Relocator, Thread, ThreadAware};
use tick::Clock;

/// Observes allocations without depending on any global memory-pool metrics.
#[derive(Debug)]
struct ObservedMemory {
    reservations: Arc<Mutex<Vec<Reservation>>>,
    generation: usize,
    relocated: bool,
}

/// Identifies the configured provider instance that serves each allocation.
#[derive(Debug)]
struct Reservation {
    bytes: usize,
    generation: usize,
    relocated: bool,
}

impl Clone for ObservedMemory {
    fn clone(&self) -> Self {
        Self {
            reservations: Arc::clone(&self.reservations),
            generation: self.generation + 1,
            relocated: self.relocated,
        }
    }
}

impl ThreadAware for ObservedMemory {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {
        self.relocated = true;
    }
}

impl Memory for ObservedMemory {
    #[expect(clippy::unwrap_used, reason = "test provider must fail if its observation lock is poisoned")]
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.reservations.lock().unwrap().push(Reservation {
            bytes: min_bytes,
            generation: self.generation,
            relocated: self.relocated,
        });
        GlobalPool::new().reserve(min_bytes)
    }
}

/// Allocates response frames through the factory's body builder, without network I/O.
struct InMemoryTransport {
    body_builder: HttpBodyBuilder,
}

impl Service<HttpRequest> for InMemoryTransport {
    type Out = fetch::Result<HttpResponse>;

    #[expect(clippy::unwrap_used, reason = "test transport only accepts buffered request bodies")]
    async fn execute(&self, input: HttpRequest) -> Self::Out {
        assert_eq!(BytesView::try_from(input.into_body()).unwrap(), b"request payload");
        let frames = [b"reply " as &[u8], b"payload"].map(|data| Ok(BytesView::copied_from_slice(data, &self.body_builder)));
        Ok(HttpResponse::new(
            self.body_builder.stream(futures::stream::iter(frames), &HttpBodyOptions::default()),
        ))
    }
}

/// Builds the same deterministic handler under both supported isolation models.
fn client(isolation: Isolation, reservations: &Arc<Mutex<Vec<Reservation>>>, options: HttpBodyOptions) -> HttpClient {
    create_builder(
        "test",
        "in-memory",
        |context: CustomContext| InMemoryTransport {
            body_builder: context.body_builder,
        },
        isolation,
        CustomDeps {
            clock: Clock::new_frozen(),
            memory: OpaqueMemory::new(ObservedMemory {
                reservations: Arc::clone(reservations),
                generation: 0,
                relocated: false,
            }),
            extras: (),
        },
    )
    .insecure_allow_http()
    .minimal_pipeline()
    .response_body_options(options)
    .build()
}

#[test]
fn custom_memory_reaches_requests_responses_and_client_memory() {
    for isolation in [Isolation::Shared, Isolation::Isolated] {
        let reservations = Arc::new(Mutex::new(Vec::new()));
        let client = client(isolation, &reservations, HttpBodyOptions::default());
        let response = block_on(client.post("http://example.test/").text("request payload").fetch()).unwrap();
        assert_eq!(block_on(response.into_body().into_text()).unwrap(), "reply payload");
        assert!(client.reserve(19).capacity() >= 19);
        assert!(client.memory().reserve(23).capacity() >= 23);

        let observed = reservations.lock().unwrap();
        assert_eq!(observed.iter().map(|entry| entry.bytes).collect::<Vec<_>>(), [15, 6, 7, 19, 23]);
        assert!(observed.iter().all(|entry| entry.generation > 0));
        assert!(observed.iter().all(|entry| !entry.relocated));
    }
}

#[test]
fn relocating_a_client_clone_reaches_its_provider() {
    for isolation in [Isolation::Shared, Isolation::Isolated] {
        let reservations = Arc::new(Mutex::new(Vec::new()));
        let client = client(isolation, &reservations, HttpBodyOptions::default());
        let mut relocated = client.clone();
        _ = Relocator::between_threads().source(false).relocate(&mut relocated);

        assert!(relocated.reserve(17).capacity() >= 17);
        assert!(client.reserve(18).capacity() >= 18);
        let observed = reservations.lock().unwrap();
        assert_eq!(observed.len(), 2);
        assert!(observed[0].relocated);
        assert!(!observed[1].relocated);
        assert!(observed[0].generation > observed[1].generation);
        drop(observed);

        // An isolated transport must also rebuild its factory context with relocated memory.
        let response = block_on(relocated.post("http://example.test/").text("request payload").fetch()).unwrap();
        assert_eq!(block_on(response.into_body().into_text()).unwrap(), "reply payload");
        let observed = reservations.lock().unwrap();
        assert!(observed[2].relocated);
        if isolation == Isolation::Isolated {
            assert!(observed[3..].iter().all(|entry| entry.relocated));
        }
    }
}

#[test]
fn custom_memory_preserves_response_buffer_limits() {
    let reservations = Arc::new(Mutex::new(Vec::new()));
    // The limit fits either frame alone, but not the complete response.
    let client = client(Isolation::Shared, &reservations, HttpBodyOptions::default().buffer_limit(8));
    let response = block_on(client.post("http://example.test/").text("request payload").fetch()).unwrap();

    let error = block_on(response.into_body().into_buffered()).unwrap_err();
    assert_eq!(error.label(), "body_size_limit");
    assert_eq!(
        reservations.lock().unwrap().iter().map(|entry| entry.bytes).collect::<Vec<_>>(),
        [15, 6, 7]
    );
}

#[cfg(all(feature = "tokio", feature = "rustls"))]
#[test]
fn tokio_dependencies_preserve_caller_selected_memory() {
    let reservations = Arc::new(Mutex::new(Vec::new()));
    let client = HttpClient::builder_tokio(TokioDeps {
        clock: Clock::new_frozen(),
        memory: OpaqueMemory::new(ObservedMemory {
            reservations: Arc::clone(&reservations),
            generation: 0,
            relocated: false,
        }),
    })
    .minimal_pipeline()
    .build();

    assert!(client.reserve(29).capacity() >= 29);
    assert!(client.memory().reserve(31).capacity() >= 31);
    assert_eq!(
        reservations.lock().unwrap().iter().map(|entry| entry.bytes).collect::<Vec<_>>(),
        [29, 31]
    );
}

testing_aids::init_tracing!();
