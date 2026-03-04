use autoresolve_macros::resolvable;

#[derive(Clone)]
struct Bar;

struct Foo {
    bar: Bar,
}

#[resolvable]
impl Foo {
    fn new(bar: Bar) -> Self {
        Self { bar }
    }
}

fn main() {}
