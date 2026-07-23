// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::{Request, StatusCode, router};

struct AppState;
struct OtherState;
struct Api;

#[router(state = AppState)]
impl Api {
    #[route(GET, "/")]
    async fn home(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

async fn wrong_state() {
    let request = Request::get("/").body(()).expect("valid request");
    let _response = Api.route(request, &OtherState).await;
}

fn main() {}
