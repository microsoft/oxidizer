// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::{FromRequestParts, RequestParts, StatusCode, router};

struct AppState;
struct OtherState;
struct OtherStateExtractor;

impl FromRequestParts<'_, OtherState> for OtherStateExtractor {
    type Rejection = StatusCode;

    fn from_request_parts(_parts: &RequestParts, _state: &OtherState) -> Result<Self, Self::Rejection> {
        Ok(Self)
    }
}

struct Api;

#[router(state = AppState)]
impl Api {
    #[route(GET, "/")]
    async fn home(&self, extractor: OtherStateExtractor) -> StatusCode {
        let _ = extractor;
        StatusCode::NO_CONTENT
    }

    #[catch(StatusCode, from = OtherStateExtractor)]
    async fn catch(&self, rejection: StatusCode) -> StatusCode {
        rejection
    }
}

fn main() {}
