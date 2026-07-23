// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::{Before, BeforeContext, StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/books/{slug}")]
    async fn book(&self, slug: &str) -> String {
        slug.to_owned()
    }

    #[before(book)]
    async fn guard(&self, _ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        Before::Next
    }
}

fn main() {}
