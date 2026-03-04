/// Creates a [`Resolver`] with multiple base types.
///
/// Each argument is `name: Type` where `name` is a variable in scope of type `Type`. These become
/// root types in the dependency hierarchy — they are pre-inserted into the resolver and available
/// as dependencies for any type marked with `#[resolvable]`.
///
/// # Example
///
/// ```ignore
/// let builtins = Builtins::new();
/// let telemetry = Telemetry::new();
///
/// let mut resolver = autoresolve::resolver!(
///     builtins: Builtins,
///     telemetry: Telemetry,
/// );
///
/// let client = resolver.get::<Client>();
/// ```
#[macro_export]
macro_rules! resolver {
    ( $( $name:ident : $ty:ty ),+ $(,)? ) => {
        {
            struct __AutoresolveBase;

            // SAFETY: ZST used only as a type parameter; never instantiated or shared.
            unsafe impl Send for __AutoresolveBase {}
            // SAFETY: ZST used only as a type parameter; never instantiated or shared.
            unsafe impl Sync for __AutoresolveBase {}

            $(
                impl $crate::ResolveFrom<__AutoresolveBase> for $ty {
                    type Inputs = $crate::ResolutionDepsEnd;

                    fn new(_: $crate::ResolutionDepsEnd) -> Self {
                        unreachable!("base types are pre-inserted into the resolver")
                    }
                }
            )+

            let mut r = $crate::Resolver::<__AutoresolveBase>::new_empty();
            $(
                r.insert($name);
            )+
            r
        }
    };
}
