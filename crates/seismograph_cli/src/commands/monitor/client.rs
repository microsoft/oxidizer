// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs;
use std::io::{self, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use seismograph_protocol::message::{RecorderStatistics, RecordingConfiguration, RecordingPolicy, Request, Response, SnapshotOptions};
use seismograph_protocol::monitor::MonitorDescriptor;
use seismograph_protocol::{read_response, write_request};

use super::Error;
use super::app::Instance;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
const IO_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn discover() -> Result<Vec<Instance>, Error> {
    let directory = seismograph_protocol::monitor_directory().map_err(Error::Protocol)?;
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(Error::Io(error)),
    };
    let mut instances = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        if entry.path().extension().and_then(|extension| extension.to_str()) != Some("monitor") {
            continue;
        }
        let Ok(descriptor) = MonitorDescriptor::read_file(entry.path()) else {
            continue;
        };
        if let Ok(recording) = handshake(&descriptor) {
            instances.push(Instance { descriptor, recording });
        }
    }
    instances.sort_by(|left, right| {
        (&left.descriptor.name, &left.descriptor.instance, left.descriptor.process_id).cmp(&(
            &right.descriptor.name,
            &right.descriptor.instance,
            right.descriptor.process_id,
        ))
    });
    Ok(instances)
}

#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn set_recording(descriptor: &MonitorDescriptor, configuration: RecordingConfiguration) -> Result<(), Error> {
    let cache_supported = cache_recording(descriptor)?.is_some();
    if !cache_supported && configuration.cache != RecordingPolicy::default() {
        return Err(Error::Remote("cache recording is not supported by this monitor".into()));
    }
    match command(descriptor, &Request::SetRecording(configuration))? {
        Response::Acknowledged => {}
        _ => return Err(Error::UnexpectedResponse),
    }
    if cache_supported {
        match command(descriptor, &Request::SetCacheRecording(configuration.cache))? {
            Response::Acknowledged => {}
            _ => return Err(Error::UnexpectedResponse),
        }
    }
    Ok(())
}

#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn capture_snapshot(descriptor: &MonitorDescriptor, options: SnapshotOptions) -> Result<Vec<u8>, Error> {
    match command(descriptor, &Request::CaptureSnapshot(options))? {
        Response::Snapshot(bytes) => Ok(bytes),
        _ => Err(Error::UnexpectedResponse),
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn recorder_statistics(descriptor: &MonitorDescriptor) -> Result<RecorderStatistics, Error> {
    match command(descriptor, &Request::ReadRecorderStatistics)? {
        Response::RecorderStatistics(statistics) => Ok(statistics),
        _ => Err(Error::UnexpectedResponse),
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn save_snapshot(descriptor: &MonitorDescriptor, bytes: &[u8]) -> Result<PathBuf, Error> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| Error::Clock(error.to_string()))?
        .as_nanos();
    let name = sanitize(&descriptor.name);
    let path = std::env::current_dir()
        .map_err(Error::Io)?
        .join(format!("{name}-{}-{timestamp}.seismograph", descriptor.process_id));
    let mut file = fs::File::create(&path).map_err(Error::Io)?;
    file.write_all(bytes).map_err(Error::Io)?;
    Ok(path)
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn handshake(descriptor: &MonitorDescriptor) -> Result<RecordingConfiguration, Error> {
    let mut stream = connect(descriptor)?;
    write_request(
        &mut stream,
        1,
        &Request::Hello {
            authentication: descriptor.authentication,
        },
    )
    .map_err(Error::Protocol)?;
    match read_response(&mut stream).map_err(Error::Protocol)? {
        (
            1,
            Response::Hello {
                instance_id,
                mut recording,
            },
        ) if instance_id == descriptor.instance_id => {
            write_request(&mut stream, 2, &Request::ReadCacheRecording).map_err(Error::Protocol)?;
            if let Ok((2, Response::CacheRecording(cache))) = read_response(&mut stream) {
                recording.cache = cache;
            }
            Ok(recording)
        }
        (_, Response::Error(message)) => Err(Error::Remote(message)),
        _ => Err(Error::UnexpectedResponse),
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn cache_recording(descriptor: &MonitorDescriptor) -> Result<Option<seismograph_protocol::message::RecordingPolicy>, Error> {
    let mut stream = connect(descriptor)?;
    write_request(
        &mut stream,
        1,
        &Request::Hello {
            authentication: descriptor.authentication,
        },
    )
    .map_err(Error::Protocol)?;
    match read_response(&mut stream).map_err(Error::Protocol)? {
        (1, Response::Hello { instance_id, .. }) if instance_id == descriptor.instance_id => {}
        (_, Response::Error(message)) => return Err(Error::Remote(message)),
        _ => return Err(Error::UnexpectedResponse),
    }
    write_request(&mut stream, 2, &Request::ReadCacheRecording).map_err(Error::Protocol)?;
    Ok(match read_response(&mut stream) {
        Ok((2, Response::CacheRecording(policy))) => Some(policy),
        _ => None,
    })
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn connect(descriptor: &MonitorDescriptor) -> Result<TcpStream, Error> {
    let address = SocketAddr::V4(descriptor.socket_address());
    let stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).map_err(Error::Io)?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).map_err(Error::Io)?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).map_err(Error::Io)?;
    Ok(stream)
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn command(descriptor: &MonitorDescriptor, request: &Request) -> Result<Response, Error> {
    let mut stream = connect(descriptor)?;
    write_request(
        &mut stream,
        1,
        &Request::Hello {
            authentication: descriptor.authentication,
        },
    )
    .map_err(Error::Protocol)?;
    match read_response(&mut stream).map_err(Error::Protocol)? {
        (1, Response::Hello { instance_id, .. }) if instance_id == descriptor.instance_id => {}
        (_, Response::Error(message)) => return Err(Error::Remote(message)),
        _ => return Err(Error::UnexpectedResponse),
    }
    write_request(&mut stream, 2, request).map_err(Error::Protocol)?;
    let (request_id, response) = read_response(&mut stream).map_err(Error::Protocol)?;
    if request_id != 2 {
        return Err(Error::UnexpectedResponse);
    }
    match response {
        Response::Error(message) => Err(Error::Remote(message)),
        response => Ok(response),
    }
}

fn sanitize(value: &str) -> String {
    let value: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    if value.is_empty() { "seismograph".into() } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_file_names_replace_unsupported_characters() {
        assert_eq!(sanitize("worker west/europe"), "worker-west-europe");
    }

    #[test]
    fn empty_snapshot_file_name_uses_fallback() {
        assert_eq!(sanitize(""), "seismograph");
    }
}
