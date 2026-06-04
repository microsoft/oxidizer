// Copyright (c) Microsoft Corporation.

use std::any::type_name;
use std::fmt::Debug;
use std::sync::Arc;

use layered::{DynamicService, DynamicServiceExt};
use thread_aware::ThreadAware;

use crate::handlers::Dispatch;
use crate::pipeline::pipeline_context::PipelineContext;
use crate::{HttpRequest, HttpResponse, RequestHandler};

/// A convenience API for creating a custom request pipeline.
#[derive(Clone, ThreadAware)]
pub(crate) struct CustomPipelineFactory(
    #[thread_aware(skip)] Arc<dyn Fn(Dispatch, PipelineContext) -> DynamicService<HttpRequest, crate::Result<HttpResponse>> + Send + Sync>,
);

impl CustomPipelineFactory {
    pub fn new<T: RequestHandler + 'static>(factory: impl Fn(Dispatch, PipelineContext) -> T + Send + Sync + 'static) -> Self {
        Self(Arc::new(move |dispatch, context| factory(dispatch, context).into_dynamic()))
    }

    pub fn create(&self, handler: Dispatch, context: PipelineContext) -> DynamicService<HttpRequest, crate::Result<HttpResponse>> {
        self.0(handler, context)
    }
}

impl Debug for CustomPipelineFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_custom_pipeline_factory() {
        let factory = CustomPipelineFactory::new(|r, _| r);
        let debug_str = format!("{factory:?}");
        assert_eq!(debug_str, "fetch::pipeline::custom::CustomPipelineFactory");
    }
}
