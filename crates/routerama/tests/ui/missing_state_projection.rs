// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::{State, StatusCode, router};

struct AppState;
struct MissingProjection;
struct Api;

#[router(state = AppState)]
impl Api {
    #[route(GET, "/")]
    async fn home(&self, state: State<MissingProjection>) -> StatusCode {
        let _ = state;
        StatusCode::NO_CONTENT
    }
}

fn main() {}
