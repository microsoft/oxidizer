// Copyright (c) Microsoft Corporation.

/// Marker trait for a pipeline of layers and a root service that builds into a final service.
///
/// This trait serves as a marker for a pipeline of layers and a root service
/// that, upon building, produces a final composed service. It is designed for
/// constructing layered services where multiple components can be composed
/// together.
///
/// The trait is automatically implemented for tuples containing layers and a root service,
/// supporting compositions of up to 16 elements. This enables natural nesting patterns
/// where layers are stacked on top of the actual service.
///
/// This trait is sealed and cannot be implemented for types outside this crate.
pub trait ServiceBuilder {
    /// The type of service produced by this builder.
    type Service;

    /// Builds and returns the final service.
    ///
    /// This method consumes the builder and produces the configured service
    /// with all layers and components properly composed.
    fn build(self) -> Self::Service;
}
