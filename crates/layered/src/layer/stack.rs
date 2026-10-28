// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Builds a service from a tuple of layers and one root service.
///
/// The last tuple element is the root service. The earlier elements are layers,
/// from outermost to innermost. A tuple can contain up to 16 layers.
pub trait Stack {
    /// The service type that the stack produces.
    type Service;

    /// Applies all layers and returns the composed service.
    fn into_service(self) -> Self::Service;
}
