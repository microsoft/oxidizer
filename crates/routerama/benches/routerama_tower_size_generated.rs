// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Isolated compile and binary-size control for generated exact Tower routing.

#![expect(
    clippy::redundant_field_names,
    reason = "the nested router macro expansion retains explicit generated body field names"
)]

macro_rules! build_service {
    ($state:expr) => {
        Api::tower_service::<Body, _, _>(Api, $state)
    };
}

include!("common/tower_size_control.rs");
tower_size_control!(routerama::route::router(state = AppState, tower), build_service);
