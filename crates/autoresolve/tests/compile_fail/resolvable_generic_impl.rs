use autoresolve_macros::resolvable;

struct Foo<T> {
    value: T,
}

#[resolvable]
impl<T> Foo<T> {
    fn new() -> Self {
        unimplemented!()
    }
}

fn main() {}
