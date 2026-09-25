// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "integration tests")]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

#[cfg(all(target_os = "linux", zygote_workspace_tests))]
use std::path::{Path, PathBuf};
#[cfg(all(target_os = "linux", zygote_workspace_tests))]
use std::process::Command as NativeCommand;
#[cfg(all(target_os = "linux", zygote_workspace_tests))]
use std::sync::OnceLock;
#[cfg(all(target_os = "linux", zygote_workspace_tests))]
use std::time::{Duration, Instant};

use prost::Message;
#[cfg(all(target_os = "linux", zygote_workspace_tests))]
use zygote_control::Zygote;
use zygote_rt::protocol::packet::Body;
#[cfg(all(target_os = "linux", zygote_workspace_tests))]
use zygote_rt::protocol::test_support::SPECIALIZATION_STALL_ENV;
use zygote_rt::protocol::test_support::{fixed_decoder_corpus, generated_decoder_cases};
use zygote_rt::protocol::{
    DescriptorRole, ErrorMessage, Exited, FEATURE_BASE, FEATURE_LINUX_SANDBOX, Launch, LinuxSandbox, MAX_ITEM_COUNT, MAX_PACKET_LEN,
    Packet, PoolControl, PoolState, PrivilegeReduction, ProtocolError, Ready, Rlimit, SeccompInstruction, SeccompPolicy, Shutdown, Started,
    SupplementaryGroups, UnixOptions, decode, encode, packet,
};

fn packets() -> Vec<Packet> {
    vec![
        packet(0, Body::Ready(Ready { nonce: vec![b'a'; 32] }), Vec::new()),
        packet(
            7,
            Body::Launch(Launch {
                argv: vec![b"target".to_vec(), vec![0xff]],
                environment: vec![b"A=B".to_vec()],
                cwd: Some(b"/work".to_vec()),
                unix: Some(UnixOptions {
                    uid: Some(1000),
                    gid: Some(1000),
                    groups: Some(SupplementaryGroups { gids: vec![10, 20] }),
                    process_group: Some(0),
                    new_session: false,
                    umask: Some(0o027),
                }),
                rlimits: vec![Rlimit {
                    resource: 7,
                    soft: 128,
                    hard: 256,
                }],
                sandbox: None,
            }),
            vec![DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr],
        ),
        packet(
            7,
            Body::Started(Started {
                pid: 42,
                prefork_hit: false,
                idle_workers: 0,
                prefork_refills: 0,
                prefork_refill_nanoseconds: 0,
            }),
            vec![DescriptorRole::Pidfd],
        ),
        packet(7, Body::Exited(Exited { wait_status: 9 }), Vec::new()),
        packet(
            7,
            Body::Error(ErrorMessage {
                stage: 4,
                error_number: 11,
            }),
            Vec::new(),
        ),
        packet(
            0,
            Body::Error(ErrorMessage {
                stage: 102,
                error_number: 16,
            }),
            Vec::new(),
        ),
        packet(0, Body::Shutdown(Shutdown {}), Vec::new()),
        packet(0, Body::PoolControl(PoolControl { idle_workers: 2 }), Vec::new()),
        packet(
            0,
            Body::PoolState(PoolState {
                idle_workers: 2,
                prefork_refills: 3,
                prefork_refill_nanoseconds: 4,
            }),
            Vec::new(),
        ),
    ]
}

fn sandbox_launch() -> Packet {
    packet(
        9,
        Body::Launch(Launch {
            argv: vec![b"target".to_vec()],
            environment: Vec::new(),
            cwd: None,
            unix: None,
            rlimits: Vec::new(),
            sandbox: Some(LinuxSandbox {
                privileges: Some(PrivilegeReduction {
                    no_new_privs: true,
                    disable_dumping: true,
                    clear_capabilities: true,
                    drop_capability_bounding_set: true,
                    lock_securebits: true,
                }),
                seccomp: Some(SeccompPolicy {
                    audit_architecture: 0xc000_003e,
                    instructions: vec![
                        SeccompInstruction {
                            code: 0x20,
                            jump_true: 0,
                            jump_false: 0,
                            value: 4,
                        },
                        SeccompInstruction {
                            code: 0x15,
                            jump_true: 1,
                            jump_false: 0,
                            value: 0xc000_003e,
                        },
                        SeccompInstruction {
                            code: 0x06,
                            jump_true: 0,
                            jump_false: 0,
                            value: 0x8000_0000,
                        },
                        SeccompInstruction {
                            code: 0x06,
                            jump_true: 0,
                            jump_false: 0,
                            value: 0x7fff_0000,
                        },
                    ],
                }),
                landlock: true,
                cgroup: true,
                namespaces: vec![1, 4, 6],
                landlock_abi: 1,
                landlock_handled_access: 1,
            }),
        }),
        vec![
            DescriptorRole::Stdin,
            DescriptorRole::Stdout,
            DescriptorRole::Stderr,
            DescriptorRole::CgroupProcs,
            DescriptorRole::LandlockRuleset,
            DescriptorRole::NamespaceCgroup,
            DescriptorRole::NamespaceNetwork,
            DescriptorRole::NamespaceMount,
        ],
    )
}

fn launch_body_mut(packet: &mut Packet) -> &mut Launch {
    let Some(Body::Launch(launch)) = &mut packet.body else {
        unreachable!()
    };
    launch
}

#[test]
fn prost_packets_round_trip_canonically() {
    for expected in packets() {
        let bytes = encode(&expected).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, expected);
        assert_eq!(encode(&decoded).unwrap(), bytes);
        assert_eq!(Packet::decode(bytes.as_slice()).unwrap(), expected);
    }
}

#[test]
fn malformed_and_security_ambiguous_packets_are_rejected() {
    let valid = encode(&packets()[1]).unwrap();

    let mut duplicate_version = vec![8, 1];
    duplicate_version.extend_from_slice(&valid);
    assert_eq!(decode(&duplicate_version), Err(ProtocolError::DuplicateField(1)));

    let mut unknown_feature = packets()[1].clone();
    unknown_feature.required_features.push(FEATURE_LINUX_SANDBOX + 1);
    assert_eq!(
        decode(&unknown_feature.encode_to_vec()),
        Err(ProtocolError::UnknownRequiredFeature(FEATURE_LINUX_SANDBOX + 1))
    );

    let mut incompatible = packets()[1].clone();
    let launch = launch_body_mut(&mut incompatible);
    let unix = launch.unix.as_mut().unwrap();
    unix.new_session = true;
    unix.process_group = Some(0);
    assert_eq!(decode(&incompatible.encode_to_vec()), Err(ProtocolError::IncompatibleOptions));

    let mut invalid_limit = packets()[1].clone();
    if let Some(Body::Launch(launch)) = &mut invalid_limit.body {
        launch.rlimits[0].resource = u32::MAX;
    }

    assert_eq!(decode(&invalid_limit.encode_to_vec()), Err(ProtocolError::InvalidResourceLimit));

    if let Some(Body::Launch(launch)) = &mut invalid_limit.body {
        launch.rlimits[0] = Rlimit {
            resource: 7,
            soft: 257,
            hard: 256,
        };
    }
    assert_eq!(decode(&invalid_limit.encode_to_vec()), Err(ProtocolError::InvalidResourceLimit));

    if let Some(Body::Launch(launch)) = &mut invalid_limit.body {
        launch.rlimits[0].soft = 128;
        launch.rlimits.push(launch.rlimits[0]);
    }
    assert_eq!(decode(&invalid_limit.encode_to_vec()), Err(ProtocolError::DuplicateResourceLimit));
}

#[test]
fn fixed_decoder_corpus_has_explicit_verdicts() {
    for case in fixed_decoder_corpus() {
        assert_eq!(decode(&case.bytes).is_ok(), case.accepted, "{}", case.name);
    }
}

#[test]
fn generated_decoder_cases_match_the_prost_oracle() {
    for case in generated_decoder_cases() {
        assert_eq!(decode(&case.bytes).is_ok(), case.accepted, "{}", case.name);
    }
}

#[cfg(all(target_os = "linux", zygote_workspace_tests))]
fn timeout_fixture() -> &'static Path {
    static TARGET: OnceLock<PathBuf> = OnceLock::new();
    TARGET.get_or_init(|| {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let fixture = manifest.join("test-fixtures/targets/Cargo.toml");
        let target_dir = manifest.join("../../target/zygote-protocol-fixtures");
        let status = NativeCommand::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args(["build", "--quiet", "--bin", "zygote-transparent-fixture", "--manifest-path"])
            .arg(fixture)
            .arg("--target-dir")
            .arg(&target_dir)
            .status()
            .expect("fixture build command must start");
        assert!(status.success(), "specialization timeout fixture failed to build");
        target_dir.join("debug/zygote-transparent-fixture")
    })
}

#[cfg(all(target_os = "linux", zygote_workspace_tests))]
#[test]
#[cfg_attr(miri, ignore)]
fn specialization_timeout_reaps_child_and_releases_shutdown_lock() {
    let artifact_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/zygote-test-artifacts");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    let pid_path = artifact_dir.join(format!("specialization-stall-{}", std::process::id()));
    let _ = std::fs::remove_file(&pid_path);

    let zygote = Zygote::builder(timeout_fixture()).spawn().unwrap();
    let launcher = zygote.launcher();
    let (launch_sender, launch_receiver) = std::sync::mpsc::channel();
    let launch_handle = {
        let launcher = launcher.clone();
        let pid_path = pid_path.clone();
        std::thread::spawn(move || {
            let result = launcher
                .command()
                .env(SPECIALIZATION_STALL_ENV, &pid_path)
                .spawn()
                .map(|child| child.id())
                .map_err(|error| (error.kind(), error.to_string()));
            launch_sender.send(result).unwrap();
        })
    };

    let marker_deadline = Instant::now() + Duration::from_secs(2);
    while !pid_path.exists() {
        let remaining = marker_deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "specialization stall marker was not created");
        std::thread::sleep(remaining.min(Duration::from_millis(5)));
    }

    let child_pid = std::fs::read_to_string(&pid_path).unwrap();
    let child_pid = child_pid.parse::<u32>().unwrap();

    let (shutdown_started_sender, shutdown_started_receiver) = std::sync::mpsc::channel();
    let (shutdown_sender, shutdown_receiver) = std::sync::mpsc::channel();
    let shutdown_handle = std::thread::spawn(move || {
        shutdown_started_sender.send(()).unwrap();
        shutdown_sender.send(zygote.shutdown()).unwrap();
    });
    shutdown_started_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("shutdown thread did not start");
    assert!(matches!(
        shutdown_receiver.recv_timeout(Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));

    let launch_error = launch_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("launch did not report the specialization timeout")
        .unwrap_err();
    assert_eq!(launch_error.0, std::io::ErrorKind::TimedOut);
    assert!(launch_error.1.contains("stage 7"));
    launch_handle.join().unwrap();

    shutdown_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("shutdown did not complete after the specialization timeout")
        .unwrap();
    shutdown_handle.join().unwrap();
    assert!(!Path::new(&format!("/proc/{child_pid}")).exists(), "timed-out child was not reaped");
    assert_eq!(launcher.command().spawn().unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
    std::fs::remove_file(pid_path).unwrap();
}

#[test]
fn prost_decoder_rejects_nul_in_arguments_and_environment() {
    for (argv, environment) in [
        (vec![b"target".to_vec(), b"bad\0argument".to_vec()], vec![]),
        (vec![b"target".to_vec()], vec![b"NAME=bad\0value".to_vec()]),
        (vec![b"target".to_vec()], vec![b"NA\0ME=value".to_vec()]),
    ] {
        let malformed = packet(
            7,
            Body::Launch(Launch {
                argv,
                environment,
                cwd: None,
                unix: None,
                rlimits: Vec::new(),
                sandbox: None,
            }),
            vec![DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr],
        );
        assert!(matches!(
            decode(&malformed.encode_to_vec()),
            Err(ProtocolError::EmbeddedNul | ProtocolError::InvalidEnvironment)
        ));
    }
}

#[test]
fn descriptor_roles_must_match_each_packet_exactly() {
    let mut started = packets()[2].clone();
    started.descriptor_roles.clear();
    assert_eq!(decode(&started.encode_to_vec()), Err(ProtocolError::InvalidDescriptorRoles));

    let mut launch = packets()[1].clone();
    launch.descriptor_roles.swap(0, 1);
    assert_eq!(decode(&launch.encode_to_vec()), Err(ProtocolError::InvalidDescriptorRoles));
}

#[test]
fn repeated_collection_boundaries_are_enforced() {
    let mut launch = Launch {
        argv: vec![b"x".to_vec(); MAX_ITEM_COUNT],
        environment: Vec::new(),
        cwd: None,
        unix: None,
        rlimits: Vec::new(),
        sandbox: None,
    };
    let boundary = packet(
        1,
        Body::Launch(launch.clone()),
        vec![DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr],
    );
    decode(&boundary.encode_to_vec()).unwrap();

    launch.argv.push(b"x".to_vec());
    let over = packet(
        1,
        Body::Launch(launch),
        vec![DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr],
    );
    assert_eq!(decode(&over.encode_to_vec()), Err(ProtocolError::InvalidItemCount));
}

#[test]
fn serialized_packet_byte_boundary_is_enforced() {
    let mut launch = packets()[1].clone();
    let body = launch_body_mut(&mut launch);
    body.argv = vec![vec![b'a'; MAX_PACKET_LEN / 2]];
    body.environment.clear();
    let middle_len = launch.encoded_len();
    launch_body_mut(&mut launch).argv[0].resize(MAX_PACKET_LEN / 2 + MAX_PACKET_LEN - middle_len, b'a');
    let at_limit = launch.encode_to_vec();
    assert_eq!(at_limit.len(), MAX_PACKET_LEN);
    assert_eq!(encode(&launch).unwrap(), at_limit);
    assert_eq!(decode(&at_limit).unwrap(), launch);

    launch_body_mut(&mut launch).argv[0].push(b'a');
    let over_limit = launch.encode_to_vec();
    assert_eq!(over_limit.len(), MAX_PACKET_LEN + 1);
    assert_eq!(encode(&launch), Err(ProtocolError::PacketTooLarge));
    assert_eq!(decode(&over_limit), Err(ProtocolError::PacketTooLarge));
}

#[test]
fn sandbox_features_and_descriptor_roles_are_fail_closed() {
    let valid = sandbox_launch();
    assert_eq!(valid.required_features, [FEATURE_BASE, FEATURE_LINUX_SANDBOX]);
    decode(&valid.encode_to_vec()).unwrap();

    let mut missing_feature = valid.clone();
    missing_feature.required_features.pop();
    assert_eq!(decode(&missing_feature.encode_to_vec()), Err(ProtocolError::MissingRequiredFeature));

    let mut wrong_role = valid.clone();
    wrong_role.descriptor_roles.pop();
    assert_eq!(decode(&wrong_role.encode_to_vec()), Err(ProtocolError::InvalidDescriptorRoles));

    let mut user_notification = valid;
    let launch = launch_body_mut(&mut user_notification);
    launch.sandbox.as_mut().unwrap().seccomp.as_mut().unwrap().instructions[3].value = 0x7fc0_0000;
    assert_eq!(decode(&user_notification.encode_to_vec()), Err(ProtocolError::InvalidSeccomp));

    let mut invalid_jump = sandbox_launch();
    let launch = launch_body_mut(&mut invalid_jump);
    launch.sandbox.as_mut().unwrap().seccomp.as_mut().unwrap().instructions[1].jump_true = u32::MAX;
    assert_eq!(decode(&invalid_jump.encode_to_vec()), Err(ProtocolError::InvalidSeccomp));
}
