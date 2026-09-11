// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt::Debug;

use layered::{DynamicService, DynamicServiceExt};
use performables::arc::Arc;
use thread_aware::ThreadAware;

use crate::handlers::Dispatch;
use crate::pipeline::pipeline_context::PipelineContext;
use crate::{HttpRequest, HttpResponse, RequestHandler};

/// Callable that constructs a custom request pipeline.
type PipelineConstructor = dyn Fn(Dispatch, PipelineContext) -> DynamicService<HttpRequest, crate::Result<HttpResponse>> + Send + Sync;

/// Type-erased callable that assembles a custom request pipeline.
#[derive(Clone, ThreadAware)]
pub(crate) struct CustomPipeline(#[thread_aware(skip)] Arc<PipelineConstructor>);

impl CustomPipeline {
    /// Erases a concrete request-handler constructor into a reusable pipeline callable.
    pub(crate) fn new<T: RequestHandler + 'static>(make_handler: impl Fn(Dispatch, PipelineContext) -> T + Send + Sync + 'static) -> Self {
        let constructor: Box<PipelineConstructor> = Box::new(move |dispatch, context| make_handler(dispatch, context).into_dynamic());
        Self(constructor.into())
    }

    /// Creates the custom dynamic service for one dispatch handler and context.
    pub(crate) fn create(&self, handler: Dispatch, context: PipelineContext) -> DynamicService<HttpRequest, crate::Result<HttpResponse>> {
        self.0(handler, context)
    }
}

impl Debug for CustomPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>()).finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn debug_custom_pipeline() {
        let pipeline = CustomPipeline::new(|r, _| r);
        let debug_str = format!("{pipeline:?}");
        assert_eq!(debug_str, "fetch::pipeline::custom::CustomPipeline");
    }
}
