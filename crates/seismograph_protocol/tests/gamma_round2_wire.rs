// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Independent, in-memory wire scalar oracles.

use seismograph_protocol::message::{RecordingConfiguration, RecordingPolicy, Request, Response};
use seismograph_protocol::monitor::InstanceId;
use seismograph_protocol::{read_request, read_response, write_request, write_response};

#[test]
#[cfg(feature = "test-util")]
fn gamma_round2_mock_publication_rejects_a_canonical_endpoint_in_another_directory() {
    use std::path::Path;

    use seismograph_protocol::Error;
    use seismograph_protocol::discovery::Directory;
    use seismograph_protocol::io::{IoOperation, MonitorIo};
    use seismograph_protocol::monitor::{MonitorDescriptor, MonitorEndpoint, MonitorOwner};

    let (io, control) = MonitorIo::new_mocked();
    control.set_owner(MonitorOwner::UnixUser(1001));
    let directory = Directory::current(&io).unwrap();
    let instance_id = InstanceId::from_bytes([0xa5; 16]);
    let descriptor = MonitorDescriptor {
        name: "round2".into(),
        instance: None,
        process_id: 42,
        instance_id,
        owner: directory.owner().clone(),
        endpoint: MonitorEndpoint::for_instance(Path::new("/another-registry"), directory.owner(), 42, instance_id),
    };
    assert!(matches!(
        io.publish_descriptor(&directory, &descriptor),
        Err(Error::InvalidDescriptor)
    ));
    assert!(!control.operations().contains(&IoOperation::CreateDescriptor));
    assert!(control.file_bytes(&directory.path().join(descriptor.file_name())).is_none());
    let path = directory.path().join("independent-encoding");
    descriptor.write_file(&io, &path).unwrap();
    assert!(control.file_bytes(&path).is_some());
}

#[test]
fn gamma_round2_recording_scalars_preserve_distinct_third_and_fourth_bytes() {
    let policy = RecordingPolicy {
        enabled: true,
        capture_backtraces: false,
        sampling_one_in: 0x0007_0605,
    };
    let request = Request::SetRecording(RecordingConfiguration {
        general_events: policy,
        ..RecordingConfiguration::default()
    });
    let mut bytes = Vec::new();
    write_request(&mut bytes, 19, &request).unwrap();
    assert_eq!(&bytes[32..36], &[5, 6, 7, 0]);
    assert_eq!(read_request(&mut bytes.as_slice()).unwrap(), (19, request));
    let response = Response::Hello {
        instance_id: InstanceId::from_bytes([0xa5; 16]),
        recording: RecordingConfiguration::default(),
    };
    bytes.clear();
    write_response(&mut bytes, 23, &response).unwrap();
    assert_eq!(&bytes[20..36], &[0xa5; 16]);
    assert_eq!(read_response(&mut bytes.as_slice()).unwrap(), (23, response));
}
