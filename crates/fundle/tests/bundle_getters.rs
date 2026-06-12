// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    unused_attributes,
    clippy::empty_structs_with_brackets,
    clippy::redundant_type_annotations,
    clippy::items_after_statements,
    missing_docs,
    reason = "Unit tests"
)]

#[derive(Debug, Default, Clone)]
pub struct Logger {}
#[derive(Debug, Default, Clone)]
pub struct Config {}
#[derive(Debug, Default, Clone)]
pub struct Telemetry {}

mod gpu {
    #[derive(Clone, Default)]
    #[expect(
        unreachable_pub,
        reason = "must remain pub: #[fundle::bundle] generates a public interface over this bundle component type (E0446 otherwise)"
    )]
    pub struct Instance;
    #[derive(Clone, Default)]
    #[expect(
        unreachable_pub,
        reason = "must remain pub: #[fundle::bundle] generates a public interface over this bundle component type (E0446 otherwise)"
    )]
    pub struct Device;
    #[derive(Clone, Default)]
    #[expect(
        unreachable_pub,
        reason = "must remain pub: #[fundle::bundle] generates a public interface over this bundle component type (E0446 otherwise)"
    )]
    pub struct Vulkan;

    #[fundle::bundle]
    #[derive(Default)]
    pub struct GpuBundle {
        instance: Instance,
        device: Device,
        vulkan: Vulkan,
    }
}

#[fundle::bundle]
struct AppState {
    logger1: Logger,
    logger2: Logger,
    config: Config,
    telemetry: Telemetry,
    #[forward(gpu::Instance, gpu::Device, gpu::Vulkan)]
    gpu: gpu::GpuBundle,
}

#[test]
fn f() {
    AppState::builder()
        .logger1(|_| Logger::default())
        .logger2(|_| Logger::default())
        .telemetry(|x| {
            let _ = x.logger1();
            let _ = x.logger2();
            Telemetry::default()
        });
}
