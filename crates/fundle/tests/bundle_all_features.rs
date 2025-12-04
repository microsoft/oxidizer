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
    pub struct Instance;
    #[derive(Clone, Default)]
    pub struct Device;
    #[derive(Clone, Default)]
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
fn file_compiles() {
    let _: AppState = AppState::builder()
        .logger1(|_| Logger::default())
        .logger2(|_| Logger::default())
        .config(|_| Config::default())
        .telemetry(|x| {
            let app_state = AppState!(select(x) => Logger(logger1));
            let _: &Logger = app_state.as_ref();
            let _: &Config = app_state.as_ref();
            Telemetry::default()
        })
        .gpu(|_| gpu::GpuBundle::default())
        .build();
}
