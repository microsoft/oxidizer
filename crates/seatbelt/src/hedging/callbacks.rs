// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;
use std::time::Duration;

use super::args::{HedgingDelayArgs, OnHedgeArgs, RecoveryArgs, TryCloneArgs};
use crate::RecoveryInfo;

crate::utils::define_fn_wrapper!(TryClone<In>(Fn(&mut In, TryCloneArgs) -> Option<In>));
crate::utils::define_fn_wrapper!(ShouldRecover<Out>(Fn(&Out, RecoveryArgs) -> RecoveryInfo));

// Defined manually because the macro requires generic type parameters.

#[derive(Clone)]
pub(crate) struct OnHedge(Arc<dyn Fn(OnHedgeArgs) + Send + Sync>);

impl OnHedge {
    pub(crate) fn new(f: impl Fn(OnHedgeArgs) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    pub(crate) fn call(&self, args: OnHedgeArgs) {
        (self.0)(args);
    }
}

impl std::fmt::Debug for OnHedge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnHedge").finish()
    }
}

#[derive(Clone)]
pub(crate) struct DelayFn(Arc<dyn Fn(HedgingDelayArgs) -> Duration + Send + Sync>);

impl DelayFn {
    pub(crate) fn new(f: impl Fn(HedgingDelayArgs) -> Duration + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    pub(crate) fn call(&self, args: HedgingDelayArgs) -> Duration {
        (self.0)(args)
    }
}

impl std::fmt::Debug for DelayFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DelayFn").finish()
    }
}
