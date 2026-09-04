// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Message models exchanged through the monitor protocol.

use crate::Error;
use crate::codec::{SliceReader, push_string, push_u32, push_u64};
use crate::monitor::{AuthenticationToken, InstanceId};

const DEFAULT_EVENT_CAPACITY_PER_THREAD: u32 = 65_536;
const MIN_EVENT_CAPACITY_PER_THREAD: u32 = 64;
const MAX_EVENT_CAPACITY_PER_THREAD: u32 = 1_048_576;
const MAX_EVENT_SAMPLING_ONE_IN: u32 = 1_048_576;

/// Recording state exchanged with a monitor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordingConfiguration {
    /// Allocation lifecycle recording policy.
    pub allocations: RecordingPolicy,
    /// Ordinary primitive-event recording policy.
    pub general_events: RecordingPolicy,
    /// Arc dereference recording policy.
    pub arc_dereferences: RecordingPolicy,
    /// Runtime task and scheduling-event recording policy.
    pub runtime_tasks: RecordingPolicy,
    /// I/O primitive operation recording policy.
    pub io: RecordingPolicy,
    /// Cache operation recording policy.
    pub cache: RecordingPolicy,
    /// Events retained by each participating thread.
    pub event_capacity_per_thread: u32,
}

/// Recording controls for one event class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordingPolicy {
    /// Whether events in this class are recorded.
    pub enabled: bool,
    /// Whether events in this class capture backtraces.
    pub capture_backtraces: bool,
    /// Records all events for approximately one in every X objects.
    pub sampling_one_in: u32,
}

impl Default for RecordingPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            capture_backtraces: false,
            sampling_one_in: 1,
        }
    }
}

impl Default for RecordingConfiguration {
    fn default() -> Self {
        Self {
            allocations: RecordingPolicy::default(),
            general_events: RecordingPolicy::default(),
            arc_dereferences: RecordingPolicy::default(),
            runtime_tasks: RecordingPolicy::default(),
            io: RecordingPolicy::default(),
            cache: RecordingPolicy::default(),
            event_capacity_per_thread: DEFAULT_EVENT_CAPACITY_PER_THREAD,
        }
    }
}

/// Treatment of event buffers after snapshot capture.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EventBufferDisposition {
    /// Keeps retained events and their backing buffers.
    #[default]
    Retain,
    /// Discards retained events after capture while keeping allocated buffers.
    Clear,
    /// Discards retained events after capture and releases their backing buffers.
    Release,
}

/// Options controlling remote snapshot capture.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SnapshotOptions {
    /// Treatment applied after event capture.
    pub event_buffers: EventBufferDisposition,
}

/// Lightweight runtime recorder counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RecorderStatistics {
    /// Threads that emitted events in the current recording session.
    pub thread_count: u64,
    /// Events emitted in the current recording session.
    pub total_events: u64,
    /// Events currently retained across thread rings.
    pub retained_events: u64,
    /// Events overwritten across thread rings.
    pub lost_events: u64,
    /// Configured event capacity for each newly active thread.
    pub event_capacity_per_thread: u64,
    /// Memory retained by recorder metadata and event buffers.
    pub allocated_bytes: u64,
    /// Policies used by the active recording session.
    pub recording: RecordingConfiguration,
}

/// Request sent by a monitor client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Request {
    /// Authenticates with the monitor.
    Hello {
        /// Descriptor-provided authentication token.
        authentication: AuthenticationToken,
    },
    /// Changes process-wide recording configuration.
    SetRecording(RecordingConfiguration),
    /// Changes cache-event recording configuration.
    SetCacheRecording(RecordingPolicy),
    /// Captures one complete encoded snapshot.
    CaptureSnapshot(SnapshotOptions),
    /// Reads recorder counters without copying retained events.
    ReadRecorderStatistics,
    /// Reads cache-event recording configuration.
    ReadCacheRecording,
}

/// Response returned by a monitor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Response {
    /// Successful handshake and current process state.
    Hello {
        /// Connected process identity.
        instance_id: InstanceId,
        /// Current recording configuration.
        recording: RecordingConfiguration,
    },
    /// Successful command with no payload.
    Acknowledged,
    /// Complete encoded Seismograph snapshot.
    Snapshot(Vec<u8>),
    /// Current lightweight runtime recorder counters.
    RecorderStatistics(RecorderStatistics),
    /// Current cache-event recording policy.
    CacheRecording(RecordingPolicy),
    /// Request failure reported by the monitor.
    Error(String),
}

pub(crate) fn encode_request(request: &Request) -> (u16, Vec<u8>) {
    match request {
        Request::Hello { authentication } => {
            let mut payload = Vec::with_capacity(32);
            payload.extend_from_slice(&authentication.as_bytes());
            (1, payload)
        }
        Request::SetRecording(configuration) => (2, encode_recording(*configuration)),
        Request::CaptureSnapshot(options) => (3, vec![encode_event_buffer_disposition(options.event_buffers)]),
        Request::ReadRecorderStatistics => (4, Vec::new()),
        Request::SetCacheRecording(policy) => {
            let mut payload = Vec::with_capacity(6);
            encode_recording_policy(&mut payload, *policy);
            (5, payload)
        }
        Request::ReadCacheRecording => (6, Vec::new()),
    }
}

pub(crate) fn decode_request(kind: u16, payload: &[u8]) -> Result<Request, Error> {
    match kind {
        1 if payload.len() == 32 => {
            let mut token = [0; 32];
            token.copy_from_slice(payload);
            Ok(Request::Hello {
                authentication: AuthenticationToken::from_bytes(token),
            })
        }
        2 => decode_recording(payload).map(Request::SetRecording),
        3 if payload.len() == 1 => Ok(Request::CaptureSnapshot(SnapshotOptions {
            event_buffers: decode_event_buffer_disposition(payload[0])?,
        })),
        4 if payload.is_empty() => Ok(Request::ReadRecorderStatistics),
        5 => decode_recording_policy(payload).map(Request::SetCacheRecording),
        6 if payload.is_empty() => Ok(Request::ReadCacheRecording),
        _ => Err(Error::InvalidMessage),
    }
}

pub(crate) fn encode_response(response: &Response) -> Result<(u16, Vec<u8>), Error> {
    match response {
        Response::Hello { instance_id, recording } => {
            let mut payload = Vec::with_capacity(50);
            payload.extend_from_slice(&instance_id.as_bytes());
            payload.extend_from_slice(&encode_recording(*recording));
            Ok((101, payload))
        }
        Response::Acknowledged => Ok((102, Vec::new())),
        Response::Snapshot(bytes) => Ok((103, bytes.clone())),
        Response::RecorderStatistics(statistics) => {
            let mut payload = Vec::with_capacity(82);
            push_u64(&mut payload, statistics.thread_count);
            push_u64(&mut payload, statistics.total_events);
            push_u64(&mut payload, statistics.retained_events);
            push_u64(&mut payload, statistics.lost_events);
            push_u64(&mut payload, statistics.event_capacity_per_thread);
            push_u64(&mut payload, statistics.allocated_bytes);
            payload.extend_from_slice(&encode_recording(statistics.recording));
            Ok((104, payload))
        }
        Response::CacheRecording(policy) => {
            let mut payload = Vec::with_capacity(6);
            encode_recording_policy(&mut payload, *policy);
            Ok((105, payload))
        }
        Response::Error(message) => {
            let mut payload = Vec::new();
            push_string(&mut payload, message)?;
            Ok((255, payload))
        }
    }
}

pub(crate) fn decode_response(kind: u16, payload: &[u8]) -> Result<Response, Error> {
    match kind {
        101 if payload.len() == 50 => {
            let mut instance = [0; 16];
            instance.copy_from_slice(&payload[..16]);
            Ok(Response::Hello {
                instance_id: InstanceId::from_bytes(instance),
                recording: decode_recording(&payload[16..])?,
            })
        }
        102 if payload.is_empty() => Ok(Response::Acknowledged),
        103 => Ok(Response::Snapshot(payload.to_vec())),
        104 if payload.len() == 82 => {
            let mut reader = SliceReader::new(payload);
            let statistics = RecorderStatistics {
                thread_count: reader.u64()?,
                total_events: reader.u64()?,
                retained_events: reader.u64()?,
                lost_events: reader.u64()?,
                event_capacity_per_thread: reader.u64()?,
                allocated_bytes: reader.u64()?,
                recording: decode_recording(reader.take(34)?)?,
            };
            reader.finish()?;
            Ok(Response::RecorderStatistics(statistics))
        }
        105 => decode_recording_policy(payload).map(Response::CacheRecording),
        255 => {
            let mut reader = SliceReader::new(payload);
            let message = reader.string()?;
            reader.finish()?;
            Ok(Response::Error(message))
        }
        _ => Err(Error::InvalidMessage),
    }
}

fn encode_recording(configuration: RecordingConfiguration) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(34);
    push_u32(&mut encoded, configuration.event_capacity_per_thread);
    encode_recording_policy(&mut encoded, configuration.allocations);
    encode_recording_policy(&mut encoded, configuration.general_events);
    encode_recording_policy(&mut encoded, configuration.arc_dereferences);
    encode_recording_policy(&mut encoded, configuration.runtime_tasks);
    encode_recording_policy(&mut encoded, configuration.io);
    encoded
}

fn decode_recording(payload: &[u8]) -> Result<RecordingConfiguration, Error> {
    if payload.len() != 34 {
        return Err(Error::InvalidMessage);
    }
    let capacity = u32::from_le_bytes(payload[..4].try_into().map_err(|_error| Error::InvalidMessage)?);
    if !(MIN_EVENT_CAPACITY_PER_THREAD..=MAX_EVENT_CAPACITY_PER_THREAD).contains(&capacity) || !capacity.is_power_of_two() {
        return Err(Error::InvalidMessage);
    }
    Ok(RecordingConfiguration {
        allocations: decode_recording_policy(&payload[4..10])?,
        general_events: decode_recording_policy(&payload[10..16])?,
        arc_dereferences: decode_recording_policy(&payload[16..22])?,
        runtime_tasks: decode_recording_policy(&payload[22..28])?,
        io: decode_recording_policy(&payload[28..34])?,
        cache: RecordingPolicy::default(),
        event_capacity_per_thread: capacity,
    })
}

fn encode_recording_policy(encoded: &mut Vec<u8>, policy: RecordingPolicy) {
    encoded.push(u8::from(policy.enabled));
    encoded.push(u8::from(policy.capture_backtraces));
    push_u32(encoded, policy.sampling_one_in);
}

fn decode_recording_policy(payload: &[u8]) -> Result<RecordingPolicy, Error> {
    if payload.len() != 6 || payload[..2].iter().any(|value| *value > 1) {
        return Err(Error::InvalidMessage);
    }
    let sampling_one_in = u32::from_le_bytes(payload[2..6].try_into().map_err(|_error| Error::InvalidMessage)?);
    if sampling_one_in == 0 || sampling_one_in > MAX_EVENT_SAMPLING_ONE_IN {
        return Err(Error::InvalidMessage);
    }
    Ok(RecordingPolicy {
        enabled: payload[0] != 0,
        capture_backtraces: payload[1] != 0,
        sampling_one_in,
    })
}

const fn encode_event_buffer_disposition(disposition: EventBufferDisposition) -> u8 {
    match disposition {
        EventBufferDisposition::Retain => 0,
        EventBufferDisposition::Clear => 1,
        EventBufferDisposition::Release => 2,
    }
}

const fn decode_event_buffer_disposition(encoded: u8) -> Result<EventBufferDisposition, Error> {
    match encoded {
        0 => Ok(EventBufferDisposition::Retain),
        1 => Ok(EventBufferDisposition::Clear),
        2 => Ok(EventBufferDisposition::Release),
        _ => Err(Error::InvalidMessage),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{read_request, read_response, write_request, write_response};

    #[test]
    fn hello_messages_round_trip_without_public_version_fields() {
        let authentication = AuthenticationToken::from_bytes([7; 32]);
        let request = Request::Hello { authentication };
        let mut bytes = Vec::new();
        write_request(&mut bytes, 1, &request).unwrap();
        assert_eq!(read_request(&mut bytes.as_slice()).unwrap(), (1, request));

        let response = Response::Hello {
            instance_id: InstanceId::from_bytes([9; 16]),
            recording: RecordingConfiguration::default(),
        };
        bytes.clear();
        write_response(&mut bytes, 1, &response).unwrap();
        assert_eq!(read_response(&mut bytes.as_slice()).unwrap(), (1, response));
    }

    #[test]
    fn requests_and_responses_round_trip() {
        let request = Request::SetRecording(RecordingConfiguration {
            event_capacity_per_thread: 1_024,
            allocations: RecordingPolicy {
                enabled: true,
                capture_backtraces: true,
                sampling_one_in: 20,
            },
            general_events: RecordingPolicy {
                enabled: true,
                capture_backtraces: false,
                sampling_one_in: 4,
            },
            arc_dereferences: RecordingPolicy {
                enabled: false,
                capture_backtraces: true,
                sampling_one_in: 100,
            },
            runtime_tasks: RecordingPolicy {
                enabled: true,
                capture_backtraces: true,
                sampling_one_in: 1,
            },
            io: RecordingPolicy {
                enabled: true,
                capture_backtraces: false,
                sampling_one_in: 8,
            },
            cache: RecordingPolicy::default(),
        });
        let mut bytes = Vec::new();
        write_request(&mut bytes, 17, &request).unwrap();
        assert_eq!(read_request(&mut bytes.as_slice()).unwrap(), (17, request));

        let response = Response::Snapshot(vec![1, 2, 3]);
        bytes.clear();
        write_response(&mut bytes, 18, &response).unwrap();
        assert_eq!(read_response(&mut bytes.as_slice()).unwrap(), (18, response));
    }

    #[test]
    fn recording_configuration_rejects_invalid_sampling_denominators() {
        for offset in [6, 12, 18, 24, 30] {
            for sampling in [0_u32, MAX_EVENT_SAMPLING_ONE_IN + 1] {
                let mut payload = encode_recording(RecordingConfiguration::default());
                payload[offset..offset + 4].copy_from_slice(&sampling.to_le_bytes());

                decode_recording(&payload).unwrap_err();
            }
        }
    }

    #[test]
    fn recorder_statistics_request_round_trips() {
        let request = Request::ReadRecorderStatistics;
        let mut bytes = Vec::new();
        write_request(&mut bytes, 19, &request).unwrap();

        assert_eq!(read_request(&mut bytes.as_slice()).unwrap(), (19, request));
    }

    #[test]
    fn cache_recording_messages_round_trip() {
        let policy = RecordingPolicy {
            enabled: true,
            capture_backtraces: true,
            sampling_one_in: 16,
        };
        let requests = [Request::SetCacheRecording(policy), Request::ReadCacheRecording];
        let responses = [Response::CacheRecording(policy), Response::Acknowledged];

        for (index, request) in requests.into_iter().enumerate() {
            let mut bytes = Vec::new();
            write_request(&mut bytes, index as u64, &request).unwrap();
            assert_eq!(read_request(&mut bytes.as_slice()).unwrap(), (index as u64, request));
        }
        for (index, response) in responses.into_iter().enumerate() {
            let mut bytes = Vec::new();
            write_response(&mut bytes, index as u64, &response).unwrap();
            assert_eq!(read_response(&mut bytes.as_slice()).unwrap(), (index as u64, response));
        }
    }

    #[test]
    fn legacy_recording_message_sizes_remain_stable() {
        let configuration = RecordingConfiguration {
            cache: RecordingPolicy {
                enabled: true,
                capture_backtraces: true,
                sampling_one_in: 8,
            },
            ..RecordingConfiguration::default()
        };
        let (_, hello) = encode_response(&Response::Hello {
            instance_id: InstanceId::from_bytes([1; 16]),
            recording: configuration,
        })
        .unwrap();
        let (_, statistics) = encode_response(&Response::RecorderStatistics(RecorderStatistics {
            recording: configuration,
            ..RecorderStatistics::default()
        }))
        .unwrap();

        assert_eq!(
            (
                encode_recording(configuration).len(),
                hello.len(),
                statistics.len(),
                decode_recording(&encode_recording(configuration)).unwrap().cache,
            ),
            (34, 50, 82, RecordingPolicy::default())
        );
    }

    #[test]
    fn destructive_snapshot_request_round_trips() {
        let request = Request::CaptureSnapshot(SnapshotOptions {
            event_buffers: EventBufferDisposition::Release,
        });
        let mut bytes = Vec::new();
        write_request(&mut bytes, 20, &request).unwrap();

        assert_eq!(read_request(&mut bytes.as_slice()).unwrap(), (20, request));
    }

    #[test]
    fn recorder_statistics_response_round_trips() {
        let response = Response::RecorderStatistics(RecorderStatistics {
            thread_count: 3,
            total_events: 400_000,
            retained_events: 196_608,
            lost_events: 203_392,
            event_capacity_per_thread: 65_536,
            allocated_bytes: 54_000_000,
            recording: RecordingConfiguration {
                allocations: RecordingPolicy {
                    enabled: true,
                    sampling_one_in: 32,
                    ..Default::default()
                },
                ..Default::default()
            },
        });
        let mut bytes = Vec::new();
        write_response(&mut bytes, 21, &response).unwrap();

        assert_eq!(read_response(&mut bytes.as_slice()).unwrap(), (21, response));
    }

    #[test]
    fn acknowledgement_and_error_responses_round_trip() {
        for response in [Response::Acknowledged, Response::Error("request rejected".into())] {
            let mut bytes = Vec::new();
            write_response(&mut bytes, 22, &response).unwrap();
            assert_eq!(read_response(&mut bytes.as_slice()).unwrap(), (22, response));
        }
    }

    #[test]
    fn invalid_message_shapes_are_rejected() {
        assert!(matches!(decode_request(99, &[]), Err(Error::InvalidMessage)));
        assert!(matches!(decode_response(99, &[]), Err(Error::InvalidMessage)));
        assert!(matches!(decode_response(102, &[1]), Err(Error::InvalidMessage)));
        assert!(matches!(decode_event_buffer_disposition(3), Err(Error::InvalidMessage)));

        let mut short_recording = encode_recording(RecordingConfiguration::default());
        short_recording.pop();
        assert!(matches!(decode_recording(&short_recording), Err(Error::InvalidMessage)));

        let mut invalid_capacity = encode_recording(RecordingConfiguration::default());
        invalid_capacity[..4].copy_from_slice(&65_u32.to_le_bytes());
        assert!(matches!(decode_recording(&invalid_capacity), Err(Error::InvalidMessage)));

        let mut invalid_boolean = encode_recording(RecordingConfiguration::default());
        invalid_boolean[4] = 2;
        assert!(matches!(decode_recording(&invalid_boolean), Err(Error::InvalidMessage)));
    }

    #[test]
    fn all_event_buffer_dispositions_round_trip() {
        for disposition in [
            EventBufferDisposition::Retain,
            EventBufferDisposition::Clear,
            EventBufferDisposition::Release,
        ] {
            assert_eq!(
                decode_event_buffer_disposition(encode_event_buffer_disposition(disposition)).unwrap(),
                disposition
            );
        }
    }
}
