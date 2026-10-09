// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(target_os = "linux")]
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ffi::{OsStr, OsString};
use std::fmt::{self, Display};
#[cfg(target_os = "linux")]
use std::io;
use std::marker::PhantomData;
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt;
#[cfg(target_os = "linux")]
use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(target_os = "linux")]
use std::ptr;
#[cfg(target_os = "linux")]
use std::ptr::NonNull;
#[cfg(target_os = "linux")]
use std::slice;

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn zygote_rt_authenticate_prepared() -> c_int;
    fn zygote_rt_abort_prepared();
}

#[cfg(target_os = "linux")]
unsafe extern "C-unwind" {
    fn zygote_rt_run_prepared(
        context: *mut c_void,
        entry: Option<unsafe extern "C-unwind" fn(*mut c_void, c_int, *mut *mut c_char) -> c_int>,
    ) -> c_int;
}

/// State constructed once and inherited by every prepared launch.
#[derive(Debug)]
pub struct Prepared<T: ZygoteSafe> {
    state: PreparedState<T>,
}

#[derive(Debug)]
enum PreparedState<T> {
    Heap(T),
    #[cfg(target_os = "linux")]
    Protected(ProtectedState<T>),
}

#[cfg(target_os = "linux")]
struct ProtectedState<T> {
    state: NonNull<T>,
    mapping: NonNull<c_void>,
    mapping_len: usize,
}

#[cfg(target_os = "linux")]
impl<T> fmt::Debug for ProtectedState<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProtectedState")
            .field("mapping_len", &self.mapping_len)
            .finish_non_exhaustive()
    }
}

#[cfg(target_os = "linux")]
impl<T> ProtectedState<T> {
    fn new(state: T) -> io::Result<Self> {
        let page_size = unsafe {
            // SAFETY: sysconf receives a constant query and has no pointer arguments.
            libc::sysconf(libc::_SC_PAGESIZE)
        };
        let page_size =
            usize::try_from(page_size).map_err(|_invalid_page_size| io::Error::other("unable to determine the system page size"))?;
        let required = size_of::<T>()
            .max(1)
            .checked_add(align_of::<T>() - 1)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "prepared state is too large"))?;
        let mapping_len = required
            .checked_add(page_size - 1)
            .map(|length| length / page_size * page_size)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "prepared state is too large"))?;
        let mapping = unsafe {
            // SAFETY: anonymous private mmap ignores the descriptor and offset.
            libc::mmap(
                ptr::null_mut(),
                mapping_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if mapping == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let address = mapping as usize;
        let state_address = address
            .checked_add(align_of::<T>() - 1)
            .map(|value| value & !(align_of::<T>() - 1))
            .expect("mapping length calculation already included alignment padding");
        let state_pointer = state_address as *mut T;
        unsafe {
            // SAFETY: mmap returned suitably aligned writable storage large
            // enough for T, and this initializes it exactly once.
            state_pointer.write(state);
        }
        Ok(Self {
            state: NonNull::new(state_pointer).expect("successful mmap never returns null"),
            mapping: NonNull::new(mapping).expect("successful mmap never returns null"),
            mapping_len,
        })
    }

    fn freeze(self) -> io::Result<&'static T> {
        let result = unsafe {
            // SAFETY: mapping identifies this state's complete live mapping.
            libc::mprotect(self.mapping.as_ptr(), self.mapping_len, libc::PROT_READ)
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        let state = self.state;
        std::mem::forget(self);
        Ok(unsafe {
            // SAFETY: the mapping is intentionally retained for the process
            // lifetime and was made read-only before shared access begins.
            state.as_ref()
        })
    }
}

#[cfg(target_os = "linux")]
impl<T> Drop for ProtectedState<T> {
    fn drop(&mut self) {
        // SAFETY: before freeze this is the only owner of an initialized T.
        unsafe { ptr::drop_in_place(self.state.as_ptr()) };
        // SAFETY: this object exclusively owns the live anonymous mapping.
        unsafe { libc::munmap(self.mapping.as_ptr(), self.mapping_len) };
    }
}

impl<T: ZygoteSafe> Prepared<T> {
    /// Wraps state that satisfies the prepared-state safety contract.
    #[must_use]
    pub const fn new(state: T) -> Self {
        Self {
            state: PreparedState::Heap(state),
        }
    }

    /// Places the prepared root value in a dedicated page-aligned mapping.
    ///
    /// The mapping remains writable while preparation returns and becomes
    /// read-only immediately before direct execution or zygote launches begin.
    /// An accidental write to the root value therefore faults instead of
    /// silently creating copy-on-write pages.
    ///
    /// Protection is not recursive: allocations owned through `Box`, `Vec`,
    /// `String`, reference-counted pointers, or custom containers remain in
    /// their original mappings. Put immutable data inline in `T`, or arrange
    /// separately protected backing storage before constructing `T`.
    ///
    /// Prepared state is retained for the process lifetime in both ordinary
    /// and protected modes, so destructors do not run after successful
    /// preparation. If preparation is abandoned before execution, `T` is
    /// dropped and its mapping is released normally.
    ///
    /// # Errors
    ///
    /// Returns an error when the anonymous mapping cannot be allocated. The
    /// later read-only transition is also fallible and is reported by
    /// [`run_prepared`] as a preparation failure.
    #[cfg(target_os = "linux")]
    pub fn protected(state: T) -> io::Result<Self> {
        Ok(Self {
            state: PreparedState::Protected(ProtectedState::new(state)?),
        })
    }

    #[cfg(target_os = "linux")]
    fn leak(self) -> Result<&'static T, String> {
        match self.state {
            PreparedState::Heap(state) => Ok(Box::leak(Box::new(state))),
            PreparedState::Protected(state) => state.freeze().map_err(|error| error.to_string()),
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn leak(self) -> &'static T {
        let PreparedState::Heap(state) = self.state;
        Box::leak(Box::new(state))
    }
}

/// Marks state that can be inherited safely across a zygote fork.
///
/// # Safety
///
/// Implementors must contain no live threads, process-shared synchronization,
/// child-specific secrets, unregistered kernel resources, pointers outside
/// storage that remains valid for the process lifetime, or other state whose
/// duplication by `fork()` can violate memory safety. Application code receives
/// only shared access, so all interior mutability must also remain correct when
/// each child starts from the same snapshot.
pub unsafe trait ZygoteSafe: Sync + 'static {}

// SAFETY: unit contains no state or resources.
unsafe impl ZygoteSafe for () {}

/// Per-launch arguments supplied by the controller.
///
/// On Linux the arguments borrow the native launch arena and remain valid only
/// for the application callback. Use [`Launch::into_args_os`] when they must be
/// retained.
pub struct Launch<'a> {
    #[cfg(target_os = "linux")]
    arguments: &'a [*mut c_char],
    #[cfg(not(target_os = "linux"))]
    arguments: Vec<OsString>,
    lifetime: PhantomData<&'a ()>,
}

impl fmt::Debug for Launch<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.args_os()).finish()
    }
}

impl Clone for Launch<'_> {
    fn clone(&self) -> Self {
        Self {
            #[cfg(target_os = "linux")]
            arguments: self.arguments,
            #[cfg(not(target_os = "linux"))]
            arguments: self.arguments.clone(),
            lifetime: PhantomData,
        }
    }
}

impl Launch<'_> {
    /// Returns all arguments, including logical `argv[0]`.
    #[cfg(not(target_os = "linux"))]
    #[must_use]
    pub fn args_os(&self) -> impl ExactSizeIterator<Item = &OsStr> {
        self.arguments.iter().map(OsString::as_os_str)
    }

    /// Returns all arguments, including logical `argv[0]`, without copying
    /// storage owned by the native launch request.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn args_os(&self) -> impl ExactSizeIterator<Item = &OsStr> {
        self.arguments.iter().map(|argument| unsafe {
            // SAFETY: the native callback contract keeps every non-null,
            // NUL-terminated argument alive for the callback's duration.
            OsStr::from_bytes(CStr::from_ptr(*argument).to_bytes())
        })
    }

    /// Consumes the launch and copies all arguments into owned strings.
    #[must_use]
    pub fn into_args_os(self) -> Vec<OsString> {
        #[cfg(target_os = "linux")]
        {
            self.args_os().map(OsStr::to_owned).collect()
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.arguments
        }
    }
}

struct Context<T>
where
    T: ZygoteSafe,
{
    state: &'static T,
    application: for<'launch> fn(&'static T, Launch<'launch>) -> i32,
    marker: PhantomData<T>,
}

/// Runs an application with state prepared once before launching children.
///
/// Direct execution prepares the state and invokes the application once.
/// Controller execution retains the state in a single-threaded zygote and
/// invokes the application in each forked child.
/// The application is a function pointer and cannot capture state; place all
/// state needed by launched children in the returned [`Prepared`] value.
///
/// # Errors
///
/// Preparation failures are printed to standard error and returned as process
/// exit code 1. Runtime bootstrap failures return a nonzero process exit code.
pub fn run_prepared<T, P, E>(prepare: P, application: for<'launch> fn(&'static T, Launch<'launch>) -> i32) -> i32
where
    T: ZygoteSafe,
    P: FnOnce() -> Result<Prepared<T>, E>,
    E: Display,
{
    #[cfg(target_os = "linux")]
    // SAFETY: the native entry validates and consumes only process bootstrap
    // state established before main.
    let bootstrap = unsafe { zygote_rt_authenticate_prepared() };
    #[cfg(target_os = "linux")]
    if bootstrap < 0 {
        return 125;
    }

    let prepared = match prepare() {
        Ok(prepared) => prepared,
        Err(error) => {
            #[cfg(target_os = "linux")]
            if bootstrap == 1 {
                // SAFETY: authentication succeeded and this process owns the
                // retained bootstrap descriptor and nonce.
                unsafe {
                    zygote_rt_abort_prepared();
                }
            }
            eprintln!("zygote preparation failed: {error}");
            return 1;
        }
    };
    #[cfg(target_os = "linux")]
    let state = match prepared.leak() {
        Ok(state) => state,
        Err(error) => {
            if bootstrap == 1 {
                unsafe {
                    // SAFETY: authentication succeeded and this process owns
                    // the retained bootstrap descriptor and nonce.
                    zygote_rt_abort_prepared();
                }
            }
            eprintln!("zygote preparation failed: unable to protect prepared state: {error}");
            return 1;
        }
    };
    #[cfg(not(target_os = "linux"))]
    let state = prepared.leak();
    let context = Box::leak(Box::new(Context {
        state,
        application,
        marker: PhantomData,
    }));

    #[cfg(target_os = "linux")]
    {
        if bootstrap == 0 {
            return run_direct(context);
        }

        unsafe {
            // SAFETY: context is leaked for the process lifetime, and the
            // native runtime calls entry only in this process or its forked
            // children with a C-compatible argv.
            zygote_rt_run_prepared(ptr::from_mut(context).cast(), Some(prepared_entry::<T>))
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        run_direct(context)
    }
}

fn run_direct<T>(context: &Context<T>) -> i32
where
    T: ZygoteSafe,
{
    #[cfg(target_os = "linux")]
    {
        let arguments = std::env::args_os()
            .map(|argument| CString::new(argument.as_bytes()).expect("process arguments cannot contain embedded NUL bytes"))
            .collect::<Vec<_>>();
        let argument_pointers = arguments.iter().map(|argument| argument.as_ptr().cast_mut()).collect::<Vec<_>>();
        (context.application)(
            context.state,
            Launch {
                arguments: &argument_pointers,
                lifetime: PhantomData,
            },
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let launch = Launch {
            arguments: std::env::args_os().collect(),
            lifetime: PhantomData,
        };
        (context.application)(context.state, launch)
    }
}

#[cfg(target_os = "linux")]
/// Adapts the native launch arena to the prepared Rust callback.
///
/// # Safety
///
/// `context` must point to the leaked `Context<T>` created by
/// [`run_prepared`]. `argument_vector` must contain `argument_count` non-null,
/// NUL-terminated strings that remain live until the callback returns.
unsafe extern "C-unwind" fn prepared_entry<T>(context: *mut c_void, argument_count: c_int, argument_vector: *mut *mut c_char) -> c_int
where
    T: ZygoteSafe,
{
    if context.is_null() || argument_count < 0 || argument_vector.is_null() {
        return 125;
    }
    let Ok(argument_count) = usize::try_from(argument_count) else {
        return 125;
    };

    let invocation = catch_unwind(AssertUnwindSafe(|| {
        let context = unsafe {
            // SAFETY: validated non-null above and created as Context<T> by
            // run_prepared for this exact callback instantiation.
            &*context.cast::<Context<T>>()
        };
        let raw_arguments = unsafe {
            // SAFETY: the native entry contract supplies argc valid pointers.
            slice::from_raw_parts(argument_vector, argument_count)
        };
        for argument in raw_arguments {
            if argument.is_null() {
                return 125;
            }
            let _ = unsafe {
                // SAFETY: each argv entry is a NUL-terminated C string for the
                // duration of the callback.
                CStr::from_ptr(*argument)
            };
        }
        (context.application)(
            context.state,
            Launch {
                arguments: raw_arguments,
                lifetime: PhantomData,
            },
        )
    }));
    invocation.unwrap_or(101)
}

#[cfg(all(test, target_os = "linux"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![expect(
        clippy::panic,
        clippy::unwrap_used,
        reason = "tests intentionally exercise panic containment and fail directly"
    )]

    type Application = for<'launch> fn(&'static (), Launch<'launch>) -> i32;

    use super::*;

    #[expect(
        clippy::needless_pass_by_value,
        clippy::trivially_copy_pass_by_ref,
        reason = "signature must match the prepared application callback contract"
    )]
    fn returns_argument_count(_state: &'static (), launch: Launch<'_>) -> i32 {
        i32::try_from(launch.args_os().len()).unwrap()
    }

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "signature must match the prepared application callback contract"
    )]
    fn panics(_state: &'static (), _launch: Launch<'_>) -> i32 {
        panic!("prepared entry panic")
    }

    fn invoke_entry(context: *mut c_void, argument_count: c_int, argument_vector: *mut *mut c_char) -> c_int {
        unsafe {
            // SAFETY: each test supplies either deliberately invalid values
            // checked before dereference or a matching live Context and argv.
            prepared_entry::<()>(context, argument_count, argument_vector)
        }
    }

    #[test]
    fn prepared_entry_rejects_invalid_ffi_arguments() {
        let mut context = Context {
            state: &(),
            application: returns_argument_count as Application,
            marker: PhantomData,
        };
        let context = ptr::from_mut(&mut context).cast();

        assert_eq!(invoke_entry(ptr::null_mut(), 0, ptr::null_mut()), 125);
        assert_eq!(invoke_entry(context, 0, ptr::null_mut()), 125);
        assert_eq!(invoke_entry(context, -1, ptr::null_mut()), 125);
        assert_eq!(invoke_entry(context, 1, ptr::null_mut()), 125);

        let mut arguments = [ptr::null_mut()];
        assert_eq!(invoke_entry(context, 1, arguments.as_mut_ptr()), 125);
    }

    #[test]
    fn prepared_entry_passes_borrowed_arguments_and_contains_panics() {
        let first = CString::new("target").unwrap();
        let second = CString::new("argument").unwrap();
        let mut arguments = [first.as_ptr().cast_mut(), second.as_ptr().cast_mut()];
        let mut context = Context {
            state: &(),
            application: returns_argument_count as Application,
            marker: PhantomData,
        };

        let result = invoke_entry(
            ptr::from_mut(&mut context).cast(),
            i32::try_from(arguments.len()).unwrap(),
            arguments.as_mut_ptr(),
        );
        assert_eq!(result, 2);

        let mut panic_context = Context {
            state: &(),
            application: panics as Application,
            marker: PhantomData,
        };
        let result = invoke_entry(
            ptr::from_mut(&mut panic_context).cast(),
            i32::try_from(arguments.len()).unwrap(),
            arguments.as_mut_ptr(),
        );
        assert_eq!(result, 101);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn run_prepared_supports_direct_calls() {
        let expected = i32::try_from(std::env::args_os().count()).unwrap();

        assert_eq!(
            run_prepared(|| Ok::<_, &'static str>(Prepared::new(())), returns_argument_count),
            expected
        );
    }
}
