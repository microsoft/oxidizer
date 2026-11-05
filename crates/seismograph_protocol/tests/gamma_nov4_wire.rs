// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Independent wire oracles for cache policies and multibyte counters.

use seismograph_protocol::message::{RecorderStatistics, RecordingConfiguration, RecordingPolicy, Request, Response};
use seismograph_protocol::{Error, read_request, read_response, write_request, write_response};

#[test]
fn cache_recording_preserves_each_byte_and_does_not_use_the_legacy_configuration() {
    let policy = RecordingPolicy {
        enabled: false,
        capture_backtraces: true,
        sampling_one_in: 0x0007_0605,
    };
    let payload = [0, 1, 5, 6, 7, 0];
    let mut request = Vec::new();
    write_request(&mut request, 0x0807_0605_0403_0201, &Request::SetCacheRecording(policy)).unwrap();
    assert_eq!(
        request,
        [b"SGMP".as_slice(), &[8, 0, 5, 0, 1, 2, 3, 4, 5, 6, 7, 8, 6, 0, 0, 0], &payload,].concat()
    );
    assert_eq!(
        read_request(&mut request.as_slice()).unwrap(),
        (0x0807_0605_0403_0201, Request::SetCacheRecording(policy))
    );
    let mut response = Vec::new();
    write_response(&mut response, 0x0807_0605_0403_0201, &Response::CacheRecording(policy)).unwrap();
    let mut expected = request.clone();
    expected[6] = 105;
    assert_eq!(response, expected);
    assert_eq!(
        read_response(&mut response.as_slice()).unwrap(),
        (0x0807_0605_0403_0201, Response::CacheRecording(policy))
    );
    for (offset, value) in [(20, 2), (21, 2), (24, 0x10)] {
        let mut invalid = response.clone();
        invalid[offset] = value;
        assert!(matches!(read_response(&mut invalid.as_slice()), Err(Error::InvalidMessage)));
    }
}

#[test]
fn recorder_statistics_use_all_eight_bytes_of_each_counter_in_order() {
    let counters = [
        0x0807_0605_0403_0201,
        0x1817_1615_1413_1211,
        0x2827_2625_2423_2221,
        0x3837_3635_3433_3231,
        0x4847_4645_4443_4241,
        0x5857_5655_5453_5251,
    ];
    let statistics = RecorderStatistics {
        thread_count: counters[0],
        total_events: counters[1],
        retained_events: counters[2],
        lost_events: counters[3],
        event_capacity_per_thread: counters[4],
        allocated_bytes: counters[5],
        recording: RecordingConfiguration::default(),
    };
    let mut bytes = Vec::new();
    write_response(&mut bytes, 17, &Response::RecorderStatistics(statistics)).unwrap();
    assert_eq!(&bytes[..20], b"SGMP\x08\0\x68\0\x11\0\0\0\0\0\0\0\x52\0\0\0");
    let expected = [
        1, 2, 3, 4, 5, 6, 7, 8, 17, 18, 19, 20, 21, 22, 23, 24, 33, 34, 35, 36, 37, 38, 39, 40, 49, 50, 51, 52, 53, 54, 55, 56, 65, 66, 67,
        68, 69, 70, 71, 72, 81, 82, 83, 84, 85, 86, 87, 88,
    ];
    assert_eq!(&bytes[20..68], expected);
    assert_eq!(
        read_response(&mut bytes.as_slice()).unwrap(),
        (17, Response::RecorderStatistics(statistics))
    );
    for end in 20..bytes.len() {
        let mut invalid = bytes[..end].to_vec();
        invalid[16..20].copy_from_slice(&u32::try_from(end - 20).unwrap().to_le_bytes());
        assert!(matches!(read_response(&mut invalid.as_slice()), Err(Error::InvalidMessage)));
    }
}
