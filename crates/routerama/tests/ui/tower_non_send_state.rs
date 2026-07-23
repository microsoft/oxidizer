// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::rc::Rc;

use routerama::response::{Body, Response};
use routerama::route::Request;
use routerama::route::tower::RouteService;

fn main() {
    // The adapter inherits its auto traits from what it stores, so a local
    // state value keeps the whole transport service local.
    let service = RouteService::new((), Rc::new(0_u32), |(): (), _state: Rc<u32>, request: Request<Body>| async move {
        let _ = request;
        Response::new(Body::empty())
    });

    assert_send(service);
}

fn assert_send<S: Send>(service: S) {
    let _ = service;
}
