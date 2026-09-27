// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Message models exchanged through the monitor protocol.

use std::borrow::Cow;

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
    /// Cache operation recording policy, included in recorder activity and dedicated cache messages.
    ///
    /// The legacy fixed-size recording block used by `Hello`, `SetRecording`, and recorder
    /// statistics does not contain this field.
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
    /// Keeps retained events and active-thread backing buffers.
    ///
    /// Buffers belonging to exited threads are released after capture.
    #[default]
    Retain,
    /// Discards retained events after capture while keeping active-thread buffers.
    Clear,
    /// Discards retained events after capture and releases active-thread buffers,
    /// then restores the previous recording policies. This does not stop recording.
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

/// Accepted event sequence attempts by class in the current retained recording session.
///
/// These counters exclude disabled, suppressed, and sampled-out events. Like
/// [`RecorderStatistics::total_events`], they include attempts whose ring slot
/// subsequently times out. Counts saturate at [`u64::MAX`] and remain monotonic
/// within a [`RecorderActivity::session_id`], including after exited threads'
/// event rings are released.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EventClassCounts {
    /// Allocation and deallocation events.
    pub allocations: u64,
    /// Ordinary primitive events.
    pub general_events: u64,
    /// Arc dereference events.
    pub arc_dereferences: u64,
    /// Runtime task and scheduling events.
    pub runtime_tasks: u64,
    /// I/O operation events.
    pub io: u64,
    /// Cache operation events.
    pub cache: u64,
}

/// Lightweight counters for a thread participating in the retained recording session.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ThreadRecorderStatistics {
    /// Process-unique recorder identity, not an operating-system thread identifier.
    pub thread_id: u64,
    /// Recorded thread name, or an empty string for an unnamed thread.
    pub name: String,
    /// Accepted event sequence attempts since this thread joined the session.
    pub total_events: u64,
    /// Occupied ring positions, bounded by this thread's actual capacity.
    pub retained_events: u64,
    /// Accepted sequence attempts no longer retained, whether overwritten or released.
    pub lost_events: u64,
    /// Actual capacity of this thread's allocated event ring, or zero after release.
    pub event_capacity: u64,
    /// Whether the thread has exited and can no longer write events.
    pub retired: bool,
}

/// Live recorder activity without copying events or invoking snapshot sources.
///
/// Concurrent counters are sampled independently, not as an atomic snapshot.
/// Each read belongs to one recording session; global clear/release operations
/// cannot split a read.
/// Class and per-thread accepted totals survive natural retired-ring release.
/// Clients must rebase rate calculations whenever `session_id` changes, including
/// after a destructive buffer operation that only partially succeeds.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecorderActivity {
    /// Legacy ring-based counters, including all six recording policies.
    ///
    /// Unlike `class_events`, these counters exclude released rings and are not
    /// monotonic. Use class counters for live event rates.
    pub statistics: RecorderStatistics,
    /// Rate-baseline generation; zero before recording or buffer resets begin.
    ///
    /// Changes with recording sessions and destructive Clear, Release, or Stop
    /// operations. This need not equal a snapshot source's recording session.
    pub session_id: u64,
    /// Accepted sequence attempts, including threads whose rings have been released.
    pub class_events: EventClassCounts,
    /// Participating threads, including exited threads whose rings were released.
    ///
    /// Natural ring release preserves the entry with zero capacity and retention.
    /// Explicitly cleared histories are absent until the thread records again.
    pub threads: Vec<ThreadRecorderStatistics>,
}

/// Request sent by a monitor client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Request {
    /// Authenticates with the monitor.
    Hello {
        /// Descriptor-provided authentication token.
        authentication: AuthenticationToken,
    },
    /// Changes process-wide non-cache recording configuration.
    ///
    /// Cache recording is changed independently with [`Request::SetCacheRecording`].
    SetRecording(RecordingConfiguration),
    /// Changes cache-event recording configuration.
    SetCacheRecording(RecordingPolicy),
    /// Captures one complete encoded snapshot.
    CaptureSnapshot(SnapshotOptions),
    /// Reads recorder counters without copying retained events.
    ReadRecorderStatistics,
    /// Reads cache-event recording configuration.
    ReadCacheRecording,
    /// Reads the server's `seismograph` crate semantic version after authentication.
    ///
    /// Legacy servers close the connection without a response to this request.
    /// The fixed-size [`Response::Hello`] remains unchanged for legacy clients.
    ReadServerVersion,
    /// Captures events, stops every recording class, and releases event buffers.
    ///
    /// Additive request: legacy servers close without responding. Never substitute
    /// the legacy `Release` disposition, which resumes recording.
    CaptureSnapshotAndStop,
    /// Empties server event buffers without a snapshot, preserving recording policies.
    ClearEventBuffers,
    /// Reads live per-class and per-thread recorder counters without a snapshot.
    ///
    /// Legacy servers close the connection without responding.
    ReadRecorderActivity,
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
    /// Live per-class and per-thread recorder counters.
    RecorderActivity(RecorderActivity),
    /// Current cache-event recording policy.
    CacheRecording(RecordingPolicy),
    /// Semantic version of the server's `seismograph` crate, not the protocol crate
    /// or the application embedding it.
    ServerVersion(String),
    /// Request failure reported by the monitor.
    Error(String),
}

pub(crate) fn encode_request(request: &Request) -> Result<(u16, Vec<u8>), Error> {
    Ok(match request {
        Request::Hello { authentication } => {
            let mut payload = Vec::with_capacity(32);
            payload.extend_from_slice(&authentication.as_bytes());
            (1, payload)
        }
        Request::SetRecording(configuration) => {
            validate_legacy_recording(*configuration)?;
            (2, encode_recording(*configuration))
        }
        Request::CaptureSnapshot(options) => (3, vec![encode_event_buffer_disposition(options.event_buffers)]),
        Request::ReadRecorderStatistics => (4, Vec::new()),
        Request::SetCacheRecording(policy) => {
            let mut payload = Vec::with_capacity(6);
            encode_recording_policy(&mut payload, *policy);
            (5, payload)
        }
        Request::ReadCacheRecording => (6, Vec::new()),
        Request::ReadServerVersion => (7, Vec::new()),
        Request::CaptureSnapshotAndStop => (8, Vec::new()),
        Request::ClearEventBuffers => (9, Vec::new()),
        Request::ReadRecorderActivity => (10, Vec::new()),
    })
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
        7 if payload.is_empty() => Ok(Request::ReadServerVersion),
        8 if payload.is_empty() => Ok(Request::CaptureSnapshotAndStop),
        9 if payload.is_empty() => Ok(Request::ClearEventBuffers),
        10 if payload.is_empty() => Ok(Request::ReadRecorderActivity),
        _ => Err(Error::InvalidMessage),
    }
}

pub(crate) fn encode_response(response: &Response) -> Result<(u16, Cow<'_, [u8]>), Error> {
    match response {
        Response::Hello { instance_id, recording } => {
            validate_legacy_recording(*recording)?;
            let mut payload = Vec::with_capacity(50);
            payload.extend_from_slice(&instance_id.as_bytes());
            payload.extend_from_slice(&encode_recording(*recording));
            Ok((101, Cow::Owned(payload)))
        }
        Response::Acknowledged => Ok((102, Cow::Borrowed(&[]))),
        Response::Snapshot(bytes) => Ok((103, Cow::Borrowed(bytes))),
        Response::RecorderStatistics(statistics) => {
            validate_legacy_recording(statistics.recording)?;
            let payload = encode_statistics(*statistics);
            Ok((104, Cow::Owned(payload)))
        }
        Response::RecorderActivity(activity) => Ok((107, Cow::Owned(encode_activity(activity)?))),
        Response::CacheRecording(policy) => {
            let mut payload = Vec::with_capacity(6);
            encode_recording_policy(&mut payload, *policy);
            Ok((105, Cow::Owned(payload)))
        }
        Response::Error(message) => {
            let mut payload = Vec::new();
            push_string(&mut payload, message)?;
            Ok((255, Cow::Owned(payload)))
        }
        Response::ServerVersion(version) => {
            if version.is_empty() {
                return Err(Error::InvalidMessage);
            }
            let mut payload = Vec::new();
            push_string(&mut payload, version)?;
            Ok((106, Cow::Owned(payload)))
        }
    }
}

pub(crate) fn decode_response(kind: u16, payload: &[u8]) -> Result<Response, Error> {
    match kind {
        101 => {
            if payload.len() != 50 {
                return Err(Error::InvalidMessage);
            }
            let mut instance = [0; 16];
            instance.copy_from_slice(&payload[..16]);
            Ok(Response::Hello {
                instance_id: InstanceId::from_bytes(instance),
                recording: decode_recording(&payload[16..])?,
            })
        }
        102 if payload.is_empty() => Ok(Response::Acknowledged),
        103 => Ok(Response::Snapshot(payload.to_vec())),
        104 => decode_statistics(payload).map(Response::RecorderStatistics),
        107 => decode_activity(payload).map(Response::RecorderActivity),
        105 => decode_recording_policy(payload).map(Response::CacheRecording),
        106 => {
            let mut reader = SliceReader::new(payload);
            let version = reader.string()?;
            reader.finish()?;
            if version.is_empty() {
                return Err(Error::InvalidMessage);
            }
            Ok(Response::ServerVersion(version))
        }
        255 => {
            let mut reader = SliceReader::new(payload);
            let message = reader.string()?;
            reader.finish()?;
            Ok(Response::Error(message))
        }
        _ => Err(Error::InvalidMessage),
    }
}

fn encode_statistics(statistics: RecorderStatistics) -> Vec<u8> {
    let mut payload = Vec::with_capacity(82);
    push_u64(&mut payload, statistics.thread_count);
    push_u64(&mut payload, statistics.total_events);
    push_u64(&mut payload, statistics.retained_events);
    push_u64(&mut payload, statistics.lost_events);
    push_u64(&mut payload, statistics.event_capacity_per_thread);
    push_u64(&mut payload, statistics.allocated_bytes);
    payload.extend_from_slice(&encode_recording(statistics.recording));
    payload
}

fn decode_statistics(payload: &[u8]) -> Result<RecorderStatistics, Error> {
    if payload.len() != 82 {
        return Err(Error::InvalidMessage);
    }
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
    Ok(statistics)
}

// Legacy statistics, cache policy, session, six class counts, and thread count.
const ACTIVITY_HEADER_BYTES: usize = 82 + 6 + 8 + 6 * 8 + 4;
// Five counters, the retired flag, and the length prefix of an empty name.
const ACTIVITY_THREAD_MIN_BYTES: usize = 5 * 8 + 1 + 2;

fn encode_activity(activity: &RecorderActivity) -> Result<Vec<u8>, Error> {
    let thread_count = u32::try_from(activity.threads.len()).map_err(|_error| Error::MessageTooLarge)?;
    let size = activity.threads.iter().try_fold(ACTIVITY_HEADER_BYTES, |size, thread| {
        u16::try_from(thread.name.len()).map_err(|_error| Error::MessageTooLarge)?;
        size.checked_add(ACTIVITY_THREAD_MIN_BYTES + thread.name.len())
            .ok_or(Error::MessageTooLarge)
    })?;
    u32::try_from(size).map_err(|_error| Error::MessageTooLarge)?;
    let mut payload = encode_statistics(activity.statistics);
    decode_statistics(&payload)?;
    encode_recording_policy(&mut payload, activity.statistics.recording.cache);
    decode_recording_policy(&payload[82..88])?;
    payload
        .try_reserve_exact(size - payload.len())
        .map_err(|_error| Error::MessageTooLarge)?;
    push_u64(&mut payload, activity.session_id);
    for count in [
        activity.class_events.allocations,
        activity.class_events.general_events,
        activity.class_events.arc_dereferences,
        activity.class_events.runtime_tasks,
        activity.class_events.io,
        activity.class_events.cache,
    ] {
        push_u64(&mut payload, count);
    }
    push_u32(&mut payload, thread_count);
    for thread in &activity.threads {
        push_u64(&mut payload, thread.thread_id);
        push_u64(&mut payload, thread.total_events);
        push_u64(&mut payload, thread.retained_events);
        push_u64(&mut payload, thread.lost_events);
        push_u64(&mut payload, thread.event_capacity);
        payload.push(u8::from(thread.retired));
        push_string(&mut payload, &thread.name)?;
    }
    Ok(payload)
}

fn decode_activity(payload: &[u8]) -> Result<RecorderActivity, Error> {
    let mut reader = SliceReader::new(payload);
    let mut statistics = decode_statistics(reader.take(82)?)?;
    statistics.recording.cache = decode_recording_policy(reader.take(6)?)?;
    let session_id = reader.u64()?;
    let class_events = EventClassCounts {
        allocations: reader.u64()?,
        general_events: reader.u64()?,
        arc_dereferences: reader.u64()?,
        runtime_tasks: reader.u64()?,
        io: reader.u64()?,
        cache: reader.u64()?,
    };
    let thread_count = usize::try_from(reader.u32()?).map_err(|_error| Error::MessageTooLarge)?;
    if thread_count > payload.len().saturating_sub(ACTIVITY_HEADER_BYTES) / ACTIVITY_THREAD_MIN_BYTES {
        return Err(Error::InvalidMessage);
    }
    let mut threads = Vec::new();
    threads.try_reserve_exact(thread_count).map_err(|_error| Error::MessageTooLarge)?;
    for _ in 0..thread_count {
        let thread_id = reader.u64()?;
        let total_events = reader.u64()?;
        let retained_events = reader.u64()?;
        let lost_events = reader.u64()?;
        let event_capacity = reader.u64()?;
        let retired = match reader.u8()? {
            0 => false,
            1 => true,
            _ => return Err(Error::InvalidMessage),
        };
        threads.push(ThreadRecorderStatistics {
            thread_id,
            name: reader.string()?,
            total_events,
            retained_events,
            lost_events,
            event_capacity,
            retired,
        });
    }
    reader.finish()?;
    Ok(RecorderActivity {
        statistics,
        session_id,
        class_events,
        threads,
    })
}

fn validate_legacy_recording(configuration: RecordingConfiguration) -> Result<(), Error> {
    if configuration.cache != RecordingPolicy::default() {
        return Err(Error::InvalidMessage);
    }
    Ok(())
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
    #[test]
    fn lifecycle_requests_are_additive_and_reject_payloads() {
        for (tag, request) in [(8, super::Request::CaptureSnapshotAndStop), (9, super::Request::ClearEventBuffers)] {
            assert_eq!(super::encode_request(&request).unwrap(), (tag, Vec::new()));
            assert_eq!(super::decode_request(tag, &[]).unwrap(), request);
            super::decode_request(tag, &[0]).unwrap_err();
            super::decode_request(tag, &[1, 2]).unwrap_err();
        }
        assert_eq!(
            super::decode_request(3, &[2]).unwrap(),
            super::Request::CaptureSnapshot(super::SnapshotOptions {
                event_buffers: super::EventBufferDisposition::Release
            })
        );
        super::decode_request(3, &[3]).unwrap_err();
    }

    use super::*;
    use crate::{read_request, read_response, write_request, write_response};

    fn activity_fixture() -> RecorderActivity {
        RecorderActivity {
            statistics: RecorderStatistics {
                thread_count: 1,
                total_events: 11,
                retained_events: 11,
                event_capacity_per_thread: 128,
                recording: RecordingConfiguration {
                    cache: RecordingPolicy {
                        enabled: true,
                        capture_backtraces: true,
                        sampling_one_in: 3,
                    },
                    event_capacity_per_thread: 128,
                    ..Default::default()
                },
                ..Default::default()
            },
            session_id: 42,
            class_events: EventClassCounts {
                allocations: 1,
                general_events: 2,
                arc_dereferences: 3,
                runtime_tasks: 4,
                io: 5,
                cache: 6,
            },
            threads: vec![
                ThreadRecorderStatistics {
                    thread_id: 7,
                    name: "worker".into(),
                    total_events: 11,
                    retained_events: 11,
                    event_capacity: 64,
                    ..Default::default()
                },
                ThreadRecorderStatistics {
                    thread_id: 8,
                    name: "exited-λ".into(),
                    total_events: 10,
                    lost_events: 10,
                    retired: true,
                    ..Default::default()
                },
            ],
        }
    }

    #[test]
    fn activity_messages_round_trip_with_all_classes_threads_and_cache_policy() {
        let mut bytes = Vec::new();
        write_request(&mut bytes, 40, &Request::ReadRecorderActivity).unwrap();
        assert_eq!(read_request(&mut bytes.as_slice()).unwrap(), (40, Request::ReadRecorderActivity));
        assert_eq!(encode_request(&Request::ReadRecorderActivity).unwrap(), (10, Vec::new()));
        for payload in [&[0][..], &[1, 2][..]] {
            decode_request(10, payload).unwrap_err();
        }
        for activity in [RecorderActivity::default(), activity_fixture()] {
            let response = Response::RecorderActivity(activity);
            assert_eq!(encode_response(&response).unwrap().0, 107);
            bytes.clear();
            write_response(&mut bytes, 41, &response).unwrap();
            assert_eq!(read_response(&mut bytes.as_slice()).unwrap(), (41, response));
        }
    }

    #[test]
    fn activity_rejects_truncation_trailing_data_and_impossible_thread_counts() {
        let payload = encode_activity(&activity_fixture()).unwrap();
        for len in 0..payload.len() {
            decode_response(107, &payload[..len]).unwrap_err();
        }
        let mut trailing = payload.clone();
        trailing.push(0);
        decode_response(107, &trailing).unwrap_err();
        for count in [0_u32, 1, 3, u32::MAX] {
            let mut invalid = payload.clone();
            invalid[ACTIVITY_HEADER_BYTES - 4..ACTIVITY_HEADER_BYTES].copy_from_slice(&count.to_le_bytes());
            decode_response(107, &invalid).unwrap_err();
        }
    }

    #[test]
    fn activity_rejects_invalid_retired_flags_names_and_name_bounds() {
        let payload = encode_activity(&activity_fixture()).unwrap();
        for flag in [2, 255] {
            let mut invalid = payload.clone();
            invalid[ACTIVITY_HEADER_BYTES + 40] = flag;
            decode_response(107, &invalid).unwrap_err();
        }
        let mut invalid = payload.clone();
        invalid[ACTIVITY_HEADER_BYTES + ACTIVITY_THREAD_MIN_BYTES] = 0xff;
        decode_response(107, &invalid).unwrap_err();
        let mut invalid = payload;
        invalid[ACTIVITY_HEADER_BYTES + 41..ACTIVITY_HEADER_BYTES + 43].copy_from_slice(&u16::MAX.to_le_bytes());
        decode_response(107, &invalid).unwrap_err();

        let mut activity = activity_fixture();
        activity.threads[0].name = "x".repeat(usize::from(u16::MAX));
        assert_eq!(decode_activity(&encode_activity(&activity).unwrap()).unwrap(), activity);
        activity.threads[0].name.push('x');
        assert!(matches!(encode_activity(&activity), Err(Error::MessageTooLarge)));
    }

    #[test]
    fn activity_validates_all_recording_policies_and_capacity() {
        let payload = encode_activity(&activity_fixture()).unwrap();
        for offset in [52, 58, 64, 70, 76, 82] {
            for boolean in [offset, offset + 1] {
                let mut invalid = payload.clone();
                invalid[boolean] = 2;
                decode_response(107, &invalid).unwrap_err();
            }
            for sampling in [0_u32, MAX_EVENT_SAMPLING_ONE_IN + 1] {
                let mut invalid = payload.clone();
                invalid[offset + 2..offset + 6].copy_from_slice(&sampling.to_le_bytes());
                decode_response(107, &invalid).unwrap_err();
            }
        }
        for capacity in [0_u32, 63, 65, MAX_EVENT_CAPACITY_PER_THREAD * 2] {
            let mut invalid = payload.clone();
            invalid[48..52].copy_from_slice(&capacity.to_le_bytes());
            decode_response(107, &invalid).unwrap_err();
            let mut activity = activity_fixture();
            activity.statistics.recording.event_capacity_per_thread = capacity;
            encode_activity(&activity).unwrap_err();
        }
        for sampling in [0, MAX_EVENT_SAMPLING_ONE_IN + 1] {
            let mut activity = activity_fixture();
            activity.statistics.recording.cache.sampling_one_in = sampling;
            encode_activity(&activity).unwrap_err();
            activity.statistics.recording.cache = RecordingPolicy::default();
            activity.statistics.recording.general_events.sampling_one_in = sampling;
            encode_activity(&activity).unwrap_err();
        }
    }

    #[test]
    fn server_version_messages_round_trip() {
        let mut bytes = Vec::new();
        write_request(&mut bytes, 2, &Request::ReadServerVersion).unwrap();
        assert_eq!(read_request(&mut bytes.as_slice()).unwrap(), (2, Request::ReadServerVersion));

        let response = Response::ServerVersion("12.34.56-rc.7+build.8".into());
        bytes.clear();
        write_response(&mut bytes, 2, &response).unwrap();
        assert_eq!(read_response(&mut bytes.as_slice()).unwrap(), (2, response));
    }

    #[test]
    fn server_version_payloads_reject_empty_truncated_invalid_utf8_and_trailing_data() {
        for payload in [vec![], vec![0, 0], vec![1, 0], vec![1, 0, 0xff], vec![1, 0, b'1', b'2']] {
            decode_response(106, &payload).unwrap_err();
        }
        encode_response(&Response::ServerVersion(String::new())).unwrap_err();
        decode_request(7, &[0]).unwrap_err();
    }

    #[test]
    fn hello_wire_payload_is_identical_to_the_legacy_fixture() {
        // Legacy kind 101: 16-byte identity, u32 capacity, five 6-byte policies.
        let legacy = [
            9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0,
            0, 0, 0, 0, 1, 0, 0, 0,
        ];
        let response = Response::Hello {
            instance_id: InstanceId::from_bytes([9; 16]),
            recording: RecordingConfiguration::default(),
        };
        let (kind, payload) = encode_response(&response).unwrap();
        assert_eq!((kind, payload.as_ref()), (101, legacy.as_slice()));
        assert_eq!(decode_response(101, &legacy).unwrap(), response);
        let mut extended = legacy.to_vec();
        extended.push(0);
        decode_response(101, &extended).unwrap_err();
    }

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
    fn empty_and_chunked_snapshot_responses_round_trip() {
        for len in [0, 1, 8_191, 8_192, 8_193] {
            let response = Response::Snapshot(vec![0xA5; len]);
            let mut bytes = Vec::new();
            write_response(&mut bytes, 23, &response).unwrap();

            assert_eq!(read_response(&mut bytes.as_slice()).unwrap(), (23, response));
        }
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
        let configuration = RecordingConfiguration::default();
        let hello_response = Response::Hello {
            instance_id: InstanceId::from_bytes([1; 16]),
            recording: configuration,
        };
        let statistics_response = Response::RecorderStatistics(RecorderStatistics {
            recording: configuration,
            ..RecorderStatistics::default()
        });
        let (_, hello) = encode_response(&hello_response).unwrap();
        let (_, statistics) = encode_response(&statistics_response).unwrap();

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
    fn legacy_recording_messages_reject_cache_policy_instead_of_discarding_it() {
        let configuration = RecordingConfiguration {
            cache: RecordingPolicy {
                enabled: true,
                ..RecordingPolicy::default()
            },
            ..RecordingConfiguration::default()
        };

        assert!(matches!(
            write_request(&mut Vec::new(), 1, &Request::SetRecording(configuration)),
            Err(Error::InvalidMessage)
        ));
        assert!(matches!(
            write_response(
                &mut Vec::new(),
                1,
                &Response::Hello {
                    instance_id: InstanceId::from_bytes([1; 16]),
                    recording: configuration,
                },
            ),
            Err(Error::InvalidMessage)
        ));
        assert!(matches!(
            write_response(
                &mut Vec::new(),
                1,
                &Response::RecorderStatistics(RecorderStatistics {
                    recording: configuration,
                    ..RecorderStatistics::default()
                }),
            ),
            Err(Error::InvalidMessage)
        ));
    }

    #[test]
    fn legacy_recording_block_has_stable_wire_layout() {
        let configuration = RecordingConfiguration {
            event_capacity_per_thread: 1_024,
            allocations: RecordingPolicy {
                enabled: true,
                capture_backtraces: false,
                sampling_one_in: 2,
            },
            general_events: RecordingPolicy {
                enabled: false,
                capture_backtraces: true,
                sampling_one_in: 4,
            },
            arc_dereferences: RecordingPolicy {
                enabled: true,
                capture_backtraces: true,
                sampling_one_in: 8,
            },
            runtime_tasks: RecordingPolicy {
                enabled: false,
                capture_backtraces: false,
                sampling_one_in: 16,
            },
            io: RecordingPolicy {
                enabled: true,
                capture_backtraces: false,
                sampling_one_in: 32,
            },
            cache: RecordingPolicy::default(),
        };

        assert_eq!(
            encode_recording(configuration),
            [
                0, 4, 0, 0, 1, 0, 2, 0, 0, 0, 0, 1, 4, 0, 0, 0, 1, 1, 8, 0, 0, 0, 0, 0, 16, 0, 0, 0, 1, 0, 32, 0, 0, 0,
            ]
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
    fn fixed_size_messages_reject_every_adjacent_invalid_length() {
        for (kind, expected_len) in [(1, 32_usize), (3, 1), (4, 0), (6, 0)] {
            for len in [expected_len.saturating_sub(1), expected_len + 1] {
                if len != expected_len {
                    assert!(matches!(decode_request(kind, &vec![0; len]), Err(Error::InvalidMessage)));
                }
            }
        }

        for (kind, expected_len) in [(101, 50_usize), (102, 0), (104, 82)] {
            for len in [expected_len.saturating_sub(1), expected_len + 1] {
                if len != expected_len {
                    assert!(
                        matches!(decode_response(kind, &vec![0; len]), Err(Error::InvalidMessage)),
                        "response kind {kind} unexpectedly accepted length {len}"
                    );
                }
            }
        }
    }

    #[test]
    fn recording_policy_rejects_noncanonical_booleans_and_boundary_sampling() {
        for offset in [0, 1] {
            let mut payload = [0, 0, 1, 0, 0, 0];
            payload[offset] = 2;
            assert!(matches!(decode_recording_policy(&payload), Err(Error::InvalidMessage)));
        }
        for sampling in [0, MAX_EVENT_SAMPLING_ONE_IN + 1] {
            let mut payload = [0, 0, 0, 0, 0, 0];
            payload[2..].copy_from_slice(&sampling.to_le_bytes());
            assert!(matches!(decode_recording_policy(&payload), Err(Error::InvalidMessage)));
        }
        let mut maximum = [0, 0, 0, 0, 0, 0];
        maximum[2..].copy_from_slice(&MAX_EVENT_SAMPLING_ONE_IN.to_le_bytes());
        assert_eq!(
            decode_recording_policy(&maximum).unwrap().sampling_one_in,
            MAX_EVENT_SAMPLING_ONE_IN
        );
        for len in [0, 5, 7] {
            assert!(matches!(
                decode_recording_policy(&[0, 0, 1, 0, 0, 0, 0][..len]),
                Err(Error::InvalidMessage)
            ));
        }
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
