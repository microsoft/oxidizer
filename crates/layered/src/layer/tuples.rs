// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use tower_layer::Layer;

use super::Stack;

impl<L1, S> Stack for (L1, S)
where
    L1: Layer<S>,
{
    type Service = L1::Service;

    fn build(self) -> Self::Service {
        let (l1, service) = self;

        l1.layer(service)
    }
}

impl<L1, L2, S> Stack for (L1, L2, S)
where
    L1: Layer<L2::Service>,
    L2: Layer<S>,
{
    type Service = L1::Service;

    fn build(self) -> Self::Service {
        let (l1, l2, service) = self;

        (l1, l2).layer(service)
    }
}

impl<L1, L2, L3, S> Stack for (L1, L2, L3, S)
where
    L1: Layer<L2::Service>,
    L2: Layer<L3::Service>,
    L3: Layer<S>,
{
    type Service = L1::Service;

    fn build(self) -> Self::Service {
        let (l1, l2, l3, service) = self;

        (l1, l2, l3).layer(service)
    }
}

impl<L1, L2, L3, L4, S> Stack for (L1, L2, L3, L4, S)
where
    L1: Layer<L2::Service>,
    L2: Layer<L3::Service>,
    L3: Layer<L4::Service>,
    L4: Layer<S>,
{
    type Service = L1::Service;

    fn build(self) -> Self::Service {
        let (l1, l2, l3, l4, service) = self;

        (l1, l2, l3, l4).layer(service)
    }
}

impl<L1, L2, L3, L4, L5, S> Stack for (L1, L2, L3, L4, L5, S)
where
    L1: Layer<L2::Service>,
    L2: Layer<L3::Service>,
    L3: Layer<L4::Service>,
    L4: Layer<L5::Service>,
    L5: Layer<S>,
{
    type Service = L1::Service;

    fn build(self) -> Self::Service {
        let (l1, l2, l3, l4, l5, service) = self;

        (l1, l2, l3, l4, l5).layer(service)
    }
}

impl<L1, L2, L3, L4, L5, L6, S> Stack for (L1, L2, L3, L4, L5, L6, S)
where
    L1: Layer<L2::Service>,
    L2: Layer<L3::Service>,
    L3: Layer<L4::Service>,
    L4: Layer<L5::Service>,
    L5: Layer<L6::Service>,
    L6: Layer<S>,
{
    type Service = L1::Service;

    fn build(self) -> Self::Service {
        let (l1, l2, l3, l4, l5, l6, service) = self;

        (l1, l2, l3, l4, l5, l6).layer(service)
    }
}

impl<L1, L2, L3, L4, L5, L6, L7, S> Stack for (L1, L2, L3, L4, L5, L6, L7, S)
where
    L1: Layer<L2::Service>,
    L2: Layer<L3::Service>,
    L3: Layer<L4::Service>,
    L4: Layer<L5::Service>,
    L5: Layer<L6::Service>,
    L6: Layer<L7::Service>,
    L7: Layer<S>,
{
    type Service = L1::Service;

    fn build(self) -> Self::Service {
        let (l1, l2, l3, l4, l5, l6, l7, service) = self;

        (l1, l2, l3, l4, l5, l6, l7).layer(service)
    }
}

impl<L1, L2, L3, L4, L5, L6, L7, L8, S> Stack for (L1, L2, L3, L4, L5, L6, L7, L8, S)
where
    L1: Layer<L2::Service>,
    L2: Layer<L3::Service>,
    L3: Layer<L4::Service>,
    L4: Layer<L5::Service>,
    L5: Layer<L6::Service>,
    L6: Layer<L7::Service>,
    L7: Layer<L8::Service>,
    L8: Layer<S>,
{
    type Service = L1::Service;

    fn build(self) -> Self::Service {
        let (l1, l2, l3, l4, l5, l6, l7, l8, service) = self;

        (l1, l2, l3, l4, l5, l6, l7, l8).layer(service)
    }
}

impl<L1, L2, L3, L4, L5, L6, L7, L8, L9, S> Stack for (L1, L2, L3, L4, L5, L6, L7, L8, L9, S)
where
    L1: Layer<L2::Service>,
    L2: Layer<L3::Service>,
    L3: Layer<L4::Service>,
    L4: Layer<L5::Service>,
    L5: Layer<L6::Service>,
    L6: Layer<L7::Service>,
    L7: Layer<L8::Service>,
    L8: Layer<L9::Service>,
    L9: Layer<S>,
{
    type Service = L1::Service;

    fn build(self) -> Self::Service {
        let (l1, l2, l3, l4, l5, l6, l7, l8, l9, service) = self;

        (l1, l2, l3, l4, l5, l6, l7, l8, l9).layer(service)
    }
}

impl<L1, L2, L3, L4, L5, L6, L7, L8, L9, L10, S> Stack for (L1, L2, L3, L4, L5, L6, L7, L8, L9, L10, S)
where
    L1: Layer<L2::Service>,
    L2: Layer<L3::Service>,
    L3: Layer<L4::Service>,
    L4: Layer<L5::Service>,
    L5: Layer<L6::Service>,
    L6: Layer<L7::Service>,
    L7: Layer<L8::Service>,
    L8: Layer<L9::Service>,
    L9: Layer<L10::Service>,
    L10: Layer<S>,
{
    type Service = L1::Service;

    fn build(self) -> Self::Service {
        let (l1, l2, l3, l4, l5, l6, l7, l8, l9, l10, service) = self;

        (l1, l2, l3, l4, l5, l6, l7, l8, l9, l10).layer(service)
    }
}

impl<L1, L2, L3, L4, L5, L6, L7, L8, L9, L10, L11, S> Stack for (L1, L2, L3, L4, L5, L6, L7, L8, L9, L10, L11, S)
where
    L1: Layer<L2::Service>,
    L2: Layer<L3::Service>,
    L3: Layer<L4::Service>,
    L4: Layer<L5::Service>,
    L5: Layer<L6::Service>,
    L6: Layer<L7::Service>,
    L7: Layer<L8::Service>,
    L8: Layer<L9::Service>,
    L9: Layer<L10::Service>,
    L10: Layer<L11::Service>,
    L11: Layer<S>,
{
    type Service = L1::Service;

    fn build(self) -> Self::Service {
        let (l1, l2, l3, l4, l5, l6, l7, l8, l9, l10, l11, service) = self;

        (l1, l2, l3, l4, l5, l6, l7, l8, l9, l10, l11).layer(service)
    }
}

impl<L1, L2, L3, L4, L5, L6, L7, L8, L9, L10, L11, L12, S> Stack for (L1, L2, L3, L4, L5, L6, L7, L8, L9, L10, L11, L12, S)
where
    L1: Layer<L2::Service>,
    L2: Layer<L3::Service>,
    L3: Layer<L4::Service>,
    L4: Layer<L5::Service>,
    L5: Layer<L6::Service>,
    L6: Layer<L7::Service>,
    L7: Layer<L8::Service>,
    L8: Layer<L9::Service>,
    L9: Layer<L10::Service>,
    L10: Layer<L11::Service>,
    L11: Layer<L12::Service>,
    L12: Layer<S>,
{
    type Service = L1::Service;

    fn build(self) -> Self::Service {
        let (l1, l2, l3, l4, l5, l6, l7, l8, l9, l10, l11, l12, service) = self;

        (l1, l2, l3, l4, l5, l6, l7, l8, l9, l10, l11, l12).layer(service)
    }
}

impl<L1, L2, L3, L4, L5, L6, L7, L8, L9, L10, L11, L12, L13, S> Stack for (L1, L2, L3, L4, L5, L6, L7, L8, L9, L10, L11, L12, L13, S)
where
    L1: Layer<L2::Service>,
    L2: Layer<L3::Service>,
    L3: Layer<L4::Service>,
    L4: Layer<L5::Service>,
    L5: Layer<L6::Service>,
    L6: Layer<L7::Service>,
    L7: Layer<L8::Service>,
    L8: Layer<L9::Service>,
    L9: Layer<L10::Service>,
    L10: Layer<L11::Service>,
    L11: Layer<L12::Service>,
    L12: Layer<L13::Service>,
    L13: Layer<S>,
{
    type Service = L1::Service;
    fn build(self) -> Self::Service {
        let (l1, l2, l3, l4, l5, l6, l7, l8, l9, l10, l11, l12, l13, service) = self;

        (l1, l2, l3, l4, l5, l6, l7, l8, l9, l10, l11, l12, l13).layer(service)
    }
}

impl<L1, L2, L3, L4, L5, L6, L7, L8, L9, L10, L11, L12, L13, L14, S> Stack
    for (L1, L2, L3, L4, L5, L6, L7, L8, L9, L10, L11, L12, L13, L14, S)
where
    L1: Layer<L2::Service>,
    L2: Layer<L3::Service>,
    L3: Layer<L4::Service>,
    L4: Layer<L5::Service>,
    L5: Layer<L6::Service>,
    L6: Layer<L7::Service>,
    L7: Layer<L8::Service>,
    L8: Layer<L9::Service>,
    L9: Layer<L10::Service>,
    L10: Layer<L11::Service>,
    L11: Layer<L12::Service>,
    L12: Layer<L13::Service>,
    L13: Layer<L14::Service>,
    L14: Layer<S>,
{
    type Service = L1::Service;

    fn build(self) -> Self::Service {
        let (l1, l2, l3, l4, l5, l6, l7, l8, l9, l10, l11, l12, l13, l14, service) = self;

        (l1, l2, l3, l4, l5, l6, l7, l8, l9, l10, l11, l12, l13, l14).layer(service)
    }
}

impl<L1, L2, L3, L4, L5, L6, L7, L8, L9, L10, L11, L12, L13, L14, L15, S> Stack
    for (L1, L2, L3, L4, L5, L6, L7, L8, L9, L10, L11, L12, L13, L14, L15, S)
where
    L1: Layer<L2::Service>,
    L2: Layer<L3::Service>,
    L3: Layer<L4::Service>,
    L4: Layer<L5::Service>,
    L5: Layer<L6::Service>,
    L6: Layer<L7::Service>,
    L7: Layer<L8::Service>,
    L8: Layer<L9::Service>,
    L9: Layer<L10::Service>,
    L10: Layer<L11::Service>,
    L11: Layer<L12::Service>,
    L12: Layer<L13::Service>,
    L13: Layer<L14::Service>,
    L14: Layer<L15::Service>,
    L15: Layer<S>,
{
    type Service = L1::Service;

    fn build(self) -> Self::Service {
        let (l1, l2, l3, l4, l5, l6, l7, l8, l9, l10, l11, l12, l13, l14, l15, service) = self;

        (l1, l2, l3, l4, l5, l6, l7, l8, l9, l10, l11, l12, l13, l14, l15).layer(service)
    }
}

impl<L1, L2, L3, L4, L5, L6, L7, L8, L9, L10, L11, L12, L13, L14, L15, L16, S> Stack
    for (L1, L2, L3, L4, L5, L6, L7, L8, L9, L10, L11, L12, L13, L14, L15, L16, S)
where
    L1: Layer<L2::Service>,
    L2: Layer<L3::Service>,
    L3: Layer<L4::Service>,
    L4: Layer<L5::Service>,
    L5: Layer<L6::Service>,
    L6: Layer<L7::Service>,
    L7: Layer<L8::Service>,
    L8: Layer<L9::Service>,
    L9: Layer<L10::Service>,
    L10: Layer<L11::Service>,
    L11: Layer<L12::Service>,
    L12: Layer<L13::Service>,
    L13: Layer<L14::Service>,
    L14: Layer<L15::Service>,
    L15: Layer<L16::Service>,
    L16: Layer<S>,
{
    type Service = L1::Service;

    fn build(self) -> Self::Service {
        let (l1, l2, l3, l4, l5, l6, l7, l8, l9, l10, l11, l12, l13, l14, l15, l16, service) = self;

        (l1, l2, l3, l4, l5, l6, l7, l8, l9, l10, l11, l12, l13, l14, l15, l16).layer(service)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    // Integration tests have been moved to tests/stack.rs
    // No internal tests needed for this module
}
