// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Static names avoid per-entry formatting and allocation in real-capacity Miri fixtures.

#![cfg(test)]

use std::str;

pub(crate) const CAPACITY: usize = (1 << 15) - (1 << 13);

static NAMES: [[u8; 6]; CAPACITY] = {
    let mut names = [*b"x-aaaa"; CAPACITY];
    let mut index = 0;
    while index < CAPACITY {
        let mut value = index;
        let mut position = 2;
        while position < 6 {
            names[index][position] = b"abcdefghijklmnopqrstuvwxyz"[value % 26];
            value /= 26;
            position += 1;
        }
        index += 1;
    }
    names
};

pub(crate) fn name(index: usize) -> &'static str {
    str::from_utf8(&NAMES[index]).unwrap()
}
