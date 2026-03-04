use autoresolve_macros::resolvable;

struct Foo;

#[resolvable]
impl Foo {
    fn build() -> Self {
        Self
    }
}

fn main() {}
