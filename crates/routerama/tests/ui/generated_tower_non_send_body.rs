// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::rc::Rc;

use routerama::route::{StatusCode, router};

#[derive(Clone, Copy)]
struct Api;

#[router(state = (), tower)]
impl Api {
    #[route(GET, "/")]
    async fn home(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

fn main() {
    let _ = Api::tower_service::<Rc<()>, _, _>(Api, ());
}
