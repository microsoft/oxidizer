// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared workload for isolated response-head compile and section controls.
// Each target defines `insert_headers` before including this file.

use http::Response;
use routerama::response::Body;

const HEADER_FIELDS: [(&str, &str); 16] = [
    ("x-template-00", "value-00"),
    ("x-template-01", "value-01"),
    ("x-template-02", "value-02"),
    ("x-template-03", "value-03"),
    ("x-template-04", "value-04"),
    ("x-template-05", "value-05"),
    ("x-template-06", "value-06"),
    ("x-template-07", "value-07"),
    ("x-template-08", "value-08"),
    ("x-template-09", "value-09"),
    ("x-template-10", "value-10"),
    ("x-template-11", "value-11"),
    ("x-template-12", "value-12"),
    ("x-template-13", "value-13"),
    ("x-template-14", "value-14"),
    ("x-template-15", "value-15"),
];

#[derive(Clone, Copy)]
enum Scenario {
    Headers0,
    Headers1,
    Headers4,
    Headers16,
}

impl Scenario {
    const ALL: [Self; 4] = [Self::Headers0, Self::Headers1, Self::Headers4, Self::Headers16];

    const fn count(self) -> usize {
        match self {
            Self::Headers0 => 0,
            Self::Headers1 => 1,
            Self::Headers4 => 4,
            Self::Headers16 => 16,
        }
    }
}

fn observe(response: &Response<Body>) -> (usize, u64) {
    let checksum = response.headers().iter().fold(0xcbf2_9ce4_8422_2325_u64, |mut hash, (name, value)| {
        for byte in name.as_str().bytes().chain(value.as_bytes().iter().copied()) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    });
    (response.headers().len(), checksum)
}

fn main() {
    for scenario in Scenario::ALL {
        let mut response = Response::new(Body::empty());
        insert_headers(response.headers_mut(), scenario);
        assert_eq!(response.headers().len(), scenario.count());
        std::hint::black_box(observe(&response));
    }
}
