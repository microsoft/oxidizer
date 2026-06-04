// Copyright (c) Microsoft Corporation.

use data_privacy::RedactionEngine;
use futures::future::Either;
use http_extensions::HttpBodyBuilder;
use http_extensions::routing::Router;
use layered::{DynamicService, DynamicServiceExt, Service, Stack};
use opentelemetry::metrics::Meter;
use thread_aware::ThreadAware;
use tick::Clock;

use crate::handlers::Dispatch;
use crate::pipeline::StandardRequestPipeline;
use crate::pipeline::custom::CustomPipelineFactory;
use crate::pipeline::pipeline_context::PipelineContext;
use crate::pipeline::standard::{ConfigureStandardPipeline, RecoveryMode};
use crate::resilience::HttpResilienceContext;
use crate::{HttpRequest, HttpResponse};

#[derive(Debug, Clone, ThreadAware)]
pub(crate) enum PipelineBuilder {
    StandardPipeline(ConfigureStandardPipeline),
    Minimal,
    Custom(CustomPipelineFactory),
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
    pub fn dbg_string_for_custom_pipeline(&self) -> &str {
        match self {
            Self::Minimal(_) => panic!("must be custom pipeline"),
            Self::Custom { debug, .. } => debug,
        }
    }

    pub fn is_standard(&self) -> bool {
        match self {
            Self::Minimal(_) => false,
            Self::Custom { standard_pipeline, .. } => *standard_pipeline,
        }
    }
}

impl Service<HttpRequest> for Pipeline {
    type Out = crate::Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = crate::Result<HttpResponse>> + Send {
        match &self {
            Self::Minimal(handler) => Either::Left(handler.execute(input)),
            Self::Custom { pipeline, .. } => Either::Right(pipeline.execute(input)),
        }
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

    #[expect(
        clippy::too_many_arguments,
        reason = "all parameters are required dependencies for assembling the pipeline"
    )]
    pub(crate) fn build(
        self,
        dispatch_handler: Dispatch,
        resilience_context: HttpResilienceContext,
        redaction_engine: RedactionEngine,
        meter: &Meter,
        body_builder: HttpBodyBuilder,
        clock: Clock,
        router: Router,
    ) -> Pipeline {
        match self {
            Self::StandardPipeline(configure) => {
                let context = PipelineContext::new(resilience_context, meter, redaction_engine.clone(), body_builder, clock, router);
                let standard = configure.create(context, &redaction_engine);

                match standard.recovery_mode {
                    RecoveryMode::Retry => {
                        let service = (
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
                let pipeline = factory.create(
                    dispatch_handler,
                    PipelineContext::new(resilience_context, meter, redaction_engine, body_builder, clock, router),
                );

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
mod tests {
    use std::time::Duration;

    use http::StatusCode;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::SdkMeterProvider;

    use super::*;

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn build_minimal_ok() {
        let clock = Clock::new_frozen();
        let dispatch = Dispatch::new_fake(StatusCode::OK);
        let pipeline = PipelineBuilder::Minimal.build(
            dispatch,
            HttpResilienceContext::new(&clock),
            RedactionEngine::default(),
            &test_meter(),
            HttpBodyBuilder::new_fake(),
            clock,
            Router::default(),
        );

        assert!(matches!(pipeline, Pipeline::Minimal(_)));
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn build_custom_ok() {
        let clock = Clock::new_frozen();
        let dispatch = Dispatch::new_fake(StatusCode::OK);
        let factory = CustomPipelineFactory::new(|dispatch, _| dispatch);
        let pipeline = PipelineBuilder::Custom(factory).build(
            dispatch,
            HttpResilienceContext::new(&clock),
            RedactionEngine::default(),
            &test_meter(),
            HttpBodyBuilder::new_fake(),
            clock,
            Router::default(),
        );

        assert!(!pipeline.is_standard());
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn build_standard_ok() {
        let clock = Clock::new_frozen();
        let dispatch = Dispatch::new_fake(StatusCode::OK);
        let pipeline = PipelineBuilder::StandardPipeline(ConfigureStandardPipeline::default()).build(
            dispatch,
            HttpResilienceContext::new(&clock),
            RedactionEngine::default(),
            &test_meter(),
            HttpBodyBuilder::new_fake(),
            clock,
            Router::default(),
        );

        let _dbg = pipeline.dbg_string_for_custom_pipeline();
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn pipeline_builder_default_ok() {
        let clock = Clock::new_frozen();
        let dispatch = Dispatch::new_fake(StatusCode::OK);
        let pipeline = PipelineBuilder::default().build(
            dispatch,
            HttpResilienceContext::new(&clock),
            RedactionEngine::default(),
            &test_meter(),
            HttpBodyBuilder::new_fake(),
            clock,
            Router::default(),
        );

        assert!(pipeline.is_standard());
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn configure_standard() {
        let clock = Clock::new_frozen();
        let dispatch = Dispatch::new_fake(StatusCode::OK);
        let pipeline = PipelineBuilder::Minimal
            .configure_standard(|p, _context| p.retry(|retry| retry.max_retry_attempts(10)))
            .build(
                dispatch,
                HttpResilienceContext::new(&clock),
                RedactionEngine::default(),
                &test_meter(),
                HttpBodyBuilder::new_fake(),
                clock,
                Router::default(),
            );

        assert!(format!("{pipeline:?}").contains("max_attempts: 11"));
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn configure_standard_twice() {
        let clock = Clock::new_frozen();
        let dispatch = Dispatch::new_fake(StatusCode::OK);
        let pipeline = PipelineBuilder::Minimal
            .configure_standard(|p, _context| p.retry(|retry| retry.max_retry_attempts(10)))
            .configure_standard(|p, _context| p.attempt_timeout(|timeout| timeout.timeout(Duration::from_secs(123))))
            .build(
                dispatch,
                HttpResilienceContext::new(&clock),
                RedactionEngine::default(),
                &test_meter(),
                HttpBodyBuilder::new_fake(),
                clock,
                Router::default(),
            );

        let debug = format!("{pipeline:?}");
        assert!(debug.contains("max_attempts: 11"));
        assert!(debug.contains("timeout: 123s"));
    }

    fn test_meter() -> Meter {
        SdkMeterProvider::default().meter("test")
    }
}
