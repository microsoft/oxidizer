// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::rc::Rc;

use isolated_domains::Domain;

use crate::{DispatchStop, PlacementToken, YieldFuture};

/// Types and functions used for interaction with runtime. See the provided methods for more information.
#[derive(Debug, Clone)]
pub struct RuntimeOperations {
    // Stop won't be called a lot, so dyn is completely fine here. We're using it to erase the
    // RA type parameter on DispatcherClient.
    dispatch: Rc<dyn DispatchStop>,
    // Index of associated runtime worker
    domain: Option<Domain>,
}

impl RuntimeOperations {
    pub(crate) fn new(dispatch: Rc<dyn DispatchStop>, domain: Option<Domain>) -> Self {
        Self { dispatch, domain }
    }

    #[doc = include_str!("../../doc/snippets/fn_runtime_placement.md")]
    pub fn placement(&self) -> Option<PlacementToken> {
        self.domain.map(PlacementToken::new)
    }

    #[doc = include_str!("../../doc/snippets/fn_runtime_stop.md")]
    pub fn stop(&self) {
        self.dispatch.stop();
    }

    // TODO: this should be a regular function instead of being an associated function
    #[doc = include_str!("../../doc/snippets/fn_runtime_yield_now.md")]
    pub fn yield_now() -> impl Future<Output = ()> + use<> {
        YieldFuture::new()
    }
}