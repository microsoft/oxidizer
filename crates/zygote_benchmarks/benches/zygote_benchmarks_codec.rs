// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Static controller protocol costs; no target process is started.

#![allow(
    missing_docs,
    clippy::missing_docs_in_private_items,
    clippy::unwrap_used,
    reason = "benchmark-only harness"
)]

use std::hint::black_box;
use std::sync::OnceLock;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput};
#[cfg(target_os = "linux")]
use prost::Message as _;
#[cfg(target_os = "linux")]
use zygote_rt::NativeLaunchDecoder;
use zygote_rt::protocol::packet::Body;
use zygote_rt::protocol::{self, DescriptorRole, Launch, Packet, Started};

#[metabench::benchmark(CONTROLLER_DECODE, "zygote_benchmarks_codec/controller_protocol", "decode/typical")]
fn representative_decode() -> usize {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    let bytes = BYTES.get_or_init(|| protocol::encode(&packet(16, 128)).unwrap());
    black_box(protocol::decode(black_box(bytes)).unwrap()).descriptor_roles.len()
}

fn packet(environment_count: usize, argument_bytes: usize) -> Packet {
    let environment = (0..environment_count)
        .map(|index| format!("BENCH_{index:04}=value").into_bytes())
        .collect();
    Packet {
        protocol_major: protocol::PROTOCOL_MAJOR,
        request_id: 1,
        required_features: vec![protocol::FEATURE_BASE],
        descriptor_roles: [DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr]
            .map(i32::from)
            .to_vec(),
        body: Some(Body::Launch(Launch {
            argv: vec![b"benchmark".to_vec(), vec![b'a'; argument_bytes]],
            environment,
            cwd: None,
            unix: None,
            rlimits: vec![],
            sandbox: None,
        })),
    }
}

fn criterion_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("zygote_benchmarks_codec/controller_protocol");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));
    for (name, entries, bytes) in [
        ("empty", 0, 0),
        ("typical", 16, 128),
        ("entries_256", 256, 128),
        ("entries_1024", 1_024, 128),
        ("entries_4096", 4_096, 128),
        ("near_packet_limit", 16, 900_000),
    ] {
        let input = packet(entries, bytes);
        let encoded = protocol::encode(&input).unwrap();
        assert_eq!(protocol::decode(&encoded).unwrap(), input);
        eprintln!(
            "codec corpus={name} valid_wire_bytes={} environment_entries={entries}",
            encoded.len()
        );
        group.throughput(Throughput::Bytes(encoded.len().try_into().unwrap()));
        group.bench_function(BenchmarkId::new("encode", name), |b| {
            b.iter(|| black_box(protocol::encode(black_box(&input)).unwrap()));
        });
        group.bench_function(BenchmarkId::new("decode", name), |b| {
            if name == "typical" {
                b.iter(|| black_box(representative_decode()));
            } else {
                b.iter(|| black_box(protocol::decode(black_box(&encoded)).unwrap()));
            }
        });
        #[cfg(target_os = "linux")]
        {
            let mut decoder = NativeLaunchDecoder::new();
            let observation = decoder.decode(&encoded).unwrap();
            eprintln!(
                "codec corpus={name} native_arena_bytes={} native_arena_allocations={} descriptor_roles={}",
                observation.arena_bytes,
                observation.allocations,
                input.descriptor_roles.len()
            );
            group.bench_function(BenchmarkId::new("native_decode", name), |b| {
                b.iter(|| black_box(decoder.decode(black_box(&encoded)).unwrap()));
            });
        }
    }
    let started = protocol::encode(&protocol::packet(
        1,
        Body::Started(Started {
            pid: 42,
            prefork_hit: true,
            idle_workers: 4,
            prefork_refills: 8,
            prefork_refill_nanoseconds: 1_000,
        }),
        vec![DescriptorRole::Pidfd],
    ))
    .unwrap();
    group.throughput(Throughput::Bytes(started.len().try_into().unwrap()));
    group.bench_function("decode_event/started", |b| {
        b.iter(|| black_box(protocol::decode(black_box(&started)).unwrap()));
    });
    #[cfg(target_os = "linux")]
    for (name, duplicate_index) in [("early_duplicate", 1), ("late_duplicate", 4_095)] {
        let mut duplicate = packet(4_096, 128);
        let Body::Launch(launch) = duplicate.body.as_mut().unwrap() else {
            unreachable!()
        };
        let (first, remaining) = launch.environment.split_at_mut(1);
        remaining[duplicate_index - 1].clone_from(&first[0]);
        let encoded = duplicate.encode_to_vec();
        let mut decoder = NativeLaunchDecoder::new();
        assert!(decoder.decode(&encoded).is_none());
        group.throughput(Throughput::Bytes(encoded.len().try_into().unwrap()));
        group.bench_function(BenchmarkId::new("native_reject", name), |b| {
            b.iter(|| assert!(black_box(decoder.decode(black_box(&encoded))).is_none()));
        });
    }
    let tiny = protocol::encode(&packet(0, 0)).unwrap();
    let large = protocol::encode(&packet(16, 900_000)).unwrap();
    group.throughput(Throughput::Bytes((tiny.len() + large.len()).try_into().unwrap()));
    group.bench_function("alternating_decode/tiny_near_limit", |b| {
        b.iter(|| {
            black_box(protocol::decode(black_box(&tiny)).unwrap());
            black_box(protocol::decode(black_box(&large)).unwrap());
        });
    });
    #[cfg(target_os = "linux")]
    {
        let mut decoder = NativeLaunchDecoder::new();
        group.bench_function("alternating_native_decode/tiny_near_limit", |b| {
            b.iter(|| {
                black_box(decoder.decode(black_box(&tiny)).unwrap());
                black_box(decoder.decode(black_box(&large)).unwrap());
            });
        });
    }
    group.finish();
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [CONTROLLER_DECODE],);
