use autoresolve_macros::resolvable;

use super::telemetry::Telemetry;

#[derive(Clone)]
pub struct SdkProvider {
    telemetry: Telemetry,
}

#[resolvable]
impl SdkProvider {
    fn new(telemetry: &Telemetry) -> Self {
        Self {
            telemetry: telemetry.clone(),
        }
    }
}
