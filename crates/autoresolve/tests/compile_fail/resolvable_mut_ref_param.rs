use autoresolve_macros::resolvable;

#[derive(Clone)]
struct Bar;

struct Foo {
    bar: Bar,
}

#[resolvable]
impl Foo {
    fn new(bar: &mut Bar) -> Self {
        Self { bar: bar.clone() }
    }
}

fn main() {}
