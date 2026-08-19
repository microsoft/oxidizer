// Temporary review probe - not part of the PR.
// Claim: the `generics_lifetime_and_const_params_untouched` snapshot pins an
// expansion that does not compile.
use thread_aware_macros::ThreadAware;

#[derive(ThreadAware)]
struct Mixed<'a, const N: usize, T>(&'a T, [u8; N], core::marker::PhantomData<T>);

#[test]
fn probe() {}
