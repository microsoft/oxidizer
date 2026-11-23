// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generates messages, gRPC handlers, and JSON bindings for the browser monitor.

use std::path::PathBuf;

use rest_over_grpc::build::{DescriptorOptions, Generator, ServiceDefinition};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(std::env::var("OUT_DIR")?);
    let source = "src/commands/monitor/web/proto";
    println!("cargo:rerun-if-changed={source}");

    let mut compiler = protox::Compiler::new([source])?;
    compiler.include_imports(true);
    compiler.open_file("monitor.proto")?;
    let descriptors = compiler.encode_file_descriptor_set();

    tonic_prost_build::configure()
        .build_client(false)
        .build_transport(false)
        .compile_fds(compiler.file_descriptor_set())?;
    pbjson_build::Builder::new()
        .register_descriptors(&descriptors)?
        .build(&[".seismograph.monitor.v1"])?;
    Generator::builder()
        .build()
        .add_all(ServiceDefinition::from_fds(&descriptors, &DescriptorOptions::new())?)
        .write(&output)?;
    Ok(())
}
