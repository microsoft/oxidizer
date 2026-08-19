// Temporary review probe - not part of the PR.
// Claim: a bare local trait named `ThreadAware` suppresses the generated bound.
use thread_aware_macros::ThreadAware;

// A user's own, unrelated trait, in scope under the bare name `ThreadAware`.
trait ThreadAware {}

struct Inner;
impl ThreadAware for Inner {}
impl thread_aware::ThreadAware for Inner {
    fn relocate(&mut self, _s: Option<thread_aware::affinity::Affinity>, _d: thread_aware::affinity::Affinity) {}
}

#[derive(ThreadAware)]
struct Foo<T: ThreadAware>(T);

#[test]
fn probe() {}
