#[derive(Clone)]
pub struct Clock;

impl Clock {
    pub(crate) fn number(&self) -> i32 {
        42
    }
}
