// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public wire and error survivor oracles.

use std::error::Error as _;
use std::io::{self, Cursor, Write};
use std::path::PathBuf;

use seismograph_protocol::message::{
    EventBufferDisposition, EventClassCounts, RecorderActivity, RecorderStatistics, RecordingConfiguration, RecordingPolicy, Request,
    Response, SnapshotOptions, ThreadRecorderStatistics,
};
use seismograph_protocol::monitor::{InstanceId, MonitorOwner};
use seismograph_protocol::{Error, read_control_response, read_request, read_response, write_request, write_response};

fn frame(kind: u16, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![b'S', b'G', b'M', b'P', 9, 0];
    bytes.extend_from_slice(&kind.to_le_bytes());
    bytes.extend_from_slice(&0x0807_0605_0403_0201_u64.to_le_bytes());
    bytes.extend_from_slice(
        &u32::try_from(payload.len())
            .expect("test frame callers supply bounded payloads that fit the protocol's u32 length")
            .to_le_bytes(),
    );
    bytes.extend_from_slice(payload);
    bytes
}

#[test]
fn gamma_survivor_error_sources_and_registry_context() {
    let path = PathBuf::from(r"C:\registry\worker.monitor");
    let registry = Error::Registry {
        path: path.clone(),
        source: io::Error::new(io::ErrorKind::PermissionDenied, "refused"),
    };
    assert_eq!(
        registry.to_string(),
        format!("Seismograph monitor registry {}: refused", path.display())
    );
    assert_eq!(registry.source().unwrap().to_string(), "refused");
    let transport = Error::Io(io::Error::new(io::ErrorKind::BrokenPipe, "peer closed"));
    assert_eq!(transport.to_string(), "peer closed");
    assert_eq!(transport.source().unwrap().to_string(), "peer closed");
    for (error, text) in [
        (Error::InvalidMessage, "invalid Seismograph monitor message"),
        (Error::InvalidDescriptor, "invalid Seismograph monitor descriptor"),
        (Error::UnsupportedVersion, "unsupported Seismograph monitor protocol version"),
        (Error::MessageTooLarge, "Seismograph monitor message exceeds its size limit"),
        (Error::MissingRuntimeDirectory, "no per-user runtime directory is available"),
    ] {
        assert_eq!(error.to_string(), text);
        assert!(error.source().is_none());
    }
}

#[test]
fn gamma_survivor_request_control_limit_is_inclusive() {
    for (len, oversized) in [(65_535, false), (65_536, false), (65_537, true)] {
        let bytes = frame(1, &vec![0; len]);
        let mut reader = Cursor::new(bytes);
        let error = read_request(&mut reader).unwrap_err();
        if oversized {
            assert!(matches!(error, Error::MessageTooLarge));
            assert_eq!(reader.position(), 20);
        } else {
            assert!(matches!(error, Error::InvalidMessage));
            assert_eq!(reader.position(), 20 + u64::try_from(len).unwrap());
        }
    }
}

#[test]
fn gamma_survivor_write_errors_at_each_frame_field_are_returned() {
    struct FailingWriter {
        remaining: usize,
    }
    impl Write for FailingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "field failed"));
            }
            let count = buf.len().min(self.remaining);
            self.remaining -= count;
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "flush failed"))
        }
    }
    for boundary in [0, 4, 6, 8, 16, 20] {
        let error = write_response(&mut FailingWriter { remaining: boundary }, 17, &Response::Snapshot(vec![1, 2, 3])).unwrap_err();
        assert!(matches!(error, Error::Io(ref source) if source.kind() == io::ErrorKind::BrokenPipe));
        assert_eq!(error.to_string(), "field failed");
    }
    let error = write_request(&mut FailingWriter { remaining: 20 }, 17, &Request::Hello).unwrap_err();
    assert!(matches!(error, Error::Io(ref source) if source.kind() == io::ErrorKind::PermissionDenied));
    assert_eq!(error.to_string(), "flush failed");
}

#[test]
fn gamma_survivor_recording_wire_fields_and_capacity_boundaries() {
    let policies = [
        RecordingPolicy {
            enabled: true,
            capture_backtraces: false,
            sampling_one_in: 2,
        },
        RecordingPolicy {
            enabled: false,
            capture_backtraces: true,
            sampling_one_in: 4,
        },
        RecordingPolicy {
            enabled: true,
            capture_backtraces: true,
            sampling_one_in: 8,
        },
        RecordingPolicy {
            enabled: false,
            capture_backtraces: false,
            sampling_one_in: 16,
        },
        RecordingPolicy {
            enabled: true,
            capture_backtraces: false,
            sampling_one_in: 32,
        },
        RecordingPolicy {
            enabled: false,
            capture_backtraces: true,
            sampling_one_in: 64,
        },
    ];
    let configuration = RecordingConfiguration {
        allocations: policies[0],
        general_events: policies[1],
        arc_dereferences: policies[2],
        runtime_tasks: policies[3],
        io: policies[4],
        cache: policies[5],
        ..RecordingConfiguration::default()
    };
    let expected = [
        0, 0, 1, 0, 1, 0, 2, 0, 0, 0, 0, 1, 4, 0, 0, 0, 1, 1, 8, 0, 0, 0, 0, 0, 16, 0, 0, 0, 1, 0, 32, 0, 0, 0, 0, 1, 64, 0, 0, 0,
    ];
    let mut encoded = Vec::new();
    write_request(&mut encoded, 0x0807_0605_0403_0201, &Request::SetRecording(configuration)).unwrap();
    assert_eq!(encoded, frame(2, &expected));
    assert_eq!(
        read_request(&mut encoded.as_slice()).unwrap().1,
        Request::SetRecording(configuration)
    );
    for capacity in [64_u32, 1_048_576] {
        let mut payload = expected;
        payload[..4].copy_from_slice(&capacity.to_le_bytes());
        let decoded = read_request(&mut frame(2, &payload).as_slice()).unwrap().1;
        assert_eq!(
            decoded,
            Request::SetRecording(RecordingConfiguration {
                event_capacity_per_thread: capacity,
                ..configuration
            })
        );
    }
    for capacity in [0_u32, 63, 65, 1_048_575, 1_048_577] {
        let mut payload = expected;
        payload[..4].copy_from_slice(&capacity.to_le_bytes());
        assert!(matches!(
            read_request(&mut frame(2, &payload).as_slice()),
            Err(Error::InvalidMessage)
        ));
    }
    let invalid = frame(101, &[0; 56]);
    assert!(matches!(read_response(&mut invalid.as_slice()), Err(Error::InvalidMessage)));
    for len in [0, 15, 16, 55, 57] {
        assert!(matches!(
            read_response(&mut frame(101, &vec![0; len]).as_slice()),
            Err(Error::InvalidMessage)
        ));
    }
}

#[test]
fn gamma_survivor_all_request_tags_and_payload_rejection() {
    let requests = [
        (1, Request::Hello),
        (4, Request::ReadRecorderStatistics),
        (6, Request::ReadCacheRecording),
        (7, Request::ReadServerVersion),
        (8, Request::CaptureSnapshotAndStop),
        (9, Request::ClearEventBuffers),
        (10, Request::ReadRecorderActivity),
    ];
    for (kind, request) in requests {
        let mut bytes = Vec::new();
        write_request(&mut bytes, 0x0807_0605_0403_0201, &request).unwrap();
        assert_eq!(bytes, frame(kind, &[]));
        assert_eq!(read_request(&mut bytes.as_slice()).unwrap().1, request);
        assert!(matches!(
            read_request(&mut frame(kind, &[0]).as_slice()),
            Err(Error::InvalidMessage)
        ));
    }
    for (tag, disposition) in [
        (0, EventBufferDisposition::Retain),
        (1, EventBufferDisposition::Clear),
        (2, EventBufferDisposition::Release),
    ] {
        let request = Request::CaptureSnapshot(SnapshotOptions {
            event_buffers: disposition,
        });
        assert_eq!(read_request(&mut frame(3, &[tag]).as_slice()).unwrap().1, request);
    }
    for payload in [&[][..], &[3], &[0, 0]] {
        assert!(matches!(
            read_request(&mut frame(3, payload).as_slice()),
            Err(Error::InvalidMessage)
        ));
    }
}

#[test]
fn gamma_survivor_strings_are_bounded_and_malformed_errors_are_returned() {
    for response in [Response::Error("x".repeat(65_536)), Response::ServerVersion("v".repeat(65_536))] {
        assert!(matches!(write_response(&mut Vec::new(), 1, &response), Err(Error::MessageTooLarge)));
    }
    for payload in [&[][..], &[1], &[2, 0, b'x'], &[1, 0, 255], &[1, 0, b'x', 0]] {
        assert!(matches!(
            read_response(&mut frame(255, payload).as_slice()),
            Err(Error::InvalidDescriptor)
        ));
    }
    assert_eq!(
        read_response(&mut frame(255, &[2, 0, b'o', b'k']).as_slice()).unwrap().1,
        Response::Error("ok".into())
    );
}

#[test]
fn gamma_survivor_activity_wire_counters_are_independent_and_minimum_threads_are_exact() {
    let activity = RecorderActivity {
        statistics: RecorderStatistics {
            thread_count: 0x0403_0201,
            total_events: 0x0807_0605,
            retained_events: 0x0c0b_0a09,
            lost_events: 0x100f_0e0d,
            event_capacity_per_thread: 0x1413_1211,
            allocated_bytes: 0x1817_1615,
            ..RecorderStatistics::default()
        },
        session_id: 0x1c1b_1a19,
        class_events: EventClassCounts {
            allocations: 2,
            general_events: 3,
            arc_dereferences: 5,
            runtime_tasks: 7,
            io: 11,
            cache: 13,
        },
        threads: vec![ThreadRecorderStatistics {
            thread_id: 0x2827_2625_2423_2221,
            total_events: 17,
            retained_events: 19,
            lost_events: 23,
            event_capacity: 29,
            retired: true,
            name: String::new(),
        }],
    };
    let mut encoded = Vec::new();
    write_response(&mut encoded, 0x0807_0605_0403_0201, &Response::RecorderActivity(activity.clone())).unwrap();
    assert_eq!(&encoded[20..28], &[1, 2, 3, 4, 0, 0, 0, 0]);
    assert_eq!(&encoded[164..168], &[1, 0, 0, 0]);
    assert_eq!(&encoded[168..176], &[0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28]);
    assert_eq!(encoded.len(), 20 + 148 + 43);
    assert_eq!(
        read_response(&mut encoded.as_slice()).unwrap().1,
        Response::RecorderActivity(activity)
    );
    let payload = &encoded[20..];
    let mut truncated = payload[..payload.len() - 1].to_vec();
    assert!(matches!(
        read_response(&mut frame(107, &truncated).as_slice()),
        Err(Error::InvalidMessage)
    ));
    truncated[144..148].copy_from_slice(&0_u32.to_le_bytes());
    assert!(matches!(
        read_response(&mut frame(107, &truncated).as_slice()),
        Err(Error::InvalidDescriptor)
    ));
    let mut invalid = RecorderActivity::default();
    invalid.statistics.recording.event_capacity_per_thread = 65;
    assert!(matches!(
        write_response(&mut Vec::new(), 1, &Response::RecorderActivity(invalid)),
        Err(Error::InvalidMessage)
    ));
}

#[test]
fn gamma_survivor_activity_thread_field_errors_remain_recoverable() {
    let activity = RecorderActivity {
        threads: vec![ThreadRecorderStatistics::default(), ThreadRecorderStatistics::default()],
        ..RecorderActivity::default()
    };
    let mut bytes = Vec::new();
    write_response(&mut bytes, 1, &Response::RecorderActivity(activity)).unwrap();
    for consumed in [10_u16, 18, 26, 34, 42] {
        let mut payload = bytes[20..].to_vec();
        payload[148 + 41..148 + 43].copy_from_slice(&consumed.to_le_bytes());
        assert!(matches!(
            read_response(&mut frame(107, &payload).as_slice()),
            Err(Error::InvalidDescriptor)
        ));
    }
    let activity = RecorderActivity {
        threads: vec![ThreadRecorderStatistics {
            name: "x".repeat(65_536),
            ..ThreadRecorderStatistics::default()
        }],
        ..RecorderActivity::default()
    };
    assert!(matches!(
        write_response(&mut Vec::new(), 1, &Response::RecorderActivity(activity)),
        Err(Error::MessageTooLarge)
    ));
}

#[test]
fn gamma_survivor_snapshot_control_and_chunk_boundaries() {
    for length in [0, 8_192, 16_384, 65_536, 65_537] {
        let response = Response::Snapshot(vec![0xa5; length]);
        let mut bytes = Vec::new();
        write_response(&mut bytes, 0x0807_0605_0403_0201, &response).unwrap();
        assert_eq!(bytes, frame(103, &vec![0xa5; length]));
        assert_eq!(read_response(&mut bytes.as_slice()).unwrap().1, response);
        if length <= 65_536 {
            assert_eq!(read_control_response(&mut bytes.as_slice()).unwrap().1, response);
        } else {
            assert!(matches!(read_control_response(&mut bytes.as_slice()), Err(Error::MessageTooLarge)));
        }
    }
}

#[test]
fn gamma_survivor_owner_and_identity_canonical_boundaries() {
    let identity = InstanceId::from_bytes([
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10,
    ]);
    assert_eq!(identity.to_hex(), "0123456789abcdeffedcba9876543210");
    for value in ["S-1-", "S-1-5-", "S-1-5-01", "S-1-281474976710656-1", "4294967296", "01"] {
        assert!(matches!(value.parse::<MonitorOwner>(), Err(Error::InvalidDescriptor)), "{value}");
    }
    for value in ["0", "4294967295", "S-1-281474976710655-4294967295"] {
        assert_eq!(value.parse::<MonitorOwner>().unwrap().to_string(), value);
    }
}
