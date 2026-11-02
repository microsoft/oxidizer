// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Houses simple inversion of control for registering log and metric destinations
//! with the OpenTelemetry logger and meter provider builders.

use opentelemetry_sdk::logs::LoggerProviderBuilder;
use opentelemetry_sdk::metrics::MeterProviderBuilder;

use crate::{DestinationResult, IntoLogDestination, IntoMetricDestination, LogDestination, MetricDestination};

/// Convenience trait to register log destinations with the OpenTelemetry logger
/// provider builder.
///
/// # Examples
///
/// ```
/// use observed_destination_core::LoggerProviderBuilderExt;
/// use opentelemetry_sdk::logs::SdkLoggerProvider;
/// # use observed_destination_core::{DestinationResult, LogDestination};
/// # use opentelemetry_sdk::logs::LoggerProviderBuilder;
/// #
/// # #[derive(Debug)]
/// # struct MyLogDestination;
/// #
/// # impl LogDestination for MyLogDestination {
/// #     fn register(self, builder: LoggerProviderBuilder) -> DestinationResult<LoggerProviderBuilder> {
/// #         Ok(builder)
/// #     }
/// # }
///
/// # fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
/// let provider = SdkLoggerProvider::builder()
///     .register(MyLogDestination)?
///     .build();
/// # Ok(())
/// # }
/// ```
pub trait LoggerProviderBuilderExt {
    /// Registers a log destination on this logger provider builder.
    ///
    /// # Errors
    ///
    /// Returns an error if the destination cannot be registered with the builder.
    /// Specific errors depend on the destination implementation and may include
    /// configuration issues, conflicts, or other conditions.
    fn register(self, dest: impl IntoLogDestination) -> DestinationResult<LoggerProviderBuilder>;
}

impl LoggerProviderBuilderExt for LoggerProviderBuilder {
    fn register(self, dest: impl IntoLogDestination) -> DestinationResult<LoggerProviderBuilder> {
        dest.into_log_destination().register(self)
    }
}

/// Convenience trait to register metric destinations with the OpenTelemetry meter
/// provider builder.
///
/// # Examples
///
/// ```
/// use observed_destination_core::MeterProviderBuilderExt;
/// use opentelemetry_sdk::metrics::SdkMeterProvider;
/// # use observed_destination_core::{DestinationResult, MetricDestination};
/// # use opentelemetry_sdk::metrics::MeterProviderBuilder;
/// #
/// # #[derive(Debug)]
/// # struct MyMetricDestination;
/// #
/// # impl MetricDestination for MyMetricDestination {
/// #     fn register(self, builder: MeterProviderBuilder) -> DestinationResult<MeterProviderBuilder> {
/// #         Ok(builder)
/// #     }
/// # }
///
/// # fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
/// let provider = SdkMeterProvider::builder()
///     .register(MyMetricDestination)?
///     .build();
/// # Ok(())
/// # }
/// ```
pub trait MeterProviderBuilderExt {
    /// Registers a metric destination on this meter provider builder.
    ///
    /// # Errors
    ///
    /// Returns an error if the destination cannot be registered with the builder.
    /// Specific errors depend on the destination implementation and may include
    /// configuration issues, conflicts, or other conditions.
    fn register(self, dest: impl IntoMetricDestination) -> DestinationResult<MeterProviderBuilder>;
}

impl MeterProviderBuilderExt for MeterProviderBuilder {
    fn register(self, dest: impl IntoMetricDestination) -> DestinationResult<MeterProviderBuilder> {
        dest.into_metric_destination().register(self)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error::Error;
    use std::fmt::{Debug, Display, Formatter};
    use std::sync::{Arc, Mutex};

    use crate::{LogDestination, LoggerProviderBuilderExt, MeterProviderBuilderExt, MetricDestination};

    #[derive(Debug, Clone)]
    struct TestDestination {
        should_succeed: bool,
        registered_log: Arc<Mutex<bool>>,
        registered_metric: Arc<Mutex<bool>>,
    }

    impl TestDestination {
        fn new(should_succeed: bool) -> Self {
            Self {
                should_succeed,
                registered_log: Arc::new(Mutex::new(false)),
                registered_metric: Arc::new(Mutex::new(false)),
            }
        }
    }

    #[derive(Debug)]
    struct RegistrationError {
        id: u32,
    }

    impl Display for RegistrationError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            write!(f, "RegistrationError, id: {}", self.id)
        }
    }

    impl Error for RegistrationError {}

    impl LogDestination for TestDestination {
        fn register(
            self,
            builder: opentelemetry_sdk::logs::LoggerProviderBuilder,
        ) -> crate::DestinationResult<opentelemetry_sdk::logs::LoggerProviderBuilder> {
            if self.should_succeed {
                *self.registered_log.lock().unwrap() = true;
                Ok(builder)
            } else {
                Err(RegistrationError { id: 1 }.into())
            }
        }
    }

    impl MetricDestination for TestDestination {
        fn register(
            self,
            builder: opentelemetry_sdk::metrics::MeterProviderBuilder,
        ) -> crate::DestinationResult<opentelemetry_sdk::metrics::MeterProviderBuilder> {
            if self.should_succeed {
                *self.registered_metric.lock().unwrap() = true;
                Ok(builder)
            } else {
                Err(RegistrationError { id: 1 }.into())
            }
        }
    }

    #[test]
    fn logger_provider_builder_ext_registers_destination() {
        let test_destination = TestDestination::new(true);

        let result = opentelemetry_sdk::logs::SdkLoggerProvider::builder().register(test_destination.clone());

        assert!(result.is_ok(), "Expected successful registration, but got an error");
        assert!(
            *test_destination.registered_log.lock().unwrap(),
            "Log destination was not registered"
        );
    }

    #[test]
    fn logger_provider_builder_ext_propagates_error() {
        let result = opentelemetry_sdk::logs::SdkLoggerProvider::builder().register(TestDestination::new(false));

        assert!(result.is_err(), "Expected registration to fail, but it succeeded");

        let registration_error = result.unwrap_err().downcast::<RegistrationError>();
        assert!(registration_error.is_ok(), "Unexpected error type");

        let registration_error = registration_error.unwrap();
        assert_eq!(registration_error.id, 1, "Error details not preserved");
        assert_eq!(registration_error.to_string(), "RegistrationError, id: 1");
    }

    #[test]
    fn meter_provider_builder_ext_registers_destination() {
        let test_destination = TestDestination::new(true);

        let result = opentelemetry_sdk::metrics::SdkMeterProvider::builder().register(test_destination.clone());

        assert!(result.is_ok(), "Expected successful registration, but got an error");
        assert!(
            *test_destination.registered_metric.lock().unwrap(),
            "Metric destination was not registered"
        );
    }

    #[test]
    fn meter_provider_builder_ext_propagates_error() {
        let result = opentelemetry_sdk::metrics::SdkMeterProvider::builder().register(TestDestination::new(false));

        assert!(result.is_err(), "Expected registration to fail, but it succeeded");

        let registration_error = result.unwrap_err().downcast::<RegistrationError>();
        assert!(registration_error.is_ok(), "Unexpected error type");

        let registration_error = registration_error.unwrap();
        assert_eq!(registration_error.id, 1, "Error details not preserved");
        assert_eq!(registration_error.to_string(), "RegistrationError, id: 1");
    }
}
