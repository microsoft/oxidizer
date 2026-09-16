// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use futures::future::Either;
use layered::{DynamicService, DynamicServiceExt, Service, Stack};
use thread_aware::ThreadAware;

use crate::handlers::Dispatch;
use crate::pipeline::StandardRequestPipeline;
use crate::pipeline::custom::CustomPipeline;
use crate::pipeline::pipeline_context::PipelineContext;
use crate::pipeline::standard::{ConfigureStandardPipeline, RecoveryMode};
use crate::{HttpRequest, HttpResponse};

#[derive(Debug, Clone, ThreadAware)]
pub(crate) enum PipelineBuilder {
    StandardPipeline(ConfigureStandardPipeline),
    Minimal,
    Custom(CustomPipeline),
}

impl Default for PipelineBuilder {
    fn default() -> Self {
        Self::StandardPipeline(ConfigureStandardPipeline::default())
    }
}

#[derive(Debug)]
pub(crate) enum Pipeline {
    Minimal(Box<Dispatch>),
    Custom {
        #[cfg(test)]
        debug: String,
        #[cfg(test)]
        standard_pipeline: bool,
        pipeline: DynamicService<HttpRequest, crate::Result<HttpResponse>>,
    },
}

#[cfg(test)]
impl Pipeline {
    pub(crate) fn debug_string(&self) -> &str {
        match self {
            Self::Minimal(_) => panic!("expected custom pipeline, found minimal pipeline"),
            Self::Custom { debug, .. } => debug,
        }
    }

    pub(crate) fn is_standard(&self) -> bool {
        match self {
            Self::Minimal(_) => false,
            Self::Custom { standard_pipeline, .. } => *standard_pipeline,
        }
    }
}

impl Pipeline {
    pub(crate) fn execute(&self, input: HttpRequest) -> impl Future<Output = crate::Result<HttpResponse>> + Send {
        match &self {
            Self::Minimal(handler) => Either::Left(handler.execute(input)),
            Self::Custom { pipeline, .. } => Either::Right(pipeline.execute(input)),
        }
    }
}

impl Service<HttpRequest> for Pipeline {
    type Out = crate::Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = crate::Result<HttpResponse>> + Send {
        Self::execute(self, input)
    }
}

impl PipelineBuilder {
    pub(crate) fn configure_standard<F>(self, configure: F) -> Self
    where
        F: Fn(StandardRequestPipeline, PipelineContext) -> StandardRequestPipeline + Send + Sync + 'static,
    {
        match self {
            Self::StandardPipeline(pipeline) => Self::StandardPipeline(pipeline.combine(configure)),
            _ => Self::StandardPipeline(ConfigureStandardPipeline::new(configure)),
        }
    }

    pub(crate) fn build(self, dispatch_handler: Dispatch, context: PipelineContext) -> Pipeline {
        match self {
            Self::StandardPipeline(configure) => {
                let standard = configure.create(context);

                match standard.recovery_mode {
                    RecoveryMode::Retry => {
                        let service = (
                            standard.total_metrics,
                            standard.total_timeout,
                            standard.retry,
                            standard.breaker,
                            standard.attempt_timeout,
                            standard.attempt_intercept,
                            standard.attempt_logs,
                            standard.attempt_metrics,
                            dispatch_handler,
                        )
                            .into_service();

                        Pipeline::Custom {
                            #[cfg(test)]
                            debug: format!("{service:?}"),
                            #[cfg(test)]
                            standard_pipeline: true,
                            pipeline: service.into_dynamic(),
                        }
                    }
                    RecoveryMode::Hedging => {
                        let service = (
                            standard.total_metrics,
                            standard.total_timeout,
                            standard.hedging,
                            standard.breaker,
                            standard.attempt_timeout,
                            standard.attempt_intercept,
                            standard.attempt_logs,
                            standard.attempt_metrics,
                            dispatch_handler,
                        )
                            .into_service();

                        Pipeline::Custom {
                            #[cfg(test)]
                            debug: format!("{service:?}"),
                            #[cfg(test)]
                            standard_pipeline: true,
                            pipeline: service.into_dynamic(),
                        }
                    }
                }
            }
            Self::Minimal => Pipeline::Minimal(Box::new(dispatch_handler)),
            Self::Custom(factory) => {
                let pipeline = factory.create(dispatch_handler, context);

                Pipeline::Custom {
                    #[cfg(test)]
                    debug: format!("{pipeline:?}"),
                    #[cfg(test)]
                    standard_pipeline: false,
                    pipeline,
                }
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use data_privacy::RedactionEngine;
    use http::StatusCode;
    use http_extensions::HttpBodyBuilder;
    use http_extensions::routing::Router;
    use opentelemetry::metrics::{Meter, MeterProvider};
    use opentelemetry_sdk::metrics::SdkMeterProvider;
    use tick::Clock;

    use super::*;
    use crate::resilience::HttpResilienceContext;

    fn test_context() -> PipelineContext {
        let clock = Clock::new_frozen();
        PipelineContext::new(
            HttpResilienceContext::new(&clock),
            &test_meter(),
            RedactionEngine::default(),
            HttpBodyBuilder::new_fake(),
            clock,
            Router::default(),
        )
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn build_minimal_ok() {
        let dispatch = Dispatch::new_fake(StatusCode::OK);
        let pipeline = PipelineBuilder::Minimal.build(dispatch, test_context());

        assert!(matches!(pipeline, Pipeline::Minimal(_)));
        assert!(!pipeline.is_standard());
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn service_trait_forwards_to_pipeline_execution() {
        let pipeline = PipelineBuilder::Minimal.build(Dispatch::new_fake(StatusCode::OK), test_context());
        let request = http::Request::get("https://example.com")
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        let response = futures::executor::block_on(Service::execute(&pipeline, request)).unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    #[should_panic(expected = "expected custom pipeline, found minimal pipeline")]
    fn dbg_string_for_minimal_pipeline_panics() {
        let dispatch = Dispatch::new_fake(StatusCode::OK);
        let pipeline = PipelineBuilder::Minimal.build(dispatch, test_context());

        // The debug accessor is only valid for custom pipelines; a minimal pipeline must panic.
        let _ = pipeline.debug_string();
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn build_custom_ok() {
        let dispatch = Dispatch::new_fake(StatusCode::OK);
        let factory = CustomPipeline::new(|dispatch, _| dispatch);
        let pipeline = PipelineBuilder::Custom(factory).build(dispatch, test_context());

        assert!(!pipeline.is_standard());
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn build_standard_ok() {
        let dispatch = Dispatch::new_fake(StatusCode::OK);
        let pipeline = PipelineBuilder::StandardPipeline(ConfigureStandardPipeline::default()).build(dispatch, test_context());

        let _dbg = pipeline.debug_string();
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn pipeline_builder_default_ok() {
        let dispatch = Dispatch::new_fake(StatusCode::OK);
        let pipeline = PipelineBuilder::default().build(dispatch, test_context());

        assert!(pipeline.is_standard());
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn configure_standard() {
        let dispatch = Dispatch::new_fake(StatusCode::OK);
        let pipeline = PipelineBuilder::Minimal
            .configure_standard(|p, _context| p.retry(|retry| retry.max_retry_attempts(10)))
            .build(dispatch, test_context());

        assert!(format!("{pipeline:?}").contains("max_attempts: 11"));
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn configure_standard_twice() {
        let dispatch = Dispatch::new_fake(StatusCode::OK);
        let pipeline = PipelineBuilder::Minimal
            .configure_standard(|p, _context| p.retry(|retry| retry.max_retry_attempts(10)))
            .configure_standard(|p, _context| p.attempt_timeout(|timeout| timeout.timeout(Duration::from_secs(123))))
            .build(dispatch, test_context());

        let debug = format!("{pipeline:?}");
        assert!(debug.contains("max_attempts: 11"));
        assert!(debug.contains("timeout: 123s"));
    }

    fn test_meter() -> Meter {
        SdkMeterProvider::default().meter("test")
    }
}
