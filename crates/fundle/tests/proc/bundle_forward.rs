
struct Bar {
    x: u8,
}

impl AsRef<u8> for Bar {
    fn as_ref(&self) -> &u8 { &self.x }
}

#[fundle::bundle]
struct Foo {
    #[forward(u8)]
    x: Bar
}

fn main() {}
