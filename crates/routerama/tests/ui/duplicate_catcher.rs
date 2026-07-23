// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(unused_imports)]

use routerama::route::{FromRequestParts, RequestParts, StatusCode, router};

struct Reject;

impl<S: ?Sized> FromRequestParts<'_, S> for Reject {
    type Rejection = StatusCode;

    fn from_request_parts(_parts: &RequestParts, _state: &S) -> Result<Self, Self::Rejection> {
        Err(StatusCode::BAD_REQUEST)
    }
}

struct Api;

#[router]
impl Api {
    #[route(GET, "/")]
    async fn home(&self, _reject: Reject) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[catch(StatusCode, from = Reject)]
    async fn first(&self, rejection: StatusCode) -> StatusCode {
        rejection
    }

    #[catch(StatusCode, from = Reject)]
    async fn second(&self, rejection: StatusCode) -> StatusCode {
        rejection
    }
}

fn main() {}
