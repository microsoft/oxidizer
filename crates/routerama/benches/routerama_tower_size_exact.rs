// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Isolated compile and binary-size control for `RouteService<ExactBody>`.

#![expect(
    clippy::redundant_field_names,
    reason = "the nested router macro expansion retains explicit generated body field names"
)]

use routerama::route::tower::RouteService;

macro_rules! build_service {
    ($state:expr) => {
        RouteService::new(Api, $state, |api: Api, state: Arc<AppState>, request: Request<Body>| async move {
            api.route(request, state.as_ref()).await
        })
    };
}

include!("common/tower_size_control.rs");
tower_size_control!(routerama::route::router(state = AppState), build_service);
