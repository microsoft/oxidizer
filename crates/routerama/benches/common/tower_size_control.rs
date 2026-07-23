// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared isolated compile- and binary-size control for the three Tower
// response boundaries. Each target supplies only its router attribute and
// service construction so the routed workload remains identical.

macro_rules! tower_size_control {
    ($router_attr:meta, $build_service:ident) => {
        use std::future::Future;
        use std::pin::pin;
        use std::sync::Arc;
        use std::task::{Context, Poll, Waker};

        use bytes::Bytes;
        use http_body::Body as HttpBody;
        use routerama::response::{Body, Response};
        use routerama::route::{Request, State};
        use tower_service::Service as _;

        #[derive(Clone)]
        struct AppState {
            deployment: &'static str,
            routing_seed: [u64; 16],
        }

        #[derive(Clone, Copy)]
        struct Api;

        #[allow(
            clippy::allow_attributes,
            unknown_lints,
            clippy::unused_async,
            clippy::unused_async_trait_impl,
            reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
        )]
        #[$router_attr]
        impl Api {
            #[route(GET, "/health")]
            async fn health(&self, state: State<AppState>) -> Bytes {
                let _ = std::hint::black_box(state.deployment);
                let _ = std::hint::black_box(state.routing_seed[0]);
                Bytes::from_static(b"served")
            }
        }

        fn run_ready<F>(future: F) -> F::Output
        where
            F: Future,
        {
            let mut future = pin!(future);
            let mut context = Context::from_waker(Waker::noop());
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => output,
                Poll::Pending => panic!("the in-memory generated route future must complete in one poll"),
            }
        }

        fn observe<B>(response: Response<B>) -> (u16, usize)
        where
            B: HttpBody<Data = Bytes>,
        {
            let status = response.status().as_u16();
            let mut body = pin!(response.into_body());
            let mut context = Context::from_waker(Waker::noop());
            let mut bytes = 0;
            loop {
                match body.as_mut().poll_frame(&mut context) {
                    Poll::Ready(Some(Ok(frame))) => {
                        bytes += frame.data_ref().map_or(0, Bytes::len);
                    }
                    Poll::Ready(Some(Err(_))) => panic!("the size-control body must not fail"),
                    Poll::Ready(None) => break,
                    Poll::Pending => panic!("the in-memory size-control body must always be ready"),
                }
            }
            (status, bytes)
        }

        fn main() {
            let state = Arc::new(AppState {
                deployment: "west",
                routing_seed: [0xfeed_face_dead_beef; 16],
            });
            let mut service = $build_service!(state);
            let request = Request::get("/health")
                .body(Body::empty())
                .expect("the size-control request is valid");
            let response = run_ready(service.call(std::hint::black_box(request))).expect("routing is infallible");
            std::hint::black_box(observe(response));
        }
    };
}
