// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native monitor startup and transfer without requiring test-only features.

#![cfg(not(miri))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::time::Duration;

use seismograph_protocol::discovery::Directory;
use seismograph_protocol::io::MonitorIo;
use seismograph_protocol::monitor::{InstanceId, MonitorDescriptor, MonitorEndpoint};
use seismograph_protocol::transport::{Connection, Listener};

struct PublishedDescriptor<'a> {
    io: &'a MonitorIo,
    path: PathBuf,
}

impl Drop for PublishedDescriptor<'_> {
    fn drop(&mut self) {
        let _ = self.io.remove_descriptor(&self.path);
    }
}

#[test]
fn native_transport_works_without_test_utilities() {
    let io = MonitorIo::new_native();
    let directory = Directory::current(&io).unwrap();
    let owner = directory.owner().clone();
    let process_id = io.process_id();
    assert_eq!(process_id, std::process::id());
    let executable = io.current_exe().unwrap();
    let instance_id = InstanceId::from_bytes([1; 16]);
    let descriptor = MonitorDescriptor {
        name: executable.file_stem().unwrap().to_string_lossy().into_owned(),
        instance: None,
        endpoint: MonitorEndpoint::for_instance(directory.path(), &owner, process_id, instance_id),
        owner,
        process_id,
        instance_id,
    };
    let listener = Listener::bind(&io, &descriptor).unwrap();
    let publication = PublishedDescriptor {
        io: &io,
        path: io.publish_descriptor(&directory, &descriptor).unwrap(),
    };
    let restored = MonitorDescriptor::read_file(&io, &publication.path).unwrap();
    assert_eq!(restored, descriptor);
    let reopened = Directory::open(&io, directory.path(), directory.owner()).unwrap();
    assert!(
        reopened
            .descriptors()
            .unwrap()
            .any(|entry| entry.is_ok_and(|value| value == descriptor))
    );
    let _directories = seismograph_protocol::discovery::directories(&io).unwrap();
    let mut client = Connection::connect(&io, &descriptor, Duration::from_secs(1)).unwrap();
    let mut server = listener.accept().unwrap();
    client.write_all(b"native").unwrap();
    let mut bytes = [0; 6];
    server.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"native");
    io.remove_descriptor(&publication.path).unwrap();
}
