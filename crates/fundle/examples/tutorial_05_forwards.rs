#![expect(
    missing_debug_implementations,
    clippy::empty_structs_with_brackets,
    clippy::must_use_candidate,
    reason = "Unit tests"
)]

// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Assume these are random dependencies we want to inject.
pub struct ChatGpt {}

// And assume this comes from a 3rd party crate that wants to export
// a bundle that contains "a few interesting things". Assume most
// users probably only want `GpuBundle`, but some might want `Vulkan`
// or `Device`.
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

#[fundle::deps]
struct ChatGptDeps {
    _vulkan: gpu::Vulkan,
    _device: gpu::Device,
}

impl ChatGpt {
    fn new(_: impl Into<ChatGptDeps>) -> Self {
        Self {}
    }
}

#[fundle::bundle]
pub struct AppState {
    // Here `GpuBundle` is exported normally. In addition, we also
    // export `gpu::Instance`, `gpu::Device` and `gpu::Vulkan`.
    #[forward(gpu::Instance, gpu::Device, gpu::Vulkan)]
    gpu: gpu::GpuBundle,
    chat_gpt: ChatGpt,
}

fn main() {
    let _ = AppState::builder()
        .gpu(|_| gpu::GpuBundle::default())
        // Thanks to `forward` we can now inject `Vulkan` and `Device` into `ChatGpt`.
        .chat_gpt(|x| ChatGpt::new(x))
        .build();
}
