// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{Context, Poll};

use tower_service::Service as TowerService;

use crate::Service;

#[derive(Clone, Debug)]
pub(crate) struct MockService {
    poll_ready_response: Poll<Result<(), String>>,
    call_response: Result<String, String>,
}

impl MockService {
    #[must_use]
    pub fn new(poll_ready_response: Poll<Result<(), String>>, call_response: Result<String, String>) -> Self {
        Self {
            poll_ready_response,
            call_response,
        }
    }
}

impl TowerService<String> for MockService {
    type Response = String;
    type Error = String;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_ready_response.clone()
    }

    fn call(&mut self, _req: String) -> Self::Future {
        let response = self.call_response.clone();
        Box::pin(async move { response })
    }
}

impl Service<String> for MockService {
    type Out = Result<String, String>;

    async fn execute(&self, _input: String) -> Self::Out {
        self.call_response.clone()
    }
}
