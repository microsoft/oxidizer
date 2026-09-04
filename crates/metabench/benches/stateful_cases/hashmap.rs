// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashMap;

use metabench::{BenchmarkCase, Fixture};

#[derive(Clone, Copy)]
pub(crate) struct HashMapCase {
    capacity: usize,
    value_length: usize,
}

impl HashMapCase {
    const ALL: [Self; 3] = [
        Self {
            capacity: 0,
            value_length: 16,
        },
        Self {
            capacity: 1_000,
            value_length: 16,
        },
        Self {
            capacity: 1_000,
            value_length: 1_024,
        },
    ];
}

impl BenchmarkCase for HashMapCase {
    fn name(&self) -> String {
        format!("capacity={},value_length={}", self.capacity, self.value_length)
    }
}

pub(crate) struct HashMapBenchmarks {
    map: HashMap<String, String>,
    key: Option<String>,
    value: Option<String>,
}

impl Fixture for HashMapBenchmarks {
    type Case = HashMapCase;

    fn cases() -> impl IntoIterator<Item = Self::Case> {
        HashMapCase::ALL
    }

    fn setup(case: &Self::Case) -> Self {
        Self {
            map: HashMap::with_capacity(case.capacity),
            key: Some(String::from("message")),
            value: Some("x".repeat(case.value_length)),
        }
    }
}

impl Drop for HashMapBenchmarks {
    fn drop(&mut self) {
        self.map.clear();
    }
}

#[metabench::benchmarks]
impl HashMapBenchmarks {
    #[metabench::benchmark]
    fn insert_string(&mut self) {
        let key = self.key.take().expect("setup initializes every fixture with a key");
        let value = self.value.take().expect("setup initializes every fixture with a value");
        self.map.insert(key, value);
    }
}
