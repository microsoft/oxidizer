// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::tower::RouteService;

fn main() {
    let _ = RouteService::new((), (), |(): (), (): (), request: routerama::route::Request<()>| async move {
        let _ = request;
        routerama::response::Response::new(routerama::response::Body::empty())
    });
}
