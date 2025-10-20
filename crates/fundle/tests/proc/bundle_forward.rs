// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

struct Bar {
    x: u8,
    y: u16,
}

impl AsRef<u8> for Bar {
    fn as_ref(&self) -> &u8 { &self.x }
}

impl AsRef<u16> for Bar {
    fn as_ref(&self) -> &u16 { &self.y }
}

#[fundle::bundle]
struct Foo {
    #[forward(u8, u16)]
    x: Bar
}

fn main() {}
