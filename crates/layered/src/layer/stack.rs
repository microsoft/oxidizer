// Copyright (c) Microsoft Corporation.

/// Builds a service from a tuple of layers and a root service.
///
/// Automatically implemented for tuples of layers with a service at the end,
/// supporting up to 16 elements. Layers apply outer to inner.
pub trait Stack {
    /// The type of service produced by this builder.
    type Service;

    /// Builds the composed service with all layers applied.
    fn build(self) -> Self::Service;
}
