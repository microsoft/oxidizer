// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Descriptor decoding and generated code/OpenAPI emission, measured separately.
//! `protox` compilation and output-directory preparation are outside the measurement.

#![allow(missing_docs, reason = "benchmark code has no public API")]
#![allow(clippy::unwrap_used, clippy::panic, reason = "invalid benchmark fixtures must fail the run")]
#![allow(clippy::needless_pass_by_value, reason = "Gungraun owns prepared inputs")]
#![expect(
    clippy::exit,
    clippy::missing_docs_in_private_items,
    unused_qualifications,
    reason = "metabench emits Gungraun entry points"
)]

use std::hint::black_box;
use std::path::PathBuf;
use std::sync::OnceLock;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};
use rest_over_grpc::build::{DescriptorOptions, Generator, OpenApiInfo, ServiceDefinition};

const DESCRIPTORS: &str = "rog_generation/descriptors";
const GENERATE: &str = "rog_generation/generate";

fn descriptor_bytes(services: usize) -> Vec<u8> {
    static FIXTURES: OnceLock<[Vec<u8>; 3]> = OnceLock::new();
    let fixtures = FIXTURES.get_or_init(|| [compile_descriptor(1), compile_descriptor(4), compile_descriptor(16)]);
    fixtures[match services {
        1 => 0,
        4 => 1,
        16 => 2,
        _ => panic!("unknown descriptor size"),
    }]
    .clone()
}

fn compile_descriptor(services: usize) -> Vec<u8> {
    let source = include_str!("../proto/library.proto");
    let start = source.find("service Library {").unwrap();
    let end = source.find("// A shelf in the library.").unwrap();
    let service = &source[start..end];
    let mut generated = source[..end].to_owned();
    for i in 1..services {
        let route_version = i + 1;
        generated.push_str(
            &service
                .replace("service Library {", &format!("service Library{i} {{"))
                .replace("rpc ", &format!("rpc S{i}"))
                .replace("/v1/", &format!("/v{route_version}/")),
        );
    }
    generated.push_str(&source[end..]);
    let proto = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("proto");
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/metabench/rog_generation/fixtures");
    std::fs::create_dir_all(&fixtures).unwrap();
    let filename = format!("library_{services}.proto");
    std::fs::write(fixtures.join(&filename), generated).unwrap();
    let mut compiler = protox::Compiler::new([fixtures, proto]).unwrap();
    compiler.include_imports(true);
    compiler.include_source_info(true);
    compiler.open_file(filename).unwrap();
    compiler.encode_file_descriptor_set()
}

fn definitions(services: usize) -> Vec<ServiceDefinition> {
    ServiceDefinition::from_fds(descriptor_bytes(services), &DescriptorOptions::new().package(".library")).unwrap()
}

#[metabench::benchmark(FROM_FDS, DESCRIPTORS, "from_fds", gungraun_setup = verified_bytes)]
#[bench::one(descriptor_bytes(1))]
#[bench::several(descriptor_bytes(4))]
#[bench::many(descriptor_bytes(16))]
fn from_fds(bytes: Vec<u8>) -> Vec<ServiceDefinition> {
    black_box(ServiceDefinition::from_fds(black_box(&bytes), &DescriptorOptions::new().package(".library")).unwrap())
}

struct WriteInput {
    generator: Generator,
    out: PathBuf,
}

fn write_input(services: usize, openapi: bool) -> WriteInput {
    let case = format!("{services}_{}", if openapi { "openapi" } else { "code" });
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/metabench/rog_generation/generated")
        .join(case);
    std::fs::create_dir_all(&out).unwrap();
    let info = openapi.then(|| OpenApiInfo::new("Library", "v1"));
    let mut generator = Generator::builder().emit_tonic_bridge(false).emit_openapi_spec(info).build();
    generator.add_all(definitions(services));
    WriteInput { generator, out }
}

#[metabench::benchmark(WRITE, GENERATE, "write", gungraun_setup = verified_write)]
#[bench::one_code(write_input(1, false))]
#[bench::several_code(write_input(4, false))]
#[bench::many_code(write_input(16, false))]
#[bench::one_openapi(write_input(1, true))]
#[bench::several_openapi(write_input(4, true))]
#[bench::many_openapi(write_input(16, true))]
fn write(input: WriteInput) -> WriteInput {
    input.generator.write(black_box(&input.out)).unwrap();
    black_box(input)
}

fn verified_bytes(bytes: Vec<u8>) -> Vec<u8> {
    verify();
    bytes
}

fn verified_write(input: WriteInput) -> WriteInput {
    verify();
    input
}

fn verify() {
    for size in [1, 4, 16] {
        let bytes = descriptor_bytes(size);
        let services = from_fds(bytes.clone());
        assert_eq!(services.len(), size);
        for openapi in [false, true] {
            let input = write_input(size, openapi);
            input.generator.write(&input.out).unwrap();
            let code = std::fs::read(input.out.join("library.rest.rs")).unwrap();
            let transcoder = std::fs::read(input.out.join("transcoder.rest.rs")).unwrap();
            assert!(!code.is_empty() && !transcoder.is_empty());
            if openapi {
                let doc = std::fs::read(input.out.join("library.openapi.json")).unwrap();
                let json: serde_json::Value = serde_json::from_slice(&doc).unwrap();
                assert_eq!(json["openapi"], "3.1.0");
                assert_eq!(json["paths"]["/v1/shelves"]["get"]["operationId"], "ListShelves");
                assert!(!doc.is_empty());
                input.generator.write(&input.out).unwrap();
                assert_eq!(std::fs::read(input.out.join("library.openapi.json")).unwrap(), doc);
            }
        }
    }
}

fn criterion_benchmarks(c: &mut Criterion) {
    verify();
    let mut descriptors = c.benchmark_group(FROM_FDS.group_name());
    for (case, size) in [("one", 1), ("several", 4), ("many", 16)] {
        descriptors.throughput(Throughput::Elements(size as u64));
        descriptors.bench_function(BenchmarkId::new(FROM_FDS.benchmark_name(), case), |b| {
            b.iter_batched(|| descriptor_bytes(size), from_fds, BatchSize::SmallInput);
        });
    }
    descriptors.finish();
    let mut output = c.benchmark_group(WRITE.group_name());
    for (case, size) in [("one", 1), ("several", 4), ("many", 16)] {
        for (suffix, openapi) in [("code", false), ("openapi", true)] {
            output.throughput(Throughput::Elements(size as u64));
            output.bench_function(BenchmarkId::new(WRITE.benchmark_name(), format!("{case}_{suffix}")), |b| {
                b.iter_batched(|| write_input(size, openapi), write, BatchSize::SmallInput);
            });
        }
    }
    output.finish();
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [FROM_FDS, WRITE]);
