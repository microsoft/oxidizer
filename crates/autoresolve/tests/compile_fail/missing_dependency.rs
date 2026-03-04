use autoresolve_macros::resolvable;

#[derive(Clone)]
struct Builtins;

#[derive(Clone)]
struct Telemetry;

#[derive(Clone)]
struct Client {
    builtins: Builtins,
    telemetry: Telemetry,
}

#[resolvable]
impl Client {
    fn new(builtins: &Builtins, telemetry: &Telemetry) -> Self {
        Self {
            builtins: builtins.clone(),
            telemetry: telemetry.clone(),
        }
    }
}

fn main() {
    // Only provide Builtins, not Telemetry — this should fail to compile because
    // Telemetry is not resolvable from the base type.
    let builtins = Builtins;
    let mut resolver = autoresolve::resolver!(Base, builtins: Builtins);
    let _client = resolver.get::<Client>();
}
