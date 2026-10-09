// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Linux entry shim that executes before Rust's `lang_start`.
//!
//! This module deliberately uses only `core`, raw libc calls, and bounded
//! `calloc` storage before handing control to `__real_main`. It must not use
//! `std`, Rust allocation, thread-local state, formatting, or panicking paths.

#![cfg(target_os = "linux")]
#![expect(
    unsafe_op_in_unsafe_fn,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_ptr_alignment,
    clippy::similar_names,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "pre-lang_start shim mirrors checked C ABI and syscall arithmetic"
)]

use core::ffi::{CStr, c_char, c_int, c_long, c_uint, c_void};
use core::mem::zeroed;
#[cfg(test)]
use core::ptr::NonNull;
use core::ptr::{addr_of, addr_of_mut, null, null_mut};
#[cfg(test)]
use core::sync::atomic::{AtomicU8, Ordering};

use crate::protocol::{CONTROL_FD, MAX_FD_COUNT, MAX_ITEM_COUNT, MAX_PACKET_LEN, SPECIALIZATION_TIMEOUT_MILLIS};

const CONTROL_FD_ENV: &CStr = c"ZYGOTE_RT_CONTROL_FD";
const NONCE_ENV: &CStr = c"ZYGOTE_RT_NONCE";
const PREFORK_MIN_IDLE_ENV: &CStr = c"ZYGOTE_RT_PREFORK_MIN_IDLE";
const PREFORK_MAX_IDLE_ENV: &CStr = c"ZYGOTE_RT_PREFORK_MAX_IDLE";
const PREFORK_REFILL_THRESHOLD_ENV: &CStr = c"ZYGOTE_RT_PREFORK_REFILL_THRESHOLD";
const PREFORK_REFILL_DELAY_ENV: &CStr = c"ZYGOTE_RT_PREFORK_REFILL_DELAY_MS";
const FEATURE_BASE: u64 = 1;
const FEATURE_LINUX_SANDBOX: u64 = 2;
const CHILD_TABLE_SIZE: usize = MAX_ITEM_COUNT;
const FD_FALLBACK_LIMIT: c_long = 1_048_576;
const ROLE_STDIN: i32 = 1;
const ROLE_STDOUT: i32 = 2;
const ROLE_STDERR: i32 = 3;
const ROLE_PIDFD: i32 = 4;
const ROLE_CGROUP_PROCS: i32 = 5;
const ROLE_LANDLOCK_RULESET: i32 = 6;
const ROLE_NAMESPACE_CGROUP: i32 = 7;
const ROLE_NAMESPACE_IPC: i32 = 8;
const ROLE_NAMESPACE_UTS: i32 = 9;
const ROLE_NAMESPACE_NETWORK: i32 = 10;
const ROLE_NAMESPACE_TIME: i32 = 11;
const ROLE_NAMESPACE_MOUNT: i32 = 12;
const ROLE_CLOSED_STDIN: i32 = 13;
const ROLE_CLOSED_STDOUT: i32 = 14;
const ROLE_CLOSED_STDERR: i32 = 15;
const BODY_READY: u32 = 10;
const BODY_LAUNCH: u32 = 11;
const BODY_STARTED: u32 = 12;
const BODY_EXITED: u32 = 13;
const BODY_ERROR: u32 = 14;
const BODY_SHUTDOWN: u32 = 15;
const BODY_POOL_CONTROL: u32 = 16;
const BODY_POOL_STATE: u32 = 17;
const MAX_PREFORK_WORKERS: usize = 64;
const SIGNAL_COUNT: usize = 65;
const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
const SECCOMP_SET_MODE_FILTER: c_uint = 1;
#[cfg(feature = "private-test-util")]
const SPECIALIZATION_STALL_ENV: &[u8] = b"ZYGOTE_RT_TEST_STALL_SPECIALIZATION=";
#[cfg(feature = "private-test-util")]
const TEST_SPECIALIZATION_TIMEOUT_MILLIS: c_int = 500;
#[cfg(feature = "private-test-util")]
const TEST_SPECIALIZATION_STALL_MILLIS: c_int = 1_500;

type Entry = unsafe extern "C-unwind" fn(*mut c_void, c_int, *mut *mut c_char) -> c_int;

#[used]
#[unsafe(link_section = "zygote_rt_mode")]
static ZYGOTE_RT_MODE_DEFAULT: u8 = 0;

// FFI contract: libc and the linker supply these symbols, whose storage remains
// live throughout process startup.
unsafe extern "C" {
    static mut environ: *mut *mut c_char;
    static __start_zygote_rt_mode: u8;
    static __stop_zygote_rt_mode: u8;
    fn __real_main(argc: c_int, argv: *mut *mut c_char) -> c_int;
    #[cfg(not(miri))]
    fn explicit_bzero(value: *mut c_void, length: usize);
}

static mut PREPARED_ARGC: c_int = 0;
static mut PREPARED_ARGV: *mut *mut c_char = null_mut();
static mut PREPARED_CONTROL_FD: c_int = -1;
static mut PREPARED_NONCE: [c_char; 33] = [0; 33];
static mut RESET_CHILD_SIGNALS: bool = false;
static mut SIGNALS_TO_RESET: [u8; SIGNAL_COUNT] = [0; SIGNAL_COUNT];
static mut PREFORK_CONFIG: PreforkConfig = PreforkConfig::disabled();

/// Native launch-decoder state exposed only to the benchmark package.
#[cfg(feature = "private-test-util")]
#[doc(hidden)]
#[derive(Debug)]
pub struct NativeLaunchDecoder {
    arena: *mut c_void,
    capacity: usize,
    allocations: u64,
}

/// One native launch-decoder observation.
#[cfg(feature = "private-test-util")]
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeDecodeObservation {
    /// Bytes in the validated wire packet.
    pub packet_bytes: usize,
    /// Arena bytes initialized and securely erased for this request.
    pub arena_bytes: usize,
    /// Number of arena allocation or growth operations since construction.
    pub allocations: u64,
}

#[cfg(feature = "private-test-util")]
impl NativeLaunchDecoder {
    /// Creates reusable native decoder state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            arena: null_mut(),
            capacity: 0,
            allocations: 0,
        }
    }

    /// Decodes, validates, and erases one launch packet.
    pub fn decode(&mut self, bytes: &[u8]) -> Option<NativeDecodeObservation> {
        let mut packet = Packet::empty();
        let mut launch = Launch::empty();
        let previous_capacity = self.capacity;
        // SAFETY: bytes remains live through packet decoding.
        let packet_decoded = unsafe { decode_packet(bytes.as_ptr(), bytes.len(), addr_of_mut!(packet)) };
        // SAFETY: packet points into bytes, which remains live, and this object
        // exclusively owns its reusable arena.
        let launch_decoded = packet_decoded
            && packet.kind == BODY_LAUNCH
            && unsafe { parse_launch(addr_of!(packet), &mut launch, &mut self.arena, &mut self.capacity) };
        let decoded = launch_decoded;
        if !decoded {
            unsafe {
                // SAFETY: launch is initialized even when parsing fails.
                free_launch(addr_of_mut!(launch));
            }
            return None;
        }
        if self.capacity > previous_capacity {
            self.allocations = self.allocations.saturating_add(1);
        }
        let observation = NativeDecodeObservation {
            packet_bytes: bytes.len(),
            arena_bytes: launch.arena_size,
            allocations: self.allocations,
        };
        unsafe {
            // SAFETY: launch owns the initialized view into this decoder's arena.
            free_launch(addr_of_mut!(launch));
        }
        Some(observation)
    }
}

#[cfg(feature = "private-test-util")]
impl Default for NativeLaunchDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "private-test-util")]
impl Drop for NativeLaunchDecoder {
    fn drop(&mut self) {
        if !self.arena.is_null() {
            // SAFETY: this object exclusively owns the retained arena.
            unsafe { secure_zero(self.arena, self.capacity) };
            // SAFETY: the arena was allocated by libc and has not been freed.
            unsafe { libc::free(self.arena) };
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
#[cfg_attr(not(test), expect(dead_code, reason = "fault identifiers support compile-only failure-path tests"))]
enum Fault {
    None,
    Parse,
    ThreadCheck,
    AckPipe,
    Fork,
    ChildTable,
    Pidfd,
    ChildSetup,
    Seccomp,
    AckRead,
    TemplateState,
    SignalSetup,
    Allocation,
    ReadySend,
}

#[cfg(test)]
static TEST_FAULT: AtomicU8 = AtomicU8::new(Fault::None as u8);

#[inline]
/// Returns whether the named test fault is active.
///
/// # Safety
///
/// The caller must serialize access to the test-only mutable fault selector.
unsafe fn fault(point: Fault) -> bool {
    #[cfg(test)]
    let active = TEST_FAULT.load(Ordering::Relaxed) == point as u8;
    #[cfg(not(test))]
    let active = {
        let _ = point;
        false
    };
    active
}

#[cfg(test)]
fn set_test_fault(point: Fault) {
    TEST_FAULT.store(point as u8, Ordering::Relaxed);
}

#[repr(C)]
struct Packet {
    kind: u32,
    request_id: u64,
    body: *const u8,
    body_len: usize,
    roles: [i32; MAX_FD_COUNT],
    role_count: usize,
    fds: [c_int; MAX_FD_COUNT],
    fd_count: usize,
    sandbox_feature: bool,
    launch_shape: Shape,
    launch_validated: bool,
}

impl Packet {
    const fn empty() -> Self {
        Self {
            kind: 0,
            request_id: 0,
            body: null(),
            body_len: 0,
            roles: [0; MAX_FD_COUNT],
            role_count: 0,
            fds: [-1; MAX_FD_COUNT],
            fd_count: 0,
            sandbox_feature: false,
            launch_shape: Shape::empty(),
            launch_validated: false,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Limit {
    resource: c_int,
    value: libc::rlimit,
}

#[repr(C)]
struct Launch {
    argc: c_int,
    argv: *mut *mut c_char,
    envp: *mut *mut c_char,
    cwd: *mut c_char,
    uid: libc::uid_t,
    gid: libc::gid_t,
    groups: *mut libc::gid_t,
    group_count: usize,
    process_group: libc::pid_t,
    umask_value: libc::mode_t,
    limits: *mut Limit,
    limit_count: usize,
    has_uid: bool,
    has_gid: bool,
    has_groups: bool,
    has_process_group: bool,
    new_session: bool,
    has_umask: bool,
    arena: *mut c_void,
    arena_size: usize,
    arena_capacity: usize,
    no_new_privs: bool,
    disable_dumping: bool,
    clear_capabilities: bool,
    drop_capability_bounding_set: bool,
    lock_securebits: bool,
    seccomp: *mut libc::sock_filter,
    seccomp_count: usize,
    seccomp_architecture: u32,
    landlock: bool,
    landlock_abi: u32,
    landlock_handled_access: u64,
    cgroup: bool,
    namespace_count: usize,
}

impl Launch {
    const fn empty() -> Self {
        Self {
            argc: 0,
            argv: null_mut(),
            envp: null_mut(),
            cwd: null_mut(),
            uid: 0,
            gid: 0,
            groups: null_mut(),
            group_count: 0,
            process_group: 0,
            umask_value: 0,
            limits: null_mut(),
            limit_count: 0,
            has_uid: false,
            has_gid: false,
            has_groups: false,
            has_process_group: false,
            new_session: false,
            has_umask: false,
            arena: null_mut(),
            arena_size: 0,
            arena_capacity: 0,
            no_new_privs: false,
            disable_dumping: false,
            clear_capabilities: false,
            drop_capability_bounding_set: false,
            lock_securebits: false,
            seccomp: null_mut(),
            seccomp_count: 0,
            seccomp_architecture: 0,
            landlock: false,
            landlock_abi: 0,
            landlock_handled_access: 0,
            cgroup: false,
            namespace_count: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct Shape {
    argc: usize,
    envc: usize,
    group_count: usize,
    limit_count: usize,
    string_bytes: usize,
    seccomp_count: usize,
}

#[derive(Clone, Copy)]
struct EnvironmentName {
    bytes: *const u8,
    len: usize,
}

struct SingleThreaded {
    owner_tid: libc::pid_t,
    #[cfg(test)]
    bypass_process_check: bool,
}

impl SingleThreaded {
    /// Establishes the invariant once before the launch loop begins.
    ///
    /// # Safety
    ///
    /// `/proc/self/task` must be accessible and no thread may be created after
    /// this proof is returned.
    unsafe fn establish() -> Option<Self> {
        process_is_single_threaded().then(|| Self {
            owner_tid: libc::syscall(libc::SYS_gettid) as libc::pid_t,
            #[cfg(test)]
            bypass_process_check: false,
        })
    }

    #[cfg(test)]
    fn for_fault_harness() -> Self {
        // SAFETY: gettid has no pointer arguments.
        let owner_tid = unsafe { libc::syscall(libc::SYS_gettid) as libc::pid_t };
        Self {
            owner_tid,
            bypass_process_check: true,
        }
    }

    /// Confirms that launch handling remains on the thread that established
    /// the invariant and that no additional thread is live.
    ///
    /// # Safety
    ///
    /// The proof must have been created by [`Self::establish`] in this process.
    unsafe fn is_current(&self) -> bool {
        if libc::syscall(libc::SYS_gettid) != c_long::from(self.owner_tid) {
            return false;
        }
        #[cfg(test)]
        if self.bypass_process_check {
            return true;
        }
        process_is_single_threaded()
    }
}

impl Shape {
    const fn empty() -> Self {
        Self {
            argc: 0,
            envc: 0,
            group_count: 0,
            limit_count: 0,
            string_bytes: 0,
            seccomp_count: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Child {
    pid: libc::pid_t,
    request_id: u64,
}

#[derive(Clone, Copy)]
struct PreforkConfig {
    min_idle: usize,
    max_idle: usize,
    refill_threshold: usize,
    refill_delay_millis: c_int,
}

impl PreforkConfig {
    const fn disabled() -> Self {
        Self {
            min_idle: 0,
            max_idle: 0,
            refill_threshold: 0,
            refill_delay_millis: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct IdleWorker {
    pid: libc::pid_t,
    socket: c_int,
}

impl IdleWorker {
    const fn empty() -> Self {
        Self { pid: 0, socket: -1 }
    }
}

struct PreforkPool {
    workers: [IdleWorker; MAX_PREFORK_WORKERS],
    count: usize,
    config: PreforkConfig,
    maximum_capacity: usize,
    refill_deadline_millis: i64,
    refills: u64,
    refill_nanoseconds: u64,
}

impl PreforkPool {
    const fn new(config: PreforkConfig) -> Self {
        Self {
            workers: [IdleWorker::empty(); MAX_PREFORK_WORKERS],
            count: 0,
            config,
            maximum_capacity: config.max_idle,
            refill_deadline_millis: -1,
            refills: 0,
            refill_nanoseconds: 0,
        }
    }
}

enum ReapResult {
    Continue,
    Detached,
    Failed,
}

struct Cursor {
    cursor: *const u8,
    end: *const u8,
}

impl Cursor {
    /// Creates a cursor over one contiguous packet region.
    ///
    /// # Safety
    ///
    /// `bytes` must be readable for `len` bytes, or null only when `len == 0`,
    /// and that allocation must outlive the returned cursor.
    unsafe fn new(bytes: *const u8, len: usize) -> Option<Self> {
        if bytes.is_null() {
            return (len == 0).then_some(Self { cursor: bytes, end: bytes });
        }
        Some(Self {
            cursor: bytes,
            end: bytes.add(len),
        })
    }

    /// Returns the number of unread bytes.
    ///
    /// # Safety
    ///
    /// Both cursor pointers must remain within the same live allocation and
    /// `cursor` must not have advanced beyond `end`.
    unsafe fn remaining(&self) -> usize {
        if self.cursor == self.end {
            return 0;
        }
        self.end.offset_from(self.cursor) as usize
    }

    /// Reads one Protobuf varint.
    ///
    /// # Safety
    ///
    /// This cursor must satisfy the invariants established by [`Self::new`].
    unsafe fn varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        let mut shift = 0;
        while shift < 70 {
            if self.cursor == self.end {
                return None;
            }
            let byte = *self.cursor;
            self.cursor = self.cursor.add(1);
            if shift == 63 && byte > 1 {
                return None;
            }
            value |= u64::from(byte & 0x7f).wrapping_shl(shift);
            if byte & 0x80 == 0 {
                return Some(value);
            }
            shift = shift.wrapping_add(7);
        }
        None
    }

    /// Reads one Protobuf field key.
    ///
    /// # Safety
    ///
    /// This cursor must satisfy the invariants established by [`Self::new`].
    unsafe fn key(&mut self) -> Option<(u32, u8)> {
        let key = self.varint()?;
        let field = u32::try_from(key.wrapping_shr(3)).ok()?;
        (field != 0).then_some((field, (key & 7) as u8))
    }

    /// Reads one length-delimited Protobuf value.
    ///
    /// # Safety
    ///
    /// This cursor must satisfy the invariants established by [`Self::new`].
    unsafe fn bytes(&mut self) -> Option<(*const u8, usize)> {
        let len = usize::try_from(self.varint()?).ok()?;
        if len > self.remaining() {
            return None;
        }
        let value = self.cursor;
        self.cursor = self.cursor.add(len);
        Some((value, len))
    }

    /// Skips one Protobuf value of the supplied wire type.
    ///
    /// # Safety
    ///
    /// This cursor must satisfy the invariants established by [`Self::new`].
    unsafe fn skip(&mut self, wire: u8) -> bool {
        match wire {
            0 => self.varint().is_some(),
            1 if self.remaining() >= 8 => {
                self.cursor = self.cursor.add(8);
                true
            }
            2 => self.bytes().is_some(),
            5 if self.remaining() >= 4 => {
                self.cursor = self.cursor.add(4);
                true
            }
            _ => false,
        }
    }
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes.
unsafe fn contains_byte(mut bytes: *const u8, len: usize, needle: u8) -> bool {
    let end = bytes.add(len);
    while bytes != end {
        if *bytes == needle {
            return true;
        }
        bytes = bytes.add(1);
    }
    false
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes and `packet` must be writable.
/// On success the packet borrows from `bytes`, which must remain live.
unsafe fn decode_packet(bytes: *const u8, len: usize, packet: *mut Packet) -> bool {
    if len > MAX_PACKET_LEN {
        return false;
    }
    *packet = Packet::empty();
    let Some(mut cursor) = Cursor::new(bytes, len) else {
        return false;
    };
    let mut seen = 0u32;
    let mut feature_seen = false;
    let mut sandbox_feature_seen = false;
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        match field {
            1 | 2 => {
                let bit = 1u32.wrapping_shl(field);
                if seen & bit != 0 || wire != 0 {
                    return false;
                }
                seen |= bit;
                let Some(value) = cursor.varint() else {
                    return false;
                };
                if field == 1 {
                    if value != 1 {
                        return false;
                    }
                } else {
                    (*packet).request_id = value;
                }
            }
            3 => {
                if wire != 0 {
                    return false;
                }
                let Some(feature) = cursor.varint() else {
                    return false;
                };
                if feature == FEATURE_BASE {
                    if feature_seen {
                        return false;
                    }
                    feature_seen = true;
                } else if feature == FEATURE_LINUX_SANDBOX {
                    if !feature_seen || sandbox_feature_seen {
                        return false;
                    }
                    sandbox_feature_seen = true;
                    (*packet).sandbox_feature = true;
                } else {
                    return false;
                }
            }
            4 => {
                if wire != 0 || (*packet).role_count == MAX_FD_COUNT {
                    return false;
                }
                let Some(role) = cursor.varint() else {
                    return false;
                };
                if !(1..=ROLE_CLOSED_STDERR as u64).contains(&role) {
                    return false;
                }
                *(*packet).roles.as_mut_ptr().add((*packet).role_count) = role as i32;
                (*packet).role_count = (*packet).role_count.wrapping_add(1);
            }
            BODY_READY..=BODY_POOL_STATE => {
                if (*packet).kind != 0 || wire != 2 {
                    return false;
                }
                let Some((body, body_len)) = cursor.bytes() else {
                    return false;
                };
                (*packet).kind = field;
                (*packet).body = body;
                (*packet).body_len = body_len;
            }
            _ if !cursor.skip(wire) => return false,
            _ => {}
        }
    }
    if seen & 2 == 0 || !feature_seen || (*packet).kind == 0 {
        return false;
    }
    if (*packet).sandbox_feature && (*packet).kind != BODY_LAUNCH {
        return false;
    }
    match (*packet).kind {
        BODY_READY | BODY_POOL_CONTROL | BODY_POOL_STATE => (*packet).request_id == 0 && (*packet).role_count == 0,
        BODY_LAUNCH => {
            let envelope_valid = (*packet).request_id != 0
                && (*packet).role_count >= 3
                && matches!((*packet).roles[0], ROLE_STDIN | ROLE_CLOSED_STDIN)
                && matches!((*packet).roles[1], ROLE_STDOUT | ROLE_CLOSED_STDOUT)
                && matches!((*packet).roles[2], ROLE_STDERR | ROLE_CLOSED_STDERR)
                && ((*packet).sandbox_feature || (*packet).role_count == 3);
            if !envelope_valid {
                return false;
            }
            let mut shape = Shape::empty();
            if !scan_launch(packet, &mut shape) {
                return false;
            }
            (*packet).launch_shape = shape;
            (*packet).launch_validated = true;
            true
        }
        BODY_STARTED => (*packet).request_id != 0 && (*packet).role_count == 1 && (*packet).roles[0] == ROLE_PIDFD,
        BODY_EXITED => (*packet).request_id != 0 && (*packet).role_count == 0,
        BODY_ERROR => (*packet).role_count == 0,
        BODY_SHUTDOWN => (*packet).request_id == 0 && (*packet).role_count == 0 && (*packet).body_len == 0,
        _ => false,
    }
}

/// # Safety
///
/// `packet` must point to an initialized packet whose nonnegative descriptors
/// are owned by the caller. This consumes those descriptor entries.
unsafe fn close_packet_fds(packet: *mut Packet) {
    let mut index = 0;
    while index < (*packet).fd_count {
        let descriptor = *(*packet).fds.as_ptr().add(index);
        if descriptor >= 0 {
            libc::close(descriptor);
            *(*packet).fds.as_mut_ptr().add(index) = -1;
        }
        index = index.wrapping_add(1);
    }
}

/// # Safety
///
/// `fd` must be a live sequenced-packet socket, `buffer` must be writable for
/// `MAX_PACKET_LEN` bytes, and `packet` must be writable. Received descriptors
/// become owned by `packet` and must later be closed exactly once.
unsafe fn receive_packet(fd: c_int, buffer: *mut u8, packet: *mut Packet) -> c_int {
    let mut vector = libc::iovec {
        iov_base: buffer.cast(),
        iov_len: MAX_PACKET_LEN,
    };
    let mut control = [0usize; 64];
    let mut message: libc::msghdr = zeroed();
    message.msg_iov = addr_of_mut!(vector);
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    let control_capacity = size_of_val(&control);
    message.msg_controllen = control_capacity;
    let received = loop {
        let value = libc::recvmsg(fd, addr_of_mut!(message), libc::MSG_CMSG_CLOEXEC);
        if value < 0 && errno() == libc::EINTR {
            continue;
        }
        break value;
    };
    if received < 0 {
        return received as c_int;
    }
    let mut received_fds = [-1; MAX_FD_COUNT];
    let mut received_fd_count = 0usize;
    let mut ancillary_valid = message.msg_controllen <= control_capacity;
    let control_start = message.msg_control as usize;
    let Some(control_end) = control_start.checked_add(message.msg_controllen) else {
        return close_received_fds(&received_fds, received_fd_count);
    };
    let mut header = if ancillary_valid {
        libc::CMSG_FIRSTHDR(addr_of!(message))
    } else {
        null_mut()
    };
    while ancillary_valid && !header.is_null() {
        let header_start = header as usize;
        let Some(header_end) = header_start.checked_add(size_of::<libc::cmsghdr>()) else {
            ancillary_valid = false;
            break;
        };
        if header_start < control_start || header_end > control_end {
            ancillary_valid = false;
            break;
        }
        let mut control_header = zeroed::<libc::cmsghdr>();
        libc::memcpy(addr_of_mut!(control_header).cast(), header.cast(), size_of::<libc::cmsghdr>());
        let control_message_len = control_header.cmsg_len;
        let base = libc::CMSG_LEN(0) as usize;
        let Some(message_end) = header_start.checked_add(control_message_len) else {
            ancillary_valid = false;
            break;
        };
        if control_message_len < base || message_end > control_end {
            ancillary_valid = false;
            break;
        }
        if control_header.cmsg_level == libc::SOL_SOCKET && control_header.cmsg_type == libc::SCM_RIGHTS {
            let bytes = control_message_len - base;
            if bytes & (size_of::<c_int>().wrapping_sub(1)) != 0 {
                ancillary_valid = false;
                break;
            }
            let count = bytes.wrapping_shr(size_of::<c_int>().trailing_zeros());
            let descriptor_bytes = libc::CMSG_DATA(header);
            let data_start = descriptor_bytes as usize;
            let Some(data_end) = data_start.checked_add(bytes) else {
                ancillary_valid = false;
                break;
            };
            if data_start < header_start || data_end > message_end || data_end > control_end {
                ancillary_valid = false;
                break;
            }
            if count > MAX_FD_COUNT.wrapping_sub(received_fd_count) {
                let mut current = 0;
                while current < count {
                    libc::close(*descriptor_bytes.cast::<c_int>().add(current));
                    current = current.wrapping_add(1);
                }
                ancillary_valid = false;
                break;
            }
            libc::memcpy(
                received_fds.as_mut_ptr().add(received_fd_count).cast(),
                descriptor_bytes.cast(),
                count.wrapping_mul(size_of::<c_int>()),
            );
            received_fd_count = received_fd_count.wrapping_add(count);
        }
        header = libc::CMSG_NXTHDR(addr_of!(message), header);
    }
    if received == 0 {
        close_received_fds(&received_fds, received_fd_count);
        return 0;
    }
    if !ancillary_valid
        || message.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0
        || !decode_packet(buffer, received as usize, packet)
        || received_fd_count != (*packet).role_count
    {
        return close_received_fds(&received_fds, received_fd_count);
    }
    libc::memcpy(
        (*packet).fds.as_mut_ptr().cast(),
        received_fds.as_ptr().cast(),
        received_fd_count.wrapping_mul(size_of::<c_int>()),
    );
    (*packet).fd_count = received_fd_count;
    received as c_int
}

/// # Safety
///
/// The first `count` entries must be descriptors owned by this process.
unsafe fn close_received_fds(descriptors: &[c_int; MAX_FD_COUNT], count: usize) -> c_int {
    let mut index = 0;
    while index < count {
        libc::close(*descriptors.as_ptr().add(index));
        index = index.wrapping_add(1);
    }
    -1
}

/// Forwards one validated packet and duplicates its descriptors to `fd`.
///
/// # Safety
///
/// `bytes` must be readable for `len` bytes, `packet` must describe that
/// validated packet, and every listed descriptor must remain live for the call.
unsafe fn forward_packet(fd: c_int, bytes: *const u8, len: usize, packet: *const Packet) -> bool {
    let mut vector = libc::iovec {
        iov_base: bytes.cast_mut().cast(),
        iov_len: len,
    };
    let mut control = [0usize; 64];
    let mut message: libc::msghdr = zeroed();
    message.msg_iov = addr_of_mut!(vector);
    message.msg_iovlen = 1;
    if (*packet).fd_count != 0 {
        let descriptor_bytes = (*packet).fd_count.wrapping_mul(size_of::<c_int>());
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = libc::CMSG_SPACE(descriptor_bytes as c_uint) as usize;
        if message.msg_controllen > size_of_val(&control) {
            set_errno(libc::EOVERFLOW);
            return false;
        }
        let header = libc::CMSG_FIRSTHDR(addr_of!(message));
        if header.is_null() {
            set_errno(libc::EINVAL);
            return false;
        }
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(descriptor_bytes as c_uint) as usize;
        libc::memcpy(libc::CMSG_DATA(header).cast(), (*packet).fds.as_ptr().cast(), descriptor_bytes);
    }
    loop {
        let sent = libc::sendmsg(fd, addr_of!(message), libc::MSG_NOSIGNAL);
        if sent == len as isize {
            return true;
        }
        if sent < 0 && errno() == libc::EINTR {
            continue;
        }
        return false;
    }
}

struct Encoder {
    bytes: [u8; 256],
    len: usize,
}

impl Encoder {
    const fn new() -> Self {
        Self { bytes: [0; 256], len: 0 }
    }

    /// Appends one byte to the fixed encoder buffer.
    ///
    /// # Safety
    ///
    /// `self.bytes` must be initialized and exclusively borrowed.
    unsafe fn byte(&mut self, value: u8) -> bool {
        if self.len == self.bytes.len() {
            return false;
        }
        *self.bytes.as_mut_ptr().add(self.len) = value;
        self.len = self.len.wrapping_add(1);
        true
    }

    /// Appends one Protobuf varint.
    ///
    /// # Safety
    ///
    /// `self.bytes` must be initialized and exclusively borrowed.
    unsafe fn varint(&mut self, mut value: u64) -> bool {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value = value.wrapping_shr(7);
            if value != 0 {
                byte |= 0x80;
            }
            if !self.byte(byte) {
                return false;
            }
            if value == 0 {
                return true;
            }
        }
    }

    /// Appends a varint field.
    ///
    /// # Safety
    ///
    /// `self.bytes` must be initialized and exclusively borrowed.
    unsafe fn field_varint(&mut self, field: u32, value: u64) -> bool {
        self.varint(u64::from(field).wrapping_shl(3)) && self.varint(value)
    }

    /// Appends a length-delimited field.
    ///
    /// # Safety
    ///
    /// `value` must be readable for `len` bytes and remain valid for the call.
    unsafe fn field_bytes(&mut self, field: u32, value: *const u8, len: usize) -> bool {
        if !self.varint(u64::from(field).wrapping_shl(3) | 2) || !self.varint(len as u64) || self.bytes.len().wrapping_sub(self.len) < len {
            return false;
        }
        libc::memcpy(self.bytes.as_mut_ptr().add(self.len).cast(), value.cast(), len);
        self.len = self.len.wrapping_add(len);
        true
    }
}

/// # Safety
///
/// `fd` must be a live sequenced-packet socket, `body` must be readable for
/// `body_len` bytes, and a nonnegative `descriptor` must be live for the call.
unsafe fn send_body(fd: c_int, request_id: u64, kind: u32, body: *const u8, body_len: usize, role: i32, descriptor: c_int) -> c_int {
    let mut encoder = Encoder::new();
    if !encoder.field_varint(1, 1)
        || (request_id != 0 && !encoder.field_varint(2, request_id))
        || !encoder.field_varint(3, FEATURE_BASE)
        || (role != 0 && !encoder.field_varint(4, role as u64))
        || !encoder.field_bytes(kind, body, body_len)
    {
        set_errno(libc::EOVERFLOW);
        return -1;
    }
    let mut vector = libc::iovec {
        iov_base: encoder.bytes.as_mut_ptr().cast(),
        iov_len: encoder.len,
    };
    let mut control = [0usize; 4];
    let mut message: libc::msghdr = zeroed();
    message.msg_iov = addr_of_mut!(vector);
    message.msg_iovlen = 1;
    if descriptor >= 0 {
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = libc::CMSG_SPACE(size_of::<c_int>() as c_uint) as usize;
        let header = libc::CMSG_FIRSTHDR(addr_of!(message));
        if header.is_null() {
            set_errno(libc::EINVAL);
            return -1;
        }
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(size_of::<c_int>() as c_uint) as usize;
        *libc::CMSG_DATA(header).cast::<c_int>() = descriptor;
    }
    loop {
        let sent = libc::sendmsg(fd, addr_of!(message), libc::MSG_NOSIGNAL);
        if sent == encoder.len as isize {
            return 0;
        }
        if sent < 0 && errno() == libc::EINTR {
            continue;
        }
        return -1;
    }
}

/// # Safety
///
/// `values` must be readable for `count` elements and `output` writable for
/// `capacity` bytes; the two regions must not overlap.
unsafe fn encode_varint_body(values: *const u64, count: usize, output: *mut u8, capacity: usize) -> Option<usize> {
    let mut encoder = Encoder::new();
    let mut index = 0;
    while index < count {
        let value = *values.add(index);
        if value != 0 && !encoder.field_varint(index.wrapping_add(1) as u32, value) {
            return None;
        }
        index = index.wrapping_add(1);
    }
    if encoder.len > capacity {
        return None;
    }
    libc::memcpy(output.cast(), encoder.bytes.as_ptr().cast(), encoder.len);
    Some(encoder.len)
}

/// # Safety
///
/// `fd` must be a live sequenced-packet socket.
unsafe fn send_error(fd: c_int, request_id: u64, stage: u32, error_number: u32) -> c_int {
    let values = [u64::from(stage), u64::from(error_number)];
    let mut body = [0u8; 32];
    let Some(len) = encode_varint_body(values.as_ptr(), values.len(), body.as_mut_ptr(), body.len()) else {
        return -1;
    };
    send_body(fd, request_id, BODY_ERROR, body.as_ptr(), len, 0, -1)
}

/// # Safety
///
/// `cursor` must satisfy its allocation bounds and remain exclusively borrowed.
unsafe fn parse_bytes_field(cursor: &mut Cursor, wire: u8) -> Option<(*const u8, usize)> {
    (wire == 2).then_some(())?;
    cursor.bytes()
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes.
unsafe fn validate_c_string(bytes: *const u8, len: usize) -> bool {
    !contains_byte(bytes, len, 0)
}

/// # Safety
///
/// `total` must be a valid exclusive reference. `alignment` must be nonzero.
unsafe fn add_size(total: &mut usize, count: usize, item: usize, alignment: usize) -> bool {
    let Some(remainder) = total.checked_rem(alignment) else {
        return false;
    };
    let Some(padding) = alignment.checked_sub(remainder).and_then(|value| value.checked_rem(alignment)) else {
        return false;
    };
    let Some(bytes) = count.checked_mul(item) else {
        return false;
    };
    let Some(next) = total.checked_add(padding).and_then(|value| value.checked_add(bytes)) else {
        return false;
    };
    *total = next;
    true
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes and `resource_out` writable.
unsafe fn validate_limit(bytes: *const u8, len: usize, resource_out: *mut u32) -> bool {
    let Some(mut cursor) = Cursor::new(bytes, len) else {
        return false;
    };
    let mut seen = 0u8;
    let mut resource = 0u64;
    let mut soft = 0u64;
    let mut hard = 0u64;
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        if !(1..=3).contains(&field) {
            if !cursor.skip(wire) {
                return false;
            }
            continue;
        }
        if wire != 0 {
            return false;
        }
        let bit = 1u8.wrapping_shl(field);
        if seen & bit != 0 {
            return false;
        }
        seen |= bit;
        let Some(value) = cursor.varint() else {
            return false;
        };
        if field == 1 {
            resource = value;
        } else {
            if rlim_from_u64(value).is_none() {
                return false;
            }
            if field == 2 {
                soft = value;
            } else {
                hard = value;
            }
        }
    }
    if resource > u64::from(u32::MAX) || !valid_resource(resource as u32) || soft > hard {
        return false;
    }
    *resource_out = resource as u32;
    true
}

/// # Safety
///
/// This function has no pointer preconditions; it is unsafe only because it is
/// used uniformly from the pre-runtime unsafe call graph.
unsafe fn valid_resource(resource: u32) -> bool {
    matches!(
        resource,
        libc::RLIMIT_AS | libc::RLIMIT_CPU | libc::RLIMIT_DATA | libc::RLIMIT_FSIZE | libc::RLIMIT_NOFILE | libc::RLIMIT_STACK
    )
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes and `shape` exclusively writable.
unsafe fn scan_groups(bytes: *const u8, len: usize, shape: &mut Shape) -> bool {
    let Some(mut cursor) = Cursor::new(bytes, len) else {
        return false;
    };
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        if field != 1 {
            if !cursor.skip(wire) {
                return false;
            }
            continue;
        }
        if wire != 0 {
            return false;
        }
        let Some(value) = cursor.varint() else {
            return false;
        };
        if value > u64::from(u32::MAX) || shape.group_count == MAX_ITEM_COUNT {
            return false;
        }
        shape.group_count = shape.group_count.wrapping_add(1);
    }
    true
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes and `shape` exclusively writable.
unsafe fn scan_unix(bytes: *const u8, len: usize, shape: &mut Shape) -> bool {
    let Some(mut cursor) = Cursor::new(bytes, len) else {
        return false;
    };
    let mut seen = 0u32;
    let mut new_session = false;
    let mut process_group = false;
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        if !(1..=6).contains(&field) {
            if !cursor.skip(wire) {
                return false;
            }
            continue;
        }
        let bit = 1u32.wrapping_shl(field);
        if seen & bit != 0 {
            return false;
        }
        seen |= bit;
        if field == 3 {
            let Some((groups, groups_len)) = parse_bytes_field(&mut cursor, wire) else {
                return false;
            };
            if !scan_groups(groups, groups_len, shape) {
                return false;
            }
        } else {
            if wire != 0 {
                return false;
            }
            let Some(value) = cursor.varint() else {
                return false;
            };
            match field {
                1 | 2 | 4 if value > u64::from(u32::MAX) => return false,
                4 => process_group = true,
                5 => {
                    if value > 1 {
                        return false;
                    }
                    new_session = value != 0;
                }
                6 if value > 0o777 => return false,
                _ => {}
            }
        }
    }
    !(new_session && process_group)
}

/// # Safety
///
/// `entry` must be readable for `len` bytes.
unsafe fn environment_name_len(entry: *const u8, len: usize) -> usize {
    let mut index = 0;
    while index < len && *entry.add(index) != b'=' {
        index = index.wrapping_add(1);
    }
    index
}

/// # Safety
///
/// Both names must refer to readable packet storage.
unsafe fn compare_environment_names(left: EnvironmentName, right: EnvironmentName) -> c_int {
    let common = left.len.min(right.len);
    let order = libc::memcmp(left.bytes.cast(), right.bytes.cast(), common);
    if order != 0 {
        return order;
    }
    match left.len.cmp(&right.len) {
        core::cmp::Ordering::Less => -1,
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Greater => 1,
    }
}

/// # Safety
///
/// `names` must point to `count` initialized, exclusively writable names whose
/// byte ranges remain readable.
unsafe fn sift_environment_names(names: *mut EnvironmentName, start: usize, count: usize) {
    let mut root = start;
    loop {
        let child = root.wrapping_mul(2).wrapping_add(1);
        if child >= count {
            return;
        }
        let largest = if child + 1 < count && compare_environment_names(*names.add(child), *names.add(child + 1)) < 0 {
            child + 1
        } else {
            child
        };
        if compare_environment_names(*names.add(root), *names.add(largest)) >= 0 {
            return;
        }
        core::ptr::swap(names.add(root), names.add(largest));
        root = largest;
    }
}

/// # Safety
///
/// `names` must point to `count` initialized, exclusively writable names whose
/// byte ranges remain readable.
unsafe fn sort_environment_names(names: *mut EnvironmentName, count: usize) {
    let mut start = count / 2;
    while start != 0 {
        start -= 1;
        sift_environment_names(names, start, count);
    }
    let mut end = count;
    while end > 1 {
        end -= 1;
        core::ptr::swap(names, names.add(end));
        sift_environment_names(names, 0, end);
    }
}

/// # Safety
///
/// `packet` must point to a decoded launch packet whose body remains live, and
/// `names` must be writable for exactly `expected_count` entries.
unsafe fn validate_environment_names(packet: *const Packet, names: *mut EnvironmentName, expected_count: usize) -> bool {
    let Some(mut cursor) = Cursor::new((*packet).body, (*packet).body_len) else {
        return false;
    };
    let mut count = 0;
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        if field == 2 {
            let Some((entry, len)) = parse_bytes_field(&mut cursor, wire) else {
                return false;
            };
            if count == expected_count {
                return false;
            }
            *names.add(count) = EnvironmentName {
                bytes: entry,
                len: environment_name_len(entry, len),
            };
            count = count.wrapping_add(1);
        } else if !cursor.skip(wire) {
            return false;
        }
    }
    if count != expected_count {
        return false;
    }
    sort_environment_names(names, count);
    let mut index = 1;
    while index < count {
        let previous = *names.add(index - 1);
        let current = *names.add(index);
        if previous.len == current.len && libc::memcmp(previous.bytes.cast(), current.bytes.cast(), current.len) == 0 {
            return false;
        }
        index = index.wrapping_add(1);
    }
    true
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes. `index < count` and `count`
/// must be the validated instruction count for the containing filter.
unsafe fn scan_seccomp_instruction(bytes: *const u8, len: usize, index: usize, count: usize, architecture: u32) -> bool {
    let Some(mut cursor) = Cursor::new(bytes, len) else {
        return false;
    };
    let mut values = [0u64; 4];
    let mut seen = 0u8;
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        if !(1..=4).contains(&field) {
            if !cursor.skip(wire) {
                return false;
            }
            continue;
        }
        if wire != 0 {
            return false;
        }
        let bit = 1u8.wrapping_shl(field);
        if seen & bit != 0 {
            return false;
        }
        seen |= bit;
        let Some(value) = cursor.varint() else {
            return false;
        };
        values[field as usize - 1] = value;
    }
    if values[0] > u64::from(u16::MAX)
        || values[1] > u64::from(u8::MAX)
        || values[2] > u64::from(u8::MAX)
        || values[3] > u64::from(u32::MAX)
    {
        return false;
    }
    let code = values[0] as u32;
    let remaining = count.wrapping_sub(index).wrapping_sub(1);
    if code & 7 == 5 {
        if code == 0x05 {
            if values[3] as usize >= remaining {
                return false;
            }
        } else if values[1] as usize >= remaining || values[2] as usize >= remaining {
            return false;
        }
    }
    if code & 7 == 6 && (code != 0x06 || values[3] as u32 & 0xffff_0000 == 0x7fc0_0000) {
        return false;
    }
    match index {
        0 => code == 0x20 && values[1] == 0 && values[2] == 0 && values[3] == 4,
        1 => code == 0x15 && values[1] == 1 && values[2] == 0 && values[3] == u64::from(architecture),
        2 => code == 0x06 && values[1] == 0 && values[2] == 0 && values[3] == 0x8000_0000,
        _ if index + 1 == count => code == 0x06,
        _ => true,
    }
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes and `shape` exclusively writable.
unsafe fn scan_seccomp(bytes: *const u8, len: usize, shape: &mut Shape) -> bool {
    let Some(mut cursor) = Cursor::new(bytes, len) else {
        return false;
    };
    let mut architecture = 0u64;
    let mut instruction_count = 0usize;
    let mut seen_architecture = false;
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        match field {
            1 => {
                if wire != 0 || seen_architecture {
                    return false;
                }
                let Some(value) = cursor.varint() else {
                    return false;
                };
                architecture = value;
                seen_architecture = true;
            }
            2 if wire == 2 => {
                if instruction_count == MAX_ITEM_COUNT || cursor.bytes().is_none() {
                    return false;
                }
                instruction_count = instruction_count.wrapping_add(1);
            }
            _ if cursor.skip(wire) => {}
            _ => return false,
        }
    }
    #[cfg(target_arch = "x86_64")]
    let current_architecture = 0xc000_003e;
    #[cfg(target_arch = "aarch64")]
    let current_architecture = 0xc000_00b7;
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let current_architecture = 0;
    if current_architecture == 0 || !seen_architecture || architecture != current_architecture || instruction_count < 4 {
        return false;
    }
    let Some(mut cursor) = Cursor::new(bytes, len) else {
        return false;
    };
    let mut index = 0;
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        if field == 1 {
            if wire != 0 || cursor.varint().is_none() {
                return false;
            }
        } else if field == 2 {
            let Some((instruction, instruction_len)) = parse_bytes_field(&mut cursor, wire) else {
                return false;
            };
            if !scan_seccomp_instruction(instruction, instruction_len, index, instruction_count, architecture as u32) {
                return false;
            }
            index = index.wrapping_add(1);
        } else if !cursor.skip(wire) {
            return false;
        }
    }
    shape.seccomp_count = instruction_count;
    true
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes and `no_new_privs` writable.
unsafe fn scan_privileges(bytes: *const u8, len: usize, no_new_privs: *mut bool) -> bool {
    let Some(mut cursor) = Cursor::new(bytes, len) else {
        return false;
    };
    let mut seen = 0u8;
    let mut reduces_privileges = false;
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        if !(1..=5).contains(&field) {
            if !cursor.skip(wire) {
                return false;
            }
            continue;
        }
        if wire != 0 {
            return false;
        }
        let bit = 1u8.wrapping_shl(field);
        if seen & bit != 0 {
            return false;
        }
        seen |= bit;
        let Some(value) = cursor.varint() else {
            return false;
        };
        if value > 1 {
            return false;
        }
        reduces_privileges |= value != 0;
        if field == 1 {
            *no_new_privs = value != 0;
        }
    }
    reduces_privileges
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes, `packet` must point to the
/// containing decoded packet, and `shape` must be exclusively writable.
unsafe fn scan_sandbox(bytes: *const u8, len: usize, packet: *const Packet, shape: &mut Shape) -> bool {
    let Some(mut cursor) = Cursor::new(bytes, len) else {
        return false;
    };
    let mut seen = 0u8;
    let mut no_new_privs = false;
    let mut landlock = false;
    let mut cgroup = false;
    let mut namespace_count = 0usize;
    let mut namespace_kinds = [0u64; 6];
    let mut previous_namespace = 0u64;
    let mut landlock_abi = 0u64;
    let mut landlock_rights = 0u64;
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        if !(1..=7).contains(&field) {
            if !cursor.skip(wire) {
                return false;
            }
            continue;
        }
        if field != 5 {
            let bit = 1u8.wrapping_shl(field);
            if seen & bit != 0 {
                return false;
            }
            seen |= bit;
        }
        match field {
            1 => {
                let Some((value, value_len)) = parse_bytes_field(&mut cursor, wire) else {
                    return false;
                };
                if !scan_privileges(value, value_len, addr_of_mut!(no_new_privs)) {
                    return false;
                }
            }
            2 => {
                let Some((value, value_len)) = parse_bytes_field(&mut cursor, wire) else {
                    return false;
                };
                if !scan_seccomp(value, value_len, shape) {
                    return false;
                }
            }
            3 | 4 => {
                if wire != 0 {
                    return false;
                }
                let Some(value) = cursor.varint() else {
                    return false;
                };
                if value > 1 {
                    return false;
                }
                if field == 3 {
                    landlock = value != 0;
                } else {
                    cgroup = value != 0;
                }
            }
            5 => {
                if wire != 0 {
                    return false;
                }
                let Some(value) = cursor.varint() else {
                    return false;
                };
                if namespace_count == namespace_kinds.len() || !(1..=6).contains(&value) || value <= previous_namespace {
                    return false;
                }
                previous_namespace = value;
                namespace_kinds[namespace_count] = value;
                namespace_count = namespace_count.wrapping_add(1);
            }
            6 | 7 => {
                if wire != 0 {
                    return false;
                }
                let Some(value) = cursor.varint() else {
                    return false;
                };
                if field == 6 {
                    landlock_abi = value;
                } else {
                    landlock_rights = value;
                }
            }
            _ => return false,
        }
    }
    if (landlock || shape.seccomp_count != 0) && !no_new_privs
        || landlock != (landlock_abi != 0)
        || landlock != (landlock_rights != 0)
        || landlock_abi > u64::from(u32::MAX)
    {
        return false;
    }
    let expected = 3usize
        .wrapping_add(usize::from(cgroup))
        .wrapping_add(usize::from(landlock))
        .wrapping_add(namespace_count);
    if (*packet).role_count != expected {
        return false;
    }
    let mut role = 3;
    if cgroup {
        if (*packet).roles[role] != ROLE_CGROUP_PROCS {
            return false;
        }
        role = role.wrapping_add(1);
    }
    if landlock {
        if (*packet).roles[role] != ROLE_LANDLOCK_RULESET {
            return false;
        }
        role = role.wrapping_add(1);
    }
    let namespace_roles = [
        ROLE_NAMESPACE_CGROUP,
        ROLE_NAMESPACE_IPC,
        ROLE_NAMESPACE_UTS,
        ROLE_NAMESPACE_NETWORK,
        ROLE_NAMESPACE_TIME,
        ROLE_NAMESPACE_MOUNT,
    ];
    let mut index = 0;
    while index < namespace_count {
        if (*packet).roles[role + index] != namespace_roles[namespace_kinds[index] as usize - 1] {
            return false;
        }
        index = index.wrapping_add(1);
    }
    true
}

/// # Safety
///
/// `packet` must point to a decoded packet whose body storage remains live,
/// and `shape` must be exclusively writable.
unsafe fn scan_launch(packet: *const Packet, shape: &mut Shape) -> bool {
    *shape = Shape::empty();
    let Some(mut cursor) = Cursor::new((*packet).body, (*packet).body_len) else {
        return false;
    };
    let mut singular = 0u32;
    let mut sandbox_present = false;
    let mut resources = [u32::MAX; MAX_ITEM_COUNT];
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        match field {
            1 | 2 => {
                let Some((value, len)) = parse_bytes_field(&mut cursor, wire) else {
                    return false;
                };
                let Some(string_bytes) = len.checked_add(1).and_then(|size| shape.string_bytes.checked_add(size)) else {
                    return false;
                };
                if contains_byte(value, len, 0) {
                    return false;
                }
                if field == 2 && (len == 0 || *value == b'=' || !contains_byte(value, len, b'=')) {
                    return false;
                }
                shape.string_bytes = string_bytes;
                if field == 1 {
                    if shape.argc == MAX_ITEM_COUNT {
                        return false;
                    }
                    shape.argc = shape.argc.wrapping_add(1);
                } else {
                    if shape.envc == MAX_ITEM_COUNT {
                        return false;
                    }
                    shape.envc = shape.envc.wrapping_add(1);
                }
            }
            3 | 4 | 6 => {
                let bit = 1u32.wrapping_shl(field);
                if singular & bit != 0 {
                    return false;
                }
                singular |= bit;
                let Some((value, value_len)) = parse_bytes_field(&mut cursor, wire) else {
                    return false;
                };
                if field == 3 {
                    let Some(string_bytes) = value_len.checked_add(1).and_then(|size| shape.string_bytes.checked_add(size)) else {
                        return false;
                    };
                    if !validate_c_string(value, value_len) {
                        return false;
                    }
                    shape.string_bytes = string_bytes;
                } else {
                    match field {
                        4 if !scan_unix(value, value_len, shape) => return false,
                        6 => {
                            if !scan_sandbox(value, value_len, packet, shape) {
                                return false;
                            }
                            sandbox_present = true;
                        }
                        _ => {}
                    }
                }
            }
            5 => {
                let Some((value, value_len)) = parse_bytes_field(&mut cursor, wire) else {
                    return false;
                };
                if shape.limit_count == MAX_ITEM_COUNT {
                    return false;
                }
                let mut resource = 0;
                if !validate_limit(value, value_len, addr_of_mut!(resource)) {
                    return false;
                }
                let mut index = 0;
                while index < shape.limit_count {
                    if *resources.as_ptr().add(index) == resource {
                        return false;
                    }
                    index = index.wrapping_add(1);
                }
                *resources.as_mut_ptr().add(shape.limit_count) = resource;
                shape.limit_count = shape.limit_count.wrapping_add(1);
            }
            _ if !cursor.skip(wire) => return false,
            _ => {}
        }
    }
    shape.argc != 0 && sandbox_present == (*packet).sandbox_feature
}

struct Arena {
    base: *mut u8,
    size: usize,
    used: usize,
}

/// # Safety
///
/// `arena.base` must identify a writable allocation of `arena.size` bytes and
/// `arena.used` must describe its initialized prefix. `alignment` is nonzero.
unsafe fn arena_allocate(arena: &mut Arena, count: usize, item: usize, alignment: usize) -> *mut c_void {
    let Some(remainder) = arena.used.checked_rem(alignment) else {
        return null_mut();
    };
    let Some(padding) = alignment.checked_sub(remainder).and_then(|value| value.checked_rem(alignment)) else {
        return null_mut();
    };
    let Some(bytes) = count.checked_mul(item) else {
        return null_mut();
    };
    let Some(offset) = arena.used.checked_add(padding) else {
        return null_mut();
    };
    let Some(end) = offset.checked_add(bytes) else {
        return null_mut();
    };
    if end > arena.size {
        return null_mut();
    }
    arena.used = end;
    arena.base.add(offset).cast()
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes and `arena` must satisfy
/// [`arena_allocate`]'s allocation invariant.
unsafe fn copy_string(arena: &mut Arena, bytes: *const u8, len: usize) -> *mut c_char {
    let Some(storage_len) = len.checked_add(1) else {
        return null_mut();
    };
    let target = arena_allocate(arena, storage_len, 1, 1).cast::<c_char>();
    if target.is_null() {
        return null_mut();
    }
    libc::memcpy(target.cast(), bytes.cast(), len);
    *target.add(len) = 0;
    target
}

/// # Safety
///
/// `value` must be writable for `len` bytes.
#[cfg(not(miri))]
unsafe fn secure_zero(value: *mut c_void, len: usize) {
    explicit_bzero(value, len);
}

/// # Safety
///
/// `value` must be writable for `len` bytes.
#[cfg(miri)]
unsafe fn secure_zero(value: *mut c_void, len: usize) {
    let value = value.cast::<u8>();
    let mut offset = 0;
    while offset < len {
        // SAFETY: the caller guarantees the complete range is writable.
        unsafe { value.add(offset).write_volatile(0) };
        offset += 1;
    }
}

/// # Safety
///
/// `buffer` must be writable for `MAX_PACKET_LEN` bytes, `received` must be a
/// positive count returned by `receive_packet`, and `high_water` must track the
/// greatest datagram length previously stored in this allocation.
unsafe fn clear_received_packet(buffer: *mut u8, received: c_int, high_water: &mut usize) {
    debug_assert!(received > 0 && (received as usize) <= MAX_PACKET_LEN);
    *high_water = (*high_water).max(received as usize);
    secure_zero(buffer.cast(), *high_water);
}

/// # Safety
///
/// `launch` must point to an initialized launch. Its arena, when non-null,
/// must still be owned by the reusable arena slot and must not be freed here.
/// The complete reusable allocation is erased, including capacity beyond the
/// current launch's initialized region.
unsafe fn free_launch(launch: *mut Launch) {
    if !(*launch).arena.is_null() {
        secure_zero((*launch).arena, (*launch).arena_capacity);
    }
    *launch = Launch::empty();
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes and `launch` must point to an
/// initialized launch with group storage sized by the preceding scan.
unsafe fn decode_unix(bytes: *const u8, len: usize, launch: *mut Launch) -> bool {
    let Some(mut cursor) = Cursor::new(bytes, len) else {
        return false;
    };
    let mut group_index = 0;
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        if !(1..=6).contains(&field) {
            if !cursor.skip(wire) {
                return false;
            }
            continue;
        }
        if field == 3 {
            let Some((group_bytes, group_len)) = parse_bytes_field(&mut cursor, wire) else {
                return false;
            };
            let Some(mut groups) = Cursor::new(group_bytes, group_len) else {
                return false;
            };
            (*launch).has_groups = true;
            while groups.remaining() != 0 {
                let Some((field, wire)) = groups.key() else {
                    return false;
                };
                if field != 1 {
                    if !groups.skip(wire) {
                        return false;
                    }
                    continue;
                }
                if wire != 0 {
                    return false;
                }
                let Some(value) = groups.varint() else {
                    return false;
                };
                *(*launch).groups.add(group_index) = value as libc::gid_t;
                group_index = group_index.wrapping_add(1);
            }
            continue;
        }
        let Some(value) = (wire == 0).then(|| cursor.varint()).flatten() else {
            return false;
        };
        match field {
            1 => {
                (*launch).has_uid = true;
                (*launch).uid = value as libc::uid_t;
            }
            2 => {
                (*launch).has_gid = true;
                (*launch).gid = value as libc::gid_t;
            }
            4 => {
                (*launch).has_process_group = true;
                let zigzag = (value.wrapping_shr(1) as i64) ^ ((value & 1) as i64).wrapping_neg();
                (*launch).process_group = zigzag as libc::pid_t;
            }
            5 => (*launch).new_session = value != 0,
            6 => {
                (*launch).has_umask = true;
                (*launch).umask_value = value as libc::mode_t;
            }
            _ => {}
        }
    }
    true
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes and `target` must be writable.
unsafe fn decode_limit(bytes: *const u8, len: usize, target: *mut Limit) -> bool {
    let Some(mut cursor) = Cursor::new(bytes, len) else {
        return false;
    };
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        if !(1..=3).contains(&field) {
            if !cursor.skip(wire) {
                return false;
            }
            continue;
        }
        if wire != 0 {
            return false;
        }
        let Some(value) = cursor.varint() else {
            return false;
        };
        match field {
            1 => (*target).resource = value as c_int,
            2 => {
                let Some(limit) = rlim_from_u64(value) else {
                    return false;
                };
                (*target).value.rlim_cur = limit;
            }
            3 => {
                let Some(limit) = rlim_from_u64(value) else {
                    return false;
                };
                (*target).value.rlim_max = limit;
            }
            _ => {}
        }
    }
    true
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes and `launch` must be writable.
unsafe fn decode_privileges(bytes: *const u8, len: usize, launch: *mut Launch) -> bool {
    let Some(mut cursor) = Cursor::new(bytes, len) else {
        return false;
    };
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        if !(1..=5).contains(&field) {
            if !cursor.skip(wire) {
                return false;
            }
            continue;
        }
        if wire != 0 {
            return false;
        }
        let Some(value) = cursor.varint() else {
            return false;
        };
        match field {
            1 => (*launch).no_new_privs = value != 0,
            2 => (*launch).disable_dumping = value != 0,
            3 => (*launch).clear_capabilities = value != 0,
            4 => (*launch).drop_capability_bounding_set = value != 0,
            5 => (*launch).lock_securebits = value != 0,
            _ => return false,
        }
    }
    true
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes and `target` must be writable.
unsafe fn decode_seccomp_instruction(bytes: *const u8, len: usize, target: *mut libc::sock_filter) -> bool {
    let Some(mut cursor) = Cursor::new(bytes, len) else {
        return false;
    };
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        if !(1..=4).contains(&field) {
            if !cursor.skip(wire) {
                return false;
            }
            continue;
        }
        if wire != 0 {
            return false;
        }
        let Some(value) = cursor.varint() else {
            return false;
        };
        match field {
            1 => (*target).code = value as u16,
            2 => (*target).jt = value as u8,
            3 => (*target).jf = value as u8,
            4 => (*target).k = value as u32,
            _ => return false,
        }
    }
    true
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes and `launch.seccomp` must provide
/// writable storage for the validated instruction count.
unsafe fn decode_seccomp(bytes: *const u8, len: usize, launch: *mut Launch) -> bool {
    let Some(mut cursor) = Cursor::new(bytes, len) else {
        return false;
    };
    let mut instruction = 0usize;
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        if field == 1 {
            if wire != 0 {
                return false;
            }
            let Some(value) = cursor.varint() else {
                return false;
            };
            (*launch).seccomp_architecture = value as u32;
        } else if field == 2 {
            let Some((value, value_len)) = parse_bytes_field(&mut cursor, wire) else {
                return false;
            };
            if !decode_seccomp_instruction(value, value_len, (*launch).seccomp.add(instruction)) {
                return false;
            }
            instruction = instruction.wrapping_add(1);
        } else if !cursor.skip(wire) {
            return false;
        }
    }
    instruction == (*launch).seccomp_count
}

/// # Safety
///
/// `bytes` must be readable for `len` bytes and `launch` must be writable with
/// any nested arrays allocated according to the preceding scan.
unsafe fn decode_sandbox(bytes: *const u8, len: usize, launch: *mut Launch) -> bool {
    let Some(mut cursor) = Cursor::new(bytes, len) else {
        return false;
    };
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        match field {
            1 => {
                let Some((value, value_len)) = parse_bytes_field(&mut cursor, wire) else {
                    return false;
                };
                if !decode_privileges(value, value_len, launch) {
                    return false;
                }
            }
            2 => {
                let Some((value, value_len)) = parse_bytes_field(&mut cursor, wire) else {
                    return false;
                };
                if !decode_seccomp(value, value_len, launch) {
                    return false;
                }
            }
            3 | 4 => {
                if wire != 0 {
                    return false;
                }
                let Some(value) = cursor.varint() else {
                    return false;
                };
                if field == 3 {
                    (*launch).landlock = value != 0;
                } else {
                    (*launch).cgroup = value != 0;
                }
            }
            5 => {
                if wire != 0 || cursor.varint().is_none() {
                    return false;
                }
                (*launch).namespace_count = (*launch).namespace_count.wrapping_add(1);
            }
            6 | 7 => {
                if wire != 0 {
                    return false;
                }
                let Some(value) = cursor.varint() else {
                    return false;
                };
                if field == 6 {
                    (*launch).landlock_abi = value as u32;
                } else {
                    (*launch).landlock_handled_access = value;
                }
            }
            _ if cursor.skip(wire) => {}
            _ => return false,
        }
    }
    true
}

#[expect(clippy::unnecessary_wraps, reason = "the 32-bit branch rejects values")]
fn rlim_from_u64(value: u64) -> Option<libc::rlim_t> {
    #[cfg(target_pointer_width = "32")]
    {
        u32::try_from(value).ok()
    }
    #[cfg(target_pointer_width = "64")]
    {
        Some(value)
    }
}

/// # Safety
///
/// `packet` must point to a successfully decoded launch packet whose body
/// remains live and whose cached shape was produced by [`scan_launch`]. All
/// output pointers must be valid and exclusively writable. The arena pointer
/// must either be null or own a `calloc` allocation. A null pointer with
/// nonzero capacity or an allocation with zero capacity is rejected before
/// arena access. Ownership remains with the pair on every early return.
unsafe fn parse_launch(packet: *const Packet, launch: &mut Launch, arena_storage: &mut *mut c_void, arena_capacity: &mut usize) -> bool {
    *launch = Launch::empty();
    if (*arena_storage).is_null() != (*arena_capacity == 0) {
        return false;
    }
    if !(*packet).launch_validated {
        return false;
    }
    let shape = (*packet).launch_shape;
    let mut arena_size = 0;
    let Some(argv_count) = shape.argc.checked_add(1) else {
        return false;
    };
    let Some(envp_count) = shape.envc.checked_add(1) else {
        return false;
    };
    if !add_size(&mut arena_size, argv_count, size_of::<*mut c_char>(), align_of::<*mut c_char>())
        || !add_size(&mut arena_size, envp_count, size_of::<*mut c_char>(), align_of::<*mut c_char>())
        || !add_size(
            &mut arena_size,
            shape.envc,
            size_of::<EnvironmentName>(),
            align_of::<EnvironmentName>(),
        )
        || !add_size(
            &mut arena_size,
            shape.group_count,
            size_of::<libc::gid_t>(),
            align_of::<libc::gid_t>(),
        )
        || !add_size(&mut arena_size, shape.limit_count, size_of::<Limit>(), align_of::<Limit>())
        || !add_size(
            &mut arena_size,
            shape.seccomp_count,
            size_of::<libc::sock_filter>(),
            align_of::<libc::sock_filter>(),
        )
        || !add_size(&mut arena_size, shape.string_bytes, 1, 1)
    {
        return false;
    }
    if *arena_capacity < arena_size {
        let replacement = libc::calloc(arena_size, 1);
        if replacement.is_null() {
            return false;
        }
        if !(*arena_storage).is_null() {
            secure_zero(*arena_storage, *arena_capacity);
            libc::free(*arena_storage);
        }
        *arena_storage = replacement;
        *arena_capacity = arena_size;
    } else {
        libc::memset(*arena_storage, 0, *arena_capacity);
    }
    launch.arena = *arena_storage;
    launch.arena_size = arena_size;
    launch.arena_capacity = *arena_capacity;
    launch.argc = shape.argc as c_int;
    launch.group_count = shape.group_count;
    launch.limit_count = shape.limit_count;
    launch.seccomp_count = shape.seccomp_count;
    let mut arena = Arena {
        base: (*arena_storage).cast(),
        size: arena_size,
        used: 0,
    };
    launch.argv = arena_allocate(&mut arena, argv_count, size_of::<*mut c_char>(), align_of::<*mut c_char>()).cast();
    launch.envp = arena_allocate(&mut arena, envp_count, size_of::<*mut c_char>(), align_of::<*mut c_char>()).cast();
    let environment_names =
        arena_allocate(&mut arena, shape.envc, size_of::<EnvironmentName>(), align_of::<EnvironmentName>()).cast::<EnvironmentName>();
    launch.groups = arena_allocate(&mut arena, shape.group_count, size_of::<libc::gid_t>(), align_of::<libc::gid_t>()).cast();
    launch.limits = arena_allocate(&mut arena, shape.limit_count, size_of::<Limit>(), align_of::<Limit>()).cast();
    launch.seccomp = arena_allocate(
        &mut arena,
        shape.seccomp_count,
        size_of::<libc::sock_filter>(),
        align_of::<libc::sock_filter>(),
    )
    .cast();
    if launch.argv.is_null()
        || launch.envp.is_null()
        || (shape.envc != 0 && environment_names.is_null())
        || (shape.group_count != 0 && launch.groups.is_null())
        || (shape.limit_count != 0 && launch.limits.is_null())
        || (shape.seccomp_count != 0 && launch.seccomp.is_null())
    {
        return false;
    }
    if !validate_environment_names(packet, environment_names, shape.envc) {
        return false;
    }
    let Some(mut cursor) = Cursor::new((*packet).body, (*packet).body_len) else {
        return false;
    };
    let mut arg = 0;
    let mut env = 0;
    let mut limit = 0;
    while cursor.remaining() != 0 {
        let Some((field, wire)) = cursor.key() else {
            return false;
        };
        match field {
            1..=6 => {
                let Some((value, value_len)) = parse_bytes_field(&mut cursor, wire) else {
                    return false;
                };
                match field {
                    1 => {
                        *launch.argv.add(arg) = copy_string(&mut arena, value, value_len);
                        arg = arg.wrapping_add(1);
                    }
                    2 => {
                        *launch.envp.add(env) = copy_string(&mut arena, value, value_len);
                        env = env.wrapping_add(1);
                    }
                    3 => launch.cwd = copy_string(&mut arena, value, value_len),
                    4 if !decode_unix(value, value_len, launch) => return false,
                    5 => {
                        if !decode_limit(value, value_len, launch.limits.add(limit)) {
                            return false;
                        }
                        limit = limit.wrapping_add(1);
                    }
                    6 if !decode_sandbox(value, value_len, launch) => return false,
                    _ => {}
                }
            }
            _ if !cursor.skip(wire) => return false,
            _ => {}
        }
    }
    arena.used == arena.size
}

/// # Safety
///
/// `/proc/self/task` must be accessible through libc in the pre-runtime process.
unsafe fn process_is_single_threaded() -> bool {
    process_thread_count() == 1
}

/// # Safety
///
/// `/proc/self/task` must be accessible through libc in the pre-runtime process.
unsafe fn process_thread_count() -> c_int {
    count_numeric_directory_entries(c"/proc/self/task".as_ptr(), -1)
}

/// # Safety
///
/// `control_fd` must be the only non-standard descriptor permitted in the
/// template; the function only observes `/proc`.
unsafe fn template_has_only_expected_fds(control_fd: c_int) -> bool {
    count_numeric_directory_entries(c"/proc/self/fd".as_ptr(), control_fd) == 0
}

/// # Safety
///
/// `path` must point to a NUL-terminated directory path. The directory stream
/// returned by libc is owned and closed on every exit after a successful open.
unsafe fn count_numeric_directory_entries(path: *const c_char, allowed_fd: c_int) -> c_int {
    let directory = libc::opendir(path);
    if directory.is_null() {
        return -1;
    }
    let directory_fd = libc::dirfd(directory);
    let mut count: c_int = 0;
    loop {
        set_errno(0);
        let entry = libc::readdir(directory);
        if entry.is_null() {
            let error = errno();
            libc::closedir(directory);
            return if error == 0 { count } else { -1 };
        }
        let name = addr_of!((*entry).d_name).cast::<c_char>();
        if *name < byte_as_c_char(b'0') || *name > byte_as_c_char(b'9') {
            continue;
        }
        let mut end = null_mut();
        let value = libc::strtol(name, addr_of_mut!(end), 10);
        if errno() == 0
            && parsed_entire_c_string(name, end)
            && (allowed_fd < 0
                || (value > c_long::from(libc::STDERR_FILENO) && value != c_long::from(allowed_fd) && value != c_long::from(directory_fd)))
        {
            count = count.wrapping_add(1);
        }
    }
}

/// # Safety
///
/// The caller must invoke this while the template is single-threaded; the
/// function owns and closes its temporary `/proc/self/maps` descriptor.
unsafe fn template_has_no_writable_shared_mappings() -> bool {
    let fd = libc::open(c"/proc/self/maps".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
    if fd < 0 {
        return false;
    }
    let mut buffer = [0u8; 4096];
    let mut column = 0usize;
    let mut in_permissions = false;
    let mut writable = false;
    let mut shared = false;
    loop {
        let read = libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len());
        if read < 0 && errno() == libc::EINTR {
            continue;
        }
        if read <= 0 {
            libc::close(fd);
            return read == 0;
        }
        let mut index = 0;
        while index < read as usize {
            let byte = *buffer.as_ptr().add(index);
            if byte == b'\n' {
                if writable && shared {
                    libc::close(fd);
                    return false;
                }
                column = 0;
                in_permissions = false;
                writable = false;
                shared = false;
            } else if !in_permissions {
                if byte == b' ' {
                    in_permissions = true;
                    column = 0;
                }
            } else {
                if column == 1 {
                    writable = byte == b'w';
                } else if column == 3 {
                    shared = byte == b's';
                }
                column = column.wrapping_add(1);
            }
            index = index.wrapping_add(1);
        }
    }
}

/// # Safety
///
/// The process must be single-threaded while the global immutable signal-reset
/// table is initialized, and no child may read it concurrently.
unsafe fn prepare_signal_reset_plan() -> bool {
    libc::memset(addr_of_mut!(SIGNALS_TO_RESET).cast(), 0, SIGNAL_COUNT);
    if !RESET_CHILD_SIGNALS {
        return true;
    }
    let mut signal = 1;
    while signal < SIGNAL_COUNT as c_int {
        let mut current: libc::sigaction = zeroed();
        if libc::sigaction(signal, null(), addr_of_mut!(current)) != 0 {
            if errno() == libc::EINVAL {
                signal = signal.wrapping_add(1);
                continue;
            }
            return false;
        }
        *addr_of_mut!(SIGNALS_TO_RESET).cast::<u8>().add(signal as usize) = u8::from(current.sa_sigaction != libc::SIG_IGN);
        signal = signal.wrapping_add(1);
    }
    true
}

/// # Safety
///
/// The signal-reset table must have been initialized and immutable; this must
/// run in the post-fork child before application threads exist.
unsafe fn normalize_signal_dispositions() -> bool {
    let mut action: libc::sigaction = zeroed();
    action.sa_sigaction = libc::SIG_DFL;
    if libc::sigemptyset(addr_of_mut!(action.sa_mask)) != 0 {
        return false;
    }
    let mut signal = 1;
    while signal < SIGNAL_COUNT as c_int {
        if *addr_of!(SIGNALS_TO_RESET).cast::<u8>().add(signal as usize) != 0
            && libc::sigaction(signal, addr_of!(action), null_mut()) != 0
            && errno() != libc::EINVAL
        {
            return false;
        }
        signal = signal.wrapping_add(1);
    }
    true
}

/// # Safety
///
/// This must run in the post-fork child before application threads exist.
unsafe fn disable_alternate_signal_stack() -> bool {
    let disabled = libc::stack_t {
        ss_sp: null_mut(),
        ss_flags: libc::SS_DISABLE,
        ss_size: 0,
    };
    libc::sigaltstack(addr_of!(disabled), null_mut()) == 0
}

/// # Safety
///
/// `fds` must point to three live descriptors owned by the decoded packet.
/// Temporary duplicates are closed on every success and failure path.
unsafe fn install_stdio(fds: *const c_int) -> bool {
    let mut copies = [-1; 3];
    let mut index = 0;
    while index < 3 {
        *copies.as_mut_ptr().add(index) = libc::fcntl(*fds.add(index), libc::F_DUPFD_CLOEXEC, 4);
        if *copies.as_ptr().add(index) < 0 {
            break;
        }
        index = index.wrapping_add(1);
    }
    if index == 3 {
        index = 0;
        while index < 3 && libc::dup2(*copies.as_ptr().add(index), index as c_int) >= 0 {
            index = index.wrapping_add(1);
        }
    }
    let success = index == 3;
    index = 0;
    while index < 3 {
        if *copies.as_ptr().add(index) >= 0 {
            libc::close(*copies.as_ptr().add(index));
        }
        index = index.wrapping_add(1);
    }
    success
}

/// Transfers packet descriptors that now alias installed standard streams to
/// the process so packet cleanup does not close them.
///
/// # Safety
///
/// `packet` must point to an initialized packet whose standard streams were
/// successfully installed from its first three descriptors.
unsafe fn preserve_installed_stdio(packet: *mut Packet) {
    let mut index = 0;
    while index < (*packet).fd_count {
        let descriptor = *(*packet).fds.as_ptr().add(index);
        if (libc::STDIN_FILENO..=libc::STDERR_FILENO).contains(&descriptor) {
            *(*packet).fds.as_mut_ptr().add(index) = -1;
        }
        index = index.wrapping_add(1);
    }
}

/// Closes standard streams whose descriptor roles preserve a closed inherited
/// stream. Their transport placeholders have already served their purpose.
///
/// # Safety
///
/// `packet` must point to an initialized launch packet whose standard streams
/// were installed and whose first three roles passed protocol validation.
unsafe fn close_inherited_closed_stdio(packet: *const Packet) {
    for (descriptor, closed_role) in [
        (libc::STDIN_FILENO, ROLE_CLOSED_STDIN),
        (libc::STDOUT_FILENO, ROLE_CLOSED_STDOUT),
        (libc::STDERR_FILENO, ROLE_CLOSED_STDERR),
    ] {
        if (*packet).roles[descriptor as usize] == closed_role {
            libc::close(descriptor);
        }
    }
}

/// # Safety
///
/// This must run in the post-fork child after fd 3 has become the
/// acknowledgement pipe; all descriptors above 3 are intentionally consumed.
unsafe fn close_above_acknowledgement() -> bool {
    #[cfg(not(test))]
    if let Some(closed) = close_above_acknowledgement_with_close_range() {
        return closed;
    }
    if let Some(closed) = close_above_acknowledgement_with_procfs() {
        return closed;
    }
    let mut maximum = libc::sysconf(libc::_SC_OPEN_MAX);
    if maximum < 0 {
        maximum = FD_FALLBACK_LIMIT;
    }
    if maximum > FD_FALLBACK_LIMIT {
        set_errno(libc::EOVERFLOW);
        return false;
    }
    let mut descriptor = 4;
    while descriptor < maximum {
        close_long(descriptor);
        descriptor = descriptor.wrapping_add(1);
    }
    true
}

/// # Safety
///
/// This closes all descriptors above the acknowledgement pipe in the post-fork child.
unsafe fn close_above_acknowledgement_with_close_range() -> Option<bool> {
    if libc::syscall(libc::SYS_close_range, 4u32, c_uint::MAX, 0u32) == 0 {
        Some(true)
    } else if errno() == libc::ENOSYS {
        None
    } else {
        Some(false)
    }
}

/// # Safety
///
/// This closes all descriptors above the acknowledgement pipe in the post-fork child.
unsafe fn close_above_acknowledgement_with_procfs() -> Option<bool> {
    let directory = libc::opendir(c"/proc/self/fd".as_ptr());
    if !directory.is_null() {
        let directory_fd = libc::dirfd(directory);
        loop {
            set_errno(0);
            let entry = libc::readdir(directory);
            if entry.is_null() {
                let error = errno();
                libc::closedir(directory);
                return Some(error == 0);
            }
            let name = addr_of!((*entry).d_name).cast::<c_char>();
            let mut end = null_mut();
            set_errno(0);
            let value = libc::strtol(name, addr_of_mut!(end), 10);
            if errno() == 0 && parsed_entire_c_string(name, end) && value >= 4 && value != c_long::from(directory_fd) {
                close_long(value);
            }
        }
    }
    None
}

/// # Safety
///
/// A representable nonnegative descriptor is closed at most once by the caller.
unsafe fn close_long(descriptor: c_long) {
    #[cfg(target_pointer_width = "32")]
    libc::close(descriptor);
    #[cfg(target_pointer_width = "64")]
    if let Ok(descriptor) = c_int::try_from(descriptor) {
        libc::close(descriptor);
    }
}

const fn byte_as_c_char(byte: u8) -> c_char {
    c_char::from_ne_bytes([byte])
}

/// # Safety
///
/// `start` must point to a NUL-terminated C string and `end` must either be
/// null or point into that string.
unsafe fn parsed_entire_c_string(start: *const c_char, end: *mut c_char) -> bool {
    !end.is_null() && end == start.add(libc::strlen(start)).cast_mut()
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ChildSetupResult {
    stage: u32,
    error_number: c_int,
}

/// # Safety
///
/// `ack_fd` must be the live child-owned acknowledgement descriptor. This
/// function never returns and relinquishes all process resources via `_exit`.
unsafe fn child_fail(ack_fd: c_int, stage: u32, error_number: c_int) -> ! {
    let result = ChildSetupResult {
        stage,
        error_number: if error_number == 0 { libc::EINVAL } else { error_number },
    };
    libc::write(ack_fd, addr_of!(result).cast(), size_of::<ChildSetupResult>());
    libc::_exit(126)
}

/// # Safety
///
/// `packet` must point to an initialized packet with matching role/fd counts.
unsafe fn descriptor_for_role(packet: *const Packet, role: i32) -> c_int {
    let mut index = 0;
    while index < (*packet).role_count {
        if (*packet).roles[index] == role {
            return (*packet).fds[index];
        }
        index = index.wrapping_add(1);
    }
    -1
}

/// # Safety
///
/// `packet` must contain validated, live namespace descriptors. Calling this
/// changes the current child process namespaces irreversibly.
unsafe fn enter_namespaces(packet: *const Packet) -> bool {
    let roles = [
        (ROLE_NAMESPACE_CGROUP, libc::CLONE_NEWCGROUP),
        (ROLE_NAMESPACE_IPC, libc::CLONE_NEWIPC),
        (ROLE_NAMESPACE_UTS, libc::CLONE_NEWUTS),
        (ROLE_NAMESPACE_NETWORK, libc::CLONE_NEWNET),
        (ROLE_NAMESPACE_TIME, libc::CLONE_NEWTIME),
        (ROLE_NAMESPACE_MOUNT, libc::CLONE_NEWNS),
    ];
    let mut index = 0;
    while index < roles.len() {
        let descriptor = descriptor_for_role(packet, roles[index].0);
        if descriptor >= 0 && libc::setns(descriptor, roles[index].1) != 0 {
            return false;
        }
        index = index.wrapping_add(1);
    }
    true
}

/// # Safety
///
/// `packet` must contain a validated live `cgroup.procs` descriptor. Calling
/// this moves the current child into that cgroup.
unsafe fn join_cgroup(packet: *const Packet) -> bool {
    let descriptor = descriptor_for_role(packet, ROLE_CGROUP_PROCS);
    if descriptor < 0 {
        return true;
    }
    let mut bytes = [0u8; 32];
    let mut value = libc::getpid() as u32;
    let mut start = bytes.len() - 1;
    bytes[start] = b'\n';
    while value != 0 {
        start = start.wrapping_sub(1);
        bytes[start] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    let bytes = &bytes[start..];
    loop {
        let written = libc::write(descriptor, bytes.as_ptr().cast(), bytes.len());
        if written == bytes.len() as isize {
            return true;
        }

        if written < 0 && errno() == libc::EINTR {
            continue;
        }
        return false;
    }
}

/// # Safety
///
/// `pid` must identify the child created for the current request and must not
/// be reaped elsewhere. This function consumes its wait status before return.
unsafe fn terminate_and_reap_child(pid: libc::pid_t) -> bool {
    if libc::kill(pid, libc::SIGKILL) != 0 && errno() != libc::ESRCH {
        return false;
    }

    loop {
        let waited = libc::waitpid(pid, null_mut(), 0);
        if waited == pid || (waited < 0 && errno() == libc::ECHILD) {
            return true;
        }
        if waited < 0 && errno() == libc::EINTR {
            continue;
        }
        return false;
    }
}

#[inline(never)]
/// Keeps the two-return process boundary opaque to whole-program optimization.
unsafe fn fork_process() -> libc::pid_t {
    libc::fork()
}

fn deadline_after_millis(now: libc::timespec, timeout_ms: c_int) -> libc::timespec {
    let seconds = timeout_ms / 1000;
    let nanoseconds = c_long::from(timeout_ms % 1000) * 1_000_000;
    let mut deadline = libc::timespec {
        tv_sec: now.tv_sec.saturating_add(seconds.into()),
        tv_nsec: now.tv_nsec.saturating_add(nanoseconds),
    };
    if deadline.tv_nsec >= 1_000_000_000 {
        deadline.tv_sec = deadline.tv_sec.saturating_add(1);
        deadline.tv_nsec -= 1_000_000_000;
    }
    deadline
}

fn remaining_timeout_millis(deadline: libc::timespec, now: libc::timespec) -> c_int {
    let seconds = i128::from(deadline.tv_sec) - i128::from(now.tv_sec);
    let nanoseconds = i128::from(deadline.tv_nsec) - i128::from(now.tv_nsec);
    let remaining = seconds.saturating_mul(1_000_000_000).saturating_add(nanoseconds);
    if remaining <= 0 {
        return 0;
    }
    let rounded_up = remaining.saturating_add(999_999) / 1_000_000;
    c_int::try_from(rounded_up).unwrap_or(c_int::MAX)
}

/// # Safety
///
/// `ack_fd` must be the live read end of the specialization pipe and `result`
/// writable. The child must retain the write end until success or exit.
unsafe fn read_child_setup_result_with_timeout(ack_fd: c_int, result: *mut ChildSetupResult, timeout_ms: c_int) -> isize {
    let mut now: libc::timespec = zeroed();
    if libc::clock_gettime(libc::CLOCK_MONOTONIC, addr_of_mut!(now)) != 0 {
        return -1;
    }
    let deadline = deadline_after_millis(now, timeout_ms.max(0));
    let mut descriptor = libc::pollfd {
        fd: ack_fd,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        if libc::clock_gettime(libc::CLOCK_MONOTONIC, addr_of_mut!(now)) != 0 {
            return -1;
        }
        let remaining = remaining_timeout_millis(deadline, now);
        if remaining == 0 {
            set_errno(libc::ETIMEDOUT);
            return -1;
        }
        let ready = libc::poll(addr_of_mut!(descriptor), 1, remaining);
        if ready < 0 && errno() == libc::EINTR {
            continue;
        }

        if ready == 0 {
            set_errno(libc::ETIMEDOUT);
            return -1;
        }
        if ready < 0 {
            return -1;
        }
        break;
    }
    loop {
        let size = libc::read(ack_fd, result.cast(), size_of::<ChildSetupResult>());
        if size < 0 && errno() == libc::EINTR {
            continue;
        }
        return size;
    }
}

#[cfg(feature = "private-test-util")]
/// Returns the file path carried by the test-only specialization stall marker.
///
/// # Safety
///
/// `launch.envp` must be a valid NUL-terminated environment vector.
unsafe fn specialization_stall_path(launch: *const Launch) -> *const c_char {
    let mut entry = (*launch).envp;
    while !(*entry).is_null() {
        let entry_len = libc::strlen(*entry);
        if entry_len >= SPECIALIZATION_STALL_ENV.len()
            && libc::memcmp(
                (*entry).cast(),
                SPECIALIZATION_STALL_ENV.as_ptr().cast(),
                SPECIALIZATION_STALL_ENV.len(),
            ) == 0
        {
            return (*entry).add(SPECIALIZATION_STALL_ENV.len());
        }
        entry = entry.add(1);
    }
    null()
}

#[cfg(feature = "private-test-util")]
/// Returns the shortened timeout when the test-only stall marker is active.
///
/// # Safety
///
/// `launch` must point to a fully decoded launch.
unsafe fn specialization_timeout_millis(launch: *const Launch) -> c_int {
    if !specialization_stall_path(launch).is_null() {
        return TEST_SPECIALIZATION_TIMEOUT_MILLIS;
    }
    SPECIALIZATION_TIMEOUT_MILLIS
}

#[cfg(not(feature = "private-test-util"))]
#[inline]
/// Returns the production specialization timeout.
///
/// # Safety
///
/// `launch` must point to a fully decoded launch.
unsafe fn specialization_timeout_millis(_: *const Launch) -> c_int {
    SPECIALIZATION_TIMEOUT_MILLIS
}

#[cfg(feature = "private-test-util")]
/// Records the child PID and stalls long enough for the parent deadline.
///
/// # Safety
///
/// `launch` must point to a fully decoded launch. The marker path, when
/// present, must remain a valid NUL-terminated string.
unsafe fn stall_specialization_for_test(launch: *const Launch) {
    let path = specialization_stall_path(launch);
    if path.is_null() {
        return;
    }
    let descriptor = libc::open(path, libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC, 0o600);
    if descriptor >= 0 {
        let mut digits = [0u8; 32];
        let mut value = libc::getpid() as u32;
        let mut start = digits.len();
        loop {
            start -= 1;
            digits[start] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        let _ = libc::write(descriptor, digits.as_ptr().add(start).cast(), digits.len() - start);
        libc::close(descriptor);
    }
    let _ = libc::poll(null_mut(), 0, TEST_SPECIALIZATION_STALL_MILLIS);
}

#[cfg(not(feature = "private-test-util"))]
#[inline]
/// Production builds contain no specialization-stall hook.
///
/// # Safety
///
/// `launch` must point to a fully decoded launch.
unsafe fn stall_specialization_for_test(_launch: *const Launch) {}

/// # Safety
///
/// Must run in the single-threaded child before application entry; it
/// irreversibly removes every capability available from the bounding set.
unsafe fn drop_capability_bounding_set() -> bool {
    let mut capability: c_int = 0;
    loop {
        let present = libc::prctl(libc::PR_CAPBSET_READ, capability, 0, 0, 0);
        if present < 0 {
            return errno() == libc::EINVAL;
        }
        if present != 0 && libc::prctl(libc::PR_CAPBSET_DROP, capability, 0, 0, 0) != 0 {
            return false;
        }
        if libc::prctl(libc::PR_CAPBSET_READ, capability, 0, 0, 0) != 0 {
            return false;
        }
        capability = capability.wrapping_add(1);
    }
}

#[repr(C)]
struct CapabilityHeader {
    version: u32,
    pid: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CapabilityData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

/// # Safety
///
/// Must run in the single-threaded child before application entry; the
/// capability syscalls mutate only the current process.
unsafe fn clear_capabilities() -> bool {
    const LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;
    let mut header = CapabilityHeader {
        version: LINUX_CAPABILITY_VERSION_3,
        pid: 0,
    };
    let data = [CapabilityData {
        effective: 0,
        permitted: 0,
        inheritable: 0,
    }; 2];
    if libc::syscall(libc::SYS_capset, addr_of_mut!(header), data.as_ptr()) != 0 {
        return false;
    }
    let mut capability: c_int = 0;
    while capability <= 63 {
        let result = libc::prctl(libc::PR_CAP_AMBIENT, libc::PR_CAP_AMBIENT_LOWER, capability, 0, 0);
        if result != 0 && errno() != libc::EINVAL {
            return false;
        }
        capability = capability.wrapping_add(1);
    }
    let mut observed = [CapabilityData {
        effective: u32::MAX,
        permitted: u32::MAX,
        inheritable: u32::MAX,
    }; 2];
    libc::syscall(libc::SYS_capget, addr_of_mut!(header), observed.as_mut_ptr()) == 0
        && observed
            .iter()
            .all(|set| set.effective == 0 && set.permitted == 0 && set.inheritable == 0)
}

/// # Safety
///
/// `launch` must point to a validated launch for the current child.
unsafe fn apply_privilege_reduction(launch: *const Launch) -> bool {
    if (*launch).clear_capabilities && !clear_capabilities() {
        return false;
    }
    if (*launch).no_new_privs
        && (libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 || libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) != 1)
    {
        return false;
    }

    if (*launch).disable_dumping
        && (libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) != 0 || libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0) != 0)
    {
        return false;
    }
    true
}

/// # Safety
///
/// Must run before credentials are finalized in the single-threaded child;
/// the securebits mutation is irreversible for that child.
unsafe fn lock_securebits() -> bool {
    const REQUIRED: c_int = 1 | 2 | 4 | 8 | 32 | 64 | 128;
    let current = libc::prctl(libc::PR_GET_SECUREBITS, 0, 0, 0, 0);
    if current < 0 {
        return false;
    }
    let requested = current | REQUIRED;
    libc::prctl(libc::PR_SET_SECUREBITS, requested, 0, 0, 0) == 0 && libc::prctl(libc::PR_GET_SECUREBITS, 0, 0, 0, 0) & REQUIRED == REQUIRED
}

/// # Safety
///
/// Both pointers must reference the same validated request and its live
/// Landlock ruleset descriptor. Restriction is irreversible for the child.
unsafe fn apply_landlock(packet: *const Packet, launch: *const Launch) -> bool {
    let descriptor = descriptor_for_role(packet, ROLE_LANDLOCK_RULESET);
    if descriptor < 0 {
        return true;
    }
    let abi = libc::syscall(
        libc::SYS_landlock_create_ruleset,
        null::<u8>(),
        0usize,
        LANDLOCK_CREATE_RULESET_VERSION,
    );
    abi >= c_long::from((*launch).landlock_abi)
        && (*launch).landlock_handled_access != 0
        && libc::syscall(libc::SYS_landlock_restrict_self, descriptor, 0u32) == 0
}

/// # Safety
///
/// `launch` must contain a validated filter array that remains live through
/// the syscall. Installing the filter is irreversible for the child.
unsafe fn install_seccomp(launch: *const Launch) -> bool {
    if (*launch).seccomp_count == 0 {
        return true;
    }
    let Ok(length) = u16::try_from((*launch).seccomp_count) else {
        return false;
    };
    let program = libc::sock_fprog {
        len: length,
        filter: (*launch).seccomp,
    };
    libc::syscall(libc::SYS_seccomp, SECCOMP_SET_MODE_FILTER, 0u32, addr_of!(program)) == 0
}

/// Runs all child-only specialization and enters the application.
///
/// # Safety
///
/// This must be called only in the freshly forked, single-threaded child.
/// `packet` and `launch` must describe the same validated request; all
/// descriptors and arena-backed pointers must remain live until `entry`
/// returns. Every failure path reports through `ack_fd` and exits without
/// returning into parent-owned control flow.
unsafe fn run_child(
    control_fd: c_int,
    signal_fd: c_int,
    ack_fd: c_int,
    packet: *mut Packet,
    launch: *mut Launch,
    entry: Entry,
    context: *mut c_void,
) -> ! {
    libc::close(control_fd);
    libc::close(signal_fd);
    if fault(Fault::ChildSetup) || !install_stdio((*packet).fds.as_ptr()) {
        child_fail(ack_fd, 200, if fault(Fault::ChildSetup) { libc::EIO } else { errno() });
    }
    preserve_installed_stdio(packet);
    if !enter_namespaces(packet) {
        child_fail(ack_fd, 201, errno());
    }
    if !join_cgroup(packet) {
        child_fail(ack_fd, 202, errno());
    }
    if !(*launch).cwd.is_null() && libc::chdir((*launch).cwd) != 0 {
        child_fail(ack_fd, 203, errno());
    }
    if (*launch).new_session && libc::setsid() < 0 {
        child_fail(ack_fd, 204, errno());
    }
    if (*launch).has_process_group && libc::setpgid(0, (*launch).process_group) != 0 {
        child_fail(ack_fd, 204, errno());
    }
    if (*launch).has_umask {
        libc::umask((*launch).umask_value);
    }
    let mut index = 0;
    while index < (*launch).limit_count {
        let limit = (*launch).limits.add(index);
        if libc::setrlimit((*limit).resource as c_uint, addr_of!((*limit).value)) != 0 {
            child_fail(ack_fd, 205, errno());
        }
        index = index.wrapping_add(1);
    }
    if (*launch).drop_capability_bounding_set && !drop_capability_bounding_set() {
        child_fail(ack_fd, 206, errno());
    }
    if (*launch).lock_securebits && !lock_securebits() {
        child_fail(ack_fd, 206, errno());
    }
    if (*launch).has_groups {
        if libc::setgroups((*launch).group_count, (*launch).groups) != 0 {
            child_fail(ack_fd, 207, errno());
        }
    } else if (*launch).has_uid && libc::setgroups(0, null()) != 0 {
        child_fail(ack_fd, 207, errno());
    }
    if (*launch).has_gid && libc::setgid((*launch).gid) != 0 {
        child_fail(ack_fd, 207, errno());
    }
    if (*launch).has_uid && libc::setuid((*launch).uid) != 0 {
        child_fail(ack_fd, 207, errno());
    }
    if !apply_privilege_reduction(launch) || !apply_landlock(packet, launch) {
        child_fail(ack_fd, 208, errno());
    }
    close_packet_fds(packet);
    environ = (*launch).envp;
    if RESET_CHILD_SIGNALS && !normalize_signal_dispositions() {
        child_fail(ack_fd, 209, errno());
    }
    if !disable_alternate_signal_stack() {
        child_fail(ack_fd, 209, errno());
    }
    let mut empty: libc::sigset_t = zeroed();
    if libc::sigemptyset(addr_of_mut!(empty)) != 0 || libc::sigprocmask(libc::SIG_SETMASK, addr_of!(empty), null_mut()) != 0 {
        child_fail(ack_fd, 209, errno());
    }
    let mut ack = ack_fd;
    if ack != 3 {
        if libc::dup3(ack, 3, libc::O_CLOEXEC) != 3 {
            child_fail(ack, 210, errno());
        }
        libc::close(ack);
        ack = 3;
    }
    if !close_above_acknowledgement() {
        child_fail(ack, 211, errno());
    }
    close_inherited_closed_stdio(packet);
    stall_specialization_for_test(launch);
    let seccomp_fault = fault(Fault::Seccomp);
    if seccomp_fault || !install_seccomp(launch) {
        child_fail(ack, 212, if seccomp_fault { libc::EINVAL } else { errno() });
    }
    let success = ChildSetupResult { stage: 0, error_number: 0 };
    if libc::write(ack, addr_of!(success).cast(), size_of::<ChildSetupResult>()) != size_of::<ChildSetupResult>() as isize {
        libc::_exit(126);
    }
    if libc::close(ack) != 0 {
        libc::_exit(126);
    }
    let status = entry(context, (*launch).argc, (*launch).argv);
    libc::_exit(status)
}

/// # Safety
///
/// `children` must point to `CHILD_TABLE_SIZE` initialized slots and
/// `child_count` must be exclusively writable and equal to table occupancy.
unsafe fn add_child(children: *mut Child, child_count: *mut usize, pid: libc::pid_t, request_id: u64) -> bool {
    if *child_count >= MAX_ITEM_COUNT {
        return false;
    }
    let mut index = 0;
    while index < *child_count {
        let child = children.add(index);
        if (*child).pid == pid {
            return false;
        }
        index = index.wrapping_add(1);
    }
    *children.add(*child_count) = Child { pid, request_id };
    *child_count = (*child_count).wrapping_add(1);
    true
}

/// # Safety
///
/// `children` and `child_count` must satisfy [`add_child`]'s table invariant.
unsafe fn remove_child(children: *mut Child, child_count: *mut usize, pid: libc::pid_t) -> u64 {
    let mut index = 0;
    while index < *child_count {
        let child = children.add(index);
        if (*child).pid == pid {
            let request_id = (*child).request_id;
            *child_count = (*child_count).wrapping_sub(1);
            *child = *children.add(*child_count);
            *children.add(*child_count) = Child { pid: 0, request_id: 0 };
            return request_id;
        }
        index = index.wrapping_add(1);
    }
    0
}

/// # Safety
///
/// `fd` must be the live control socket and the child table/count must be
/// exclusively owned by this event loop. Each reaped PID is removed once.
unsafe fn reap_children(fd: c_int, children: *mut Child, child_count: *mut usize) -> ReapResult {
    let mut index = 0;
    while index < *child_count {
        let pid = (*children.add(index)).pid;
        let mut status = 0;
        let waited = loop {
            let waited = libc::waitpid(pid, addr_of_mut!(status), libc::WNOHANG);
            if waited < 0 && errno() == libc::EINTR {
                continue;
            }
            break waited;
        };
        if waited < 0 {
            return ReapResult::Failed;
        }
        if waited == pid {
            let request_id = remove_child(children, child_count, pid);
            let values = [status as u64];
            let mut body = [0u8; 16];
            let Some(len) = encode_varint_body(values.as_ptr(), 1, body.as_mut_ptr(), body.len()) else {
                return ReapResult::Failed;
            };
            if send_body(fd, request_id, BODY_EXITED, body.as_ptr(), len, 0, -1) != 0 {
                return ReapResult::Detached;
            }
            continue;
        }
        index = index.wrapping_add(1);
    }
    ReapResult::Continue
}

/// Returns a monotonic timestamp in nanoseconds, or zero if unavailable.
unsafe fn monotonic_nanoseconds() -> u64 {
    let mut time: libc::timespec = zeroed();
    if libc::clock_gettime(libc::CLOCK_MONOTONIC, addr_of_mut!(time)) != 0 {
        return 0;
    }
    let seconds = u64::try_from(time.tv_sec).unwrap_or(0);
    let nanoseconds = u64::try_from(time.tv_nsec).unwrap_or(0);
    seconds.saturating_mul(1_000_000_000).saturating_add(nanoseconds)
}

/// Runs one dormant worker after the template assigns its single request.
///
/// # Safety
///
/// `socket` is the worker's private sequenced-packet endpoint and
/// `entry/context` satisfy the zygote entry contract.
unsafe fn run_prefork_worker(socket: c_int, entry: Entry, context: *mut c_void) -> ! {
    let buffer = libc::calloc(MAX_PACKET_LEN, 1).cast::<u8>();
    if buffer.is_null() {
        libc::_exit(126);
    }
    let mut packet = Packet::empty();
    let mut buffer_high_water = 0;
    let received = receive_packet(socket, buffer, addr_of_mut!(packet));
    if received <= 0 || packet.kind != BODY_LAUNCH {
        close_packet_fds(addr_of_mut!(packet));
        secure_zero(buffer.cast(), MAX_PACKET_LEN);
        libc::free(buffer.cast());
        libc::_exit(126);
    }
    let mut launch = Launch::empty();
    let mut arena = null_mut();
    let mut capacity = 0;
    if !parse_launch(addr_of!(packet), &mut launch, &mut arena, &mut capacity) {
        let failure = ChildSetupResult {
            stage: 1,
            error_number: libc::EINVAL,
        };
        let _ = libc::write(socket, addr_of!(failure).cast(), size_of::<ChildSetupResult>());
        close_packet_fds(addr_of_mut!(packet));
        if !arena.is_null() {
            secure_zero(arena, capacity);
            libc::free(arena);
        }
        clear_received_packet(buffer, received, &mut buffer_high_water);
        libc::free(buffer.cast());
        libc::_exit(126);
    }
    clear_received_packet(buffer, received, &mut buffer_high_water);
    libc::free(buffer.cast());
    run_child(-1, -1, socket, addr_of_mut!(packet), addr_of_mut!(launch), entry, context);
}

/// Adds one dormant worker to `pool`.
///
/// # Safety
///
/// The template is single-threaded, owns every listed descriptor, and
/// `entry/context` remain valid in the forked worker.
unsafe fn create_prefork_worker(control_fd: c_int, signal_fd: c_int, pool: &mut PreforkPool, entry: Entry, context: *mut c_void) -> bool {
    if pool.count >= pool.config.max_idle || pool.count >= MAX_PREFORK_WORKERS {
        return true;
    }
    let mut sockets = [-1; 2];
    if libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0, sockets.as_mut_ptr()) != 0 {
        return false;
    }
    let pid = fork_process();
    if pid < 0 {
        libc::close(sockets[0]);
        libc::close(sockets[1]);
        return false;
    }
    if pid == 0 {
        libc::close(sockets[0]);
        libc::close(control_fd);
        libc::close(signal_fd);
        let mut index = 0;
        while index < pool.count {
            libc::close(pool.workers[index].socket);
            index = index.wrapping_add(1);
        }
        run_prefork_worker(sockets[1], entry, context);
    }
    libc::close(sockets[1]);
    pool.workers[pool.count] = IdleWorker { pid, socket: sockets[0] };
    pool.count = pool.count.wrapping_add(1);
    true
}

/// Replenishes dormant workers to `target`.
///
/// # Safety
///
/// Satisfies [`create_prefork_worker`]'s template-state contract.
unsafe fn refill_prefork_pool(
    control_fd: c_int,
    signal_fd: c_int,
    pool: &mut PreforkPool,
    target: usize,
    entry: Entry,
    context: *mut c_void,
    record: bool,
) -> bool {
    let target = target.min(pool.config.max_idle);
    let started = monotonic_nanoseconds();
    let before = pool.count;
    while pool.count < target {
        if !create_prefork_worker(control_fd, signal_fd, pool, entry, context) {
            return false;
        }
    }
    if record && pool.count > before {
        pool.refills = pool.refills.saturating_add((pool.count - before) as u64);
        let finished = monotonic_nanoseconds();
        pool.refill_nanoseconds = pool.refill_nanoseconds.saturating_add(finished.saturating_sub(started));
    }
    true
}

/// Reports the pool state after a completed refill, trim, or reap operation.
///
/// # Safety
///
/// `control_fd` must be the live controller socket and `pool` must be valid.
unsafe fn send_pool_state(control_fd: c_int, pool: &PreforkPool) -> bool {
    let values = [pool.count as u64, pool.refills, pool.refill_nanoseconds];
    let mut body = [0u8; 48];
    let Some(len) = encode_varint_body(values.as_ptr(), values.len(), body.as_mut_ptr(), body.len()) else {
        return false;
    };
    send_body(control_fd, 0, BODY_POOL_STATE, body.as_ptr(), len, 0, -1) == 0
}

/// Removes exited dormant workers and restores the immediate minimum.
///
/// # Safety
///
/// `pool` exclusively owns each listed worker and socket.
unsafe fn reap_prefork_workers(control_fd: c_int, signal_fd: c_int, pool: &mut PreforkPool, entry: Entry, context: *mut c_void) -> bool {
    let mut index = 0;
    while index < pool.count {
        let waited = libc::waitpid(pool.workers[index].pid, null_mut(), libc::WNOHANG);
        if waited == pool.workers[index].pid {
            libc::close(pool.workers[index].socket);
            pool.count = pool.count.wrapping_sub(1);
            pool.workers[index] = pool.workers[pool.count];
            pool.workers[pool.count] = IdleWorker::empty();
            continue;
        }
        if waited < 0 && errno() != libc::EINTR {
            return false;
        }
        index = index.wrapping_add(1);
    }
    refill_prefork_pool(control_fd, signal_fd, pool, pool.config.min_idle, entry, context, true) && send_pool_state(control_fd, pool)
}

/// Terminates every dormant worker and closes its private channel.
///
/// # Safety
///
/// `pool` exclusively owns every listed PID and descriptor.
unsafe fn destroy_prefork_pool(pool: &mut PreforkPool) {
    while pool.count != 0 {
        pool.count = pool.count.wrapping_sub(1);
        let worker = pool.workers[pool.count];
        libc::close(worker.socket);
        let _ = terminate_and_reap_child(worker.pid);
        pool.workers[pool.count] = IdleWorker::empty();
    }
}

/// Decodes the requested idle-worker target from a pool-control body.
///
/// # Safety
///
/// `body` must be readable for `body_len` bytes.
unsafe fn decode_pool_target(body: *const u8, body_len: usize) -> Option<usize> {
    let mut cursor = Cursor::new(body, body_len)?;
    let mut target = None;
    while cursor.remaining() != 0 {
        let key = cursor.varint()?;
        let field = key.wrapping_shr(3);
        let wire = (key & 7) as u8;
        if field == 1 {
            if wire != 0 || target.is_some() {
                return None;
            }
            target = usize::try_from(cursor.varint()?).ok();
        } else if !cursor.skip(wire) {
            return None;
        }
    }
    target.or(Some(0))
}

/// Applies an explicit dormant-worker target.
///
/// # Safety
///
/// `pool` exclusively owns every idle worker and the template remains
/// single-threaded.
unsafe fn control_prefork_pool(
    control_fd: c_int,
    signal_fd: c_int,
    pool: &mut PreforkPool,
    target: usize,
    entry: Entry,
    context: *mut c_void,
) -> bool {
    if target > pool.maximum_capacity {
        return false;
    }
    while pool.count > target {
        pool.count = pool.count.wrapping_sub(1);
        let worker = pool.workers[pool.count];
        libc::close(worker.socket);
        if !terminate_and_reap_child(worker.pid) {
            return false;
        }
        pool.workers[pool.count] = IdleWorker::empty();
    }
    pool.config.min_idle = target;
    pool.config.max_idle = target;
    pool.config.refill_threshold = target;
    pool.refill_deadline_millis = -1;
    refill_prefork_pool(control_fd, signal_fd, pool, target, entry, context, true) && send_pool_state(control_fd, pool)
}

/// Handles one launch request in the template.
///
/// # Safety
///
/// All descriptors and pointers must belong exclusively to the single-threaded
/// zygote loop. `packet` must own its received descriptors; `children` must be
/// a valid table; `untracked_child` must be exclusively writable; and the arena
/// pair must retain ownership on every early return. A successful return leaves
/// the child table responsible for reaping.
unsafe fn handle_launch(
    fd: c_int,
    signal_fd: c_int,
    single_threaded: &SingleThreaded,
    children: *mut Child,
    child_count: *mut usize,
    untracked_child: *mut libc::pid_t,
    packet: *mut Packet,
    entry: Entry,
    context: *mut c_void,
    arena: &mut *mut c_void,
    capacity: &mut usize,
    packet_bytes: *const u8,
    packet_len: usize,
    pool: &mut PreforkPool,
) -> c_int {
    let mut launch = Launch::empty();
    if fault(Fault::Parse) || (*packet).fd_count != (*packet).role_count || !parse_launch(packet, &mut launch, arena, capacity) {
        free_launch(addr_of_mut!(launch));
        close_packet_fds(packet);
        return send_error(fd, (*packet).request_id, 1, libc::EINVAL as u32);
    }
    if fault(Fault::ThreadCheck) || !single_threaded.is_current() {
        free_launch(addr_of_mut!(launch));
        close_packet_fds(packet);
        return send_error(fd, (*packet).request_id, 2, libc::EBUSY as u32);
    }
    let specialization_timeout = specialization_timeout_millis(addr_of!(launch));
    let mut pid = -1;
    let mut acknowledgement = -1;
    let mut prefork_hit = false;
    if pool.count != 0 {
        pool.count = pool.count.wrapping_sub(1);
        let worker = pool.workers[pool.count];
        pool.workers[pool.count] = IdleWorker::empty();
        if forward_packet(worker.socket, packet_bytes, packet_len, packet) {
            pid = worker.pid;
            acknowledgement = worker.socket;
            prefork_hit = true;
        } else {
            libc::close(worker.socket);
            let _ = terminate_and_reap_child(worker.pid);
        }
    }
    if !prefork_hit {
        let mut ack = [-1; 2];
        if fault(Fault::AckPipe) || libc::pipe2(ack.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            let error = if fault(Fault::AckPipe) { libc::EMFILE } else { errno() };
            free_launch(addr_of_mut!(launch));
            close_packet_fds(packet);
            return send_error(fd, (*packet).request_id, 3, error as u32);
        }
        pid = if fault(Fault::Fork) {
            set_errno(libc::EAGAIN);
            -1
        } else {
            fork_process()
        };
        if pid < 0 {
            let error = errno();
            libc::close(ack[0]);
            libc::close(ack[1]);
            free_launch(addr_of_mut!(launch));
            close_packet_fds(packet);
            return send_error(fd, (*packet).request_id, 4, error as u32);
        }
        if pid == 0 {
            libc::close(ack[0]);
            run_child(fd, signal_fd, ack[1], packet, addr_of_mut!(launch), entry, context);
        }
        libc::close(ack[1]);
        acknowledgement = ack[0];
    }
    close_packet_fds(packet);
    free_launch(addr_of_mut!(launch));
    if fault(Fault::ChildTable) || !add_child(children, child_count, pid, (*packet).request_id) {
        libc::close(acknowledgement);
        if !terminate_and_reap_child(pid) {
            *untracked_child = pid;
            return -1;
        }
        return send_error(fd, (*packet).request_id, 5, libc::EUSERS as u32);
    }
    let pidfd = if fault(Fault::Pidfd) {
        set_errno(libc::ENOSYS);
        -1
    } else {
        c_int::try_from(libc::syscall(libc::SYS_pidfd_open, pid, 0u32)).unwrap_or(-1)
    };
    if pidfd < 0 {
        let error = errno();
        libc::close(acknowledgement);
        if !terminate_and_reap_child(pid) {
            return -1;
        }
        remove_child(children, child_count, pid);
        return send_error(fd, (*packet).request_id, 6, error as u32);
    }
    let mut child_result = ChildSetupResult { stage: 0, error_number: 0 };
    let ack_size = if fault(Fault::AckRead) {
        set_errno(libc::ETIMEDOUT);
        -1
    } else {
        read_child_setup_result_with_timeout(acknowledgement, addr_of_mut!(child_result), specialization_timeout)
    };
    let ack_error = if ack_size < 0 { errno() } else { 0 };
    libc::close(acknowledgement);
    if ack_size != size_of::<ChildSetupResult>() as isize || child_result.error_number != 0 {
        let (stage, error) = if ack_size == size_of::<ChildSetupResult>() as isize {
            (child_result.stage, child_result.error_number)
        } else {
            (7, if ack_error == 0 { libc::ECHILD } else { ack_error })
        };
        if !terminate_and_reap_child(pid) {
            libc::close(pidfd);
            return -1;
        }
        remove_child(children, child_count, pid);
        libc::close(pidfd);
        return send_error(fd, (*packet).request_id, stage, error as u32);
    }
    let values = [
        pid as u64,
        u64::from(prefork_hit),
        pool.count as u64,
        pool.refills,
        pool.refill_nanoseconds,
    ];
    let mut body = [0u8; 64];
    let Some(len) = encode_varint_body(values.as_ptr(), values.len(), body.as_mut_ptr(), body.len()) else {
        libc::close(pidfd);
        if terminate_and_reap_child(pid) {
            remove_child(children, child_count, pid);
        }
        return -1;
    };
    let result = send_body(fd, (*packet).request_id, BODY_STARTED, body.as_ptr(), len, ROLE_PIDFD, pidfd);
    libc::close(pidfd);
    if result != 0 {
        if terminate_and_reap_child(pid) {
            remove_child(children, child_count, pid);
        }
        return -1;
    }
    if pool.count < pool.config.min_idle {
        let _ = refill_prefork_pool(fd, signal_fd, pool, pool.config.min_idle, entry, context, true);
        if !send_pool_state(fd, pool) {
            return -1;
        }
    }
    if pool.count <= pool.config.refill_threshold && pool.count < pool.config.max_idle {
        if pool.config.refill_delay_millis == 0 {
            let _ = refill_prefork_pool(fd, signal_fd, pool, pool.config.max_idle, entry, context, true);
            if !send_pool_state(fd, pool) {
                return -1;
            }
        } else {
            pool.refill_deadline_millis = i64::try_from(monotonic_nanoseconds() / 1_000_000)
                .unwrap_or(i64::MAX)
                .saturating_add(i64::from(pool.config.refill_delay_millis));
        }
    }
    0
}

/// # Safety
///
/// `fd` must be an open descriptor. This function only observes its metadata.
unsafe fn validate_control_fd(fd: c_int) -> bool {
    let mut socket_type = 0;
    let mut length = size_of::<c_int>() as libc::socklen_t;
    let mut status: libc::stat = zeroed();
    fd == CONTROL_FD
        && libc::fstat(fd, addr_of_mut!(status)) == 0
        && status.st_mode & libc::S_IFMT == libc::S_IFSOCK
        && libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            addr_of_mut!(socket_type).cast(),
            addr_of_mut!(length),
        ) == 0
        && socket_type == libc::SOCK_SEQPACKET
}

/// Runs the single-threaded zygote event loop.
///
/// # Safety
///
/// `fd` must be the authenticated control socket, `nonce` writable for 33
/// bytes, and `entry/context` must remain valid for every child. No other
/// thread may access the global signal or prepared-entry state. This function
/// owns every allocation and descriptor it creates and releases them through
/// `goto_cleanup` on all exits.
unsafe fn run_zygote(fd: c_int, nonce: *mut c_char, entry: Entry, context: *mut c_void) -> c_int {
    let mut arena = null_mut();
    let mut capacity = 0;
    let mut child_count = 0;
    let mut untracked_child = 0;
    let mut result = 125;
    let mut prefork_pool = PreforkPool::new(PREFORK_CONFIG);
    let Some(single_threaded) = SingleThreaded::establish() else {
        send_error(fd, 0, 100, libc::EBUSY as u32);
        return result;
    };
    if fault(Fault::TemplateState) {
        send_error(fd, 0, 101, libc::EIO as u32);
        return result;
    }
    if !template_has_only_expected_fds(fd) {
        send_error(fd, 0, 102, libc::EBUSY as u32);
        return result;
    }
    if !template_has_no_writable_shared_mappings() {
        send_error(fd, 0, 103, libc::EBUSY as u32);
        return result;
    }
    if !prepare_signal_reset_plan() {
        let error = errno();
        send_error(fd, 0, 104, if error == 0 { libc::EINVAL } else { error } as u32);
        return result;
    }
    let mut child_action: libc::sigaction = zeroed();
    child_action.sa_sigaction = libc::SIG_DFL;
    if libc::sigemptyset(addr_of_mut!(child_action.sa_mask)) != 0 || libc::sigaction(libc::SIGCHLD, addr_of!(child_action), null_mut()) != 0
    {
        send_error(fd, 0, 104, errno() as u32);
        return result;
    }
    let mut child_signal: libc::sigset_t = zeroed();
    if fault(Fault::SignalSetup)
        || libc::sigemptyset(addr_of_mut!(child_signal)) != 0
        || libc::sigaddset(addr_of_mut!(child_signal), libc::SIGCHLD) != 0
        || libc::sigprocmask(libc::SIG_BLOCK, addr_of!(child_signal), null_mut()) != 0
    {
        send_error(fd, 0, 105, if fault(Fault::SignalSetup) { libc::EIO } else { errno() } as u32);
        return result;
    }
    let signal_fd = libc::signalfd(-1, addr_of!(child_signal), libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
    if signal_fd < 0 {
        send_error(fd, 0, 106, errno() as u32);
        return result;
    }
    let buffer = libc::calloc(MAX_PACKET_LEN, 1).cast::<u8>();
    let mut buffer_high_water = 0;
    let children = libc::calloc(CHILD_TABLE_SIZE, size_of::<Child>()).cast::<Child>();
    if fault(Fault::Allocation) || buffer.is_null() || children.is_null() {
        send_error(fd, 0, 107, libc::ENOMEM as u32);
        goto_cleanup(fd, signal_fd, buffer, children, arena, capacity, 0, true);
        return result;
    }
    let mut ready_body = [0u8; 64];
    let mut ready_encoder = Encoder::new();
    if !ready_encoder.field_bytes(1, nonce.cast(), 32) {
        goto_cleanup(fd, signal_fd, buffer, children, arena, capacity, 0, true);
        return result;
    }
    libc::memcpy(
        ready_body.as_mut_ptr().cast(),
        ready_encoder.bytes.as_ptr().cast(),
        ready_encoder.len,
    );
    secure_zero(nonce.cast(), 33);
    let initial_idle = prefork_pool.config.min_idle;
    if !refill_prefork_pool(fd, signal_fd, &mut prefork_pool, initial_idle, entry, context, false) {
        destroy_prefork_pool(&mut prefork_pool);
        goto_cleanup(fd, signal_fd, buffer, children, arena, capacity, 0, true);
        return result;
    }
    if fault(Fault::ReadySend) || send_body(fd, 0, BODY_READY, ready_body.as_ptr(), ready_encoder.len, 0, -1) != 0 {
        destroy_prefork_pool(&mut prefork_pool);
        goto_cleanup(fd, signal_fd, buffer, children, arena, capacity, 0, true);
        return result;
    }
    loop {
        let mut descriptors = [
            libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: signal_fd,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let ready = loop {
            let timeout = if prefork_pool.refill_deadline_millis < 0 {
                -1
            } else {
                let now = i64::try_from(monotonic_nanoseconds() / 1_000_000).unwrap_or(i64::MAX);
                c_int::try_from(prefork_pool.refill_deadline_millis.saturating_sub(now).max(0)).unwrap_or(c_int::MAX)
            };
            let value = libc::poll(descriptors.as_mut_ptr(), 2, timeout);
            if value < 0 && errno() == libc::EINTR {
                continue;
            }
            break value;
        };
        if ready < 0 {
            break;
        }
        if ready == 0 && prefork_pool.refill_deadline_millis >= 0 {
            prefork_pool.refill_deadline_millis = -1;
            let maximum_idle = prefork_pool.config.max_idle;
            if !refill_prefork_pool(fd, signal_fd, &mut prefork_pool, maximum_idle, entry, context, true) {
                break;
            }
            if !send_pool_state(fd, &prefork_pool) {
                break;
            }
            continue;
        }
        if descriptors[1].revents & libc::POLLIN != 0 {
            let mut info: libc::signalfd_siginfo = zeroed();
            while libc::read(signal_fd, addr_of_mut!(info).cast(), size_of::<libc::signalfd_siginfo>())
                == size_of::<libc::signalfd_siginfo>() as isize
            {}
            if !reap_prefork_workers(fd, signal_fd, &mut prefork_pool, entry, context) {
                break;
            }
            match reap_children(fd, children, addr_of_mut!(child_count)) {
                ReapResult::Continue => {}
                ReapResult::Detached => {
                    result = 0;
                    break;
                }
                ReapResult::Failed => break,
            }
        }
        if descriptors[0].revents & libc::POLLIN != 0 {
            let mut packet = Packet::empty();
            let received = receive_packet(fd, buffer, addr_of_mut!(packet));
            if received == 0 {
                result = 0;
                break;
            }
            if received < 0 {
                break;
            }
            if packet.kind == BODY_SHUTDOWN {
                clear_received_packet(buffer, received, &mut buffer_high_water);
                result = 0;
                break;
            }
            if packet.kind == BODY_POOL_CONTROL {
                let target = decode_pool_target(packet.body, packet.body_len);
                clear_received_packet(buffer, received, &mut buffer_high_water);
                if target.is_none() || !control_prefork_pool(fd, signal_fd, &mut prefork_pool, target.unwrap_or(0), entry, context) {
                    break;
                }
                continue;
            }
            if packet.kind != BODY_LAUNCH {
                close_packet_fds(addr_of_mut!(packet));
                clear_received_packet(buffer, received, &mut buffer_high_water);
                if send_error(fd, packet.request_id, 0, libc::EPROTO as u32) != 0 {
                    break;
                }
                continue;
            }
            if handle_launch(
                fd,
                signal_fd,
                &single_threaded,
                children,
                addr_of_mut!(child_count),
                addr_of_mut!(untracked_child),
                addr_of_mut!(packet),
                entry,
                context,
                &mut arena,
                &mut capacity,
                buffer,
                received as usize,
                &mut prefork_pool,
            ) != 0
            {
                clear_received_packet(buffer, received, &mut buffer_high_water);
                break;
            }
            clear_received_packet(buffer, received, &mut buffer_high_water);
        }
        if descriptors[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            result = 0;
            break;
        }
    }
    destroy_prefork_pool(&mut prefork_pool);
    goto_cleanup(fd, signal_fd, buffer, children, arena, capacity, untracked_child, result != 0);
    result
}

/// # Safety
///
/// Each non-null allocation must have been returned by `calloc`, `capacity`
/// must describe `arena`, and each nonnegative descriptor must still be owned
/// by the zygote. When `terminate_children` is true, every occupied child-table
/// entry and any positive `untracked_child` must still identify a child owned
/// by this process. This consumes all listed resources.
unsafe fn goto_cleanup(
    fd: c_int,
    signal_fd: c_int,
    buffer: *mut u8,
    children: *mut Child,
    arena: *mut c_void,
    capacity: usize,
    untracked_child: libc::pid_t,
    terminate_children: bool,
) {
    if !buffer.is_null() {
        secure_zero(buffer.cast(), MAX_PACKET_LEN);
        libc::free(buffer.cast());
    }
    if !children.is_null() {
        if terminate_children {
            if untracked_child > 0 {
                let _ = terminate_and_reap_child(untracked_child);
            }
            let mut index = 0;
            while index < CHILD_TABLE_SIZE {
                let pid = (*children.add(index)).pid;
                if pid > 0 {
                    let _ = terminate_and_reap_child(pid);
                }
                index = index.wrapping_add(1);
            }
        }
        libc::free(children.cast());
    }
    if !arena.is_null() {
        secure_zero(arena, capacity);
        libc::free(arena);
    }
    if signal_fd >= 0 {
        libc::close(signal_fd);
    }
    libc::close(fd);
}

/// # Safety
///
/// `name` and every `environ` entry must be valid NUL-terminated strings.
/// Matching storage must be writable for its current string length.
unsafe fn wipe_environment_entry(name: *const c_char) {
    let name_len = libc::strlen(name);
    let mut entry = environ;
    while !entry.is_null() && !(*entry).is_null() {
        let entry_len = libc::strlen(*entry);
        if entry_len > name_len && libc::strncmp(*entry, name, name_len) == 0 && *(*entry).add(name_len) == byte_as_c_char(b'=') {
            secure_zero((*entry).cast(), libc::strlen(*entry));
            return;
        }
        entry = entry.add(1);
    }
}

/// # Safety
///
/// `argc/argv` must be the valid C startup arguments supplied to `main`.
unsafe extern "C-unwind" fn real_main_entry(_: *mut c_void, argc: c_int, argv: *mut *mut c_char) -> c_int {
    __real_main(argc, argv)
}

/// # Safety
///
/// `name` must point to a NUL-terminated environment-variable name.
unsafe fn environment_usize(name: *const c_char, maximum: usize) -> Option<usize> {
    let value = libc::getenv(name);
    if value.is_null() {
        return None;
    }
    let mut end = null_mut();
    set_errno(0);
    let parsed = libc::strtoul(value, addr_of_mut!(end), 10);
    if errno() != 0 || !parsed_entire_c_string(value, end) {
        return None;
    }
    usize::try_from(parsed).ok().filter(|parsed| *parsed <= maximum)
}

/// # Safety
///
/// The bootstrap environment must remain live for all four lookups.
unsafe fn read_prefork_config() -> Option<PreforkConfig> {
    let min_idle = environment_usize(PREFORK_MIN_IDLE_ENV.as_ptr(), MAX_PREFORK_WORKERS)?;
    let max_idle = environment_usize(PREFORK_MAX_IDLE_ENV.as_ptr(), MAX_PREFORK_WORKERS)?;
    let refill_threshold = environment_usize(PREFORK_REFILL_THRESHOLD_ENV.as_ptr(), MAX_PREFORK_WORKERS)?;
    let refill_delay = environment_usize(PREFORK_REFILL_DELAY_ENV.as_ptr(), c_int::MAX as usize)?;
    if min_idle > refill_threshold || refill_threshold > max_idle {
        return None;
    }
    Some(PreforkConfig {
        min_idle,
        max_idle,
        refill_threshold,
        refill_delay_millis: refill_delay as c_int,
    })
}

/// Consumes and clears the bootstrap environment.
///
/// Returns zero for direct execution, one for an authenticated zygote launch,
/// and minus one for malformed bootstrap state.
unsafe fn consume_bootstrap(fd: *mut c_int, nonce_copy: *mut c_char) -> c_int {
    let fd_text = libc::getenv(CONTROL_FD_ENV.as_ptr());
    let nonce = libc::getenv(NONCE_ENV.as_ptr());
    if fd_text.is_null() && nonce.is_null() {
        return 0;
    }
    if fd_text.is_null() || nonce.is_null() || libc::strlen(nonce) != 32 {
        return -1;
    }
    let Some(prefork_config) = read_prefork_config() else {
        return -1;
    };
    let mut end = null_mut();
    set_errno(0);
    let raw_fd = libc::strtol(fd_text, addr_of_mut!(end), 10);
    let parsed_fd = c_int::try_from(raw_fd).ok();
    if errno() != 0
        || !parsed_entire_c_string(fd_text, end)
        || raw_fd > c_long::from(c_int::MAX)
        || parsed_fd.is_none()
        || !validate_control_fd(parsed_fd.unwrap_or(-1))
    {
        return -1;
    }
    *fd = parsed_fd.unwrap_or(-1);
    PREFORK_CONFIG = prefork_config;
    libc::memcpy(nonce_copy.cast(), nonce.cast(), 33);
    wipe_environment_entry(CONTROL_FD_ENV.as_ptr());
    wipe_environment_entry(NONCE_ENV.as_ptr());
    if libc::clearenv() != 0 {
        secure_zero(nonce_copy.cast(), 33);
        return -1;
    }
    1
}

/// # Safety
///
/// `argc/argv` and `entry/context` must form a valid application entry call.
/// If zygote markers are present, this function assumes single-threaded
/// pre-runtime ownership and consumes the authenticated control descriptor.
unsafe fn dispatch(argc: c_int, argv: *mut *mut c_char, entry: Entry, context: *mut c_void) -> c_int {
    let mut fd = -1;
    let mut nonce_copy = [0; 33];
    let bootstrap = consume_bootstrap(addr_of_mut!(fd), nonce_copy.as_mut_ptr());
    if bootstrap == 0 {
        return entry(context, argc, argv);
    }
    if bootstrap < 0 {
        return 125;
    }
    let result = run_zygote(fd, nonce_copy.as_mut_ptr(), entry, context);
    secure_zero(nonce_copy.as_mut_ptr().cast(), nonce_copy.len());
    result
}

#[unsafe(no_mangle)]
/// Authenticates and clears prepared-mode bootstrap state before preparation.
unsafe extern "C" fn zygote_rt_authenticate_prepared() -> c_int {
    let mut fd = -1;
    let mut nonce = [0; 33];
    let result = consume_bootstrap(addr_of_mut!(fd), nonce.as_mut_ptr());
    if result == 1 {
        PREPARED_CONTROL_FD = fd;
        libc::memcpy(addr_of_mut!(PREPARED_NONCE).cast(), nonce.as_ptr().cast(), nonce.len());
    }
    secure_zero(nonce.as_mut_ptr().cast(), nonce.len());
    result
}

#[unsafe(no_mangle)]
/// Releases authenticated prepared-mode bootstrap state after preparation fails.
unsafe extern "C" fn zygote_rt_abort_prepared() {
    if PREPARED_CONTROL_FD >= 0 {
        libc::close(PREPARED_CONTROL_FD);
        PREPARED_CONTROL_FD = -1;
    }
    secure_zero(addr_of_mut!(PREPARED_NONCE).cast(), 33);
}

#[unsafe(no_mangle)]
/// Prepared-mode entry called by the Rust adapter.
///
/// # Safety
///
/// `entry`, when present, must accept `context` and the saved startup argument
/// vector for the duration of the zygote loop.
unsafe extern "C-unwind" fn zygote_rt_run_prepared(context: *mut c_void, entry: Option<Entry>) -> c_int {
    let Some(entry) = entry else {
        return 125;
    };
    RESET_CHILD_SIGNALS = PREPARED_CONTROL_FD >= 0;
    if PREPARED_CONTROL_FD < 0 {
        return entry(context, PREPARED_ARGC, PREPARED_ARGV);
    }
    let fd = PREPARED_CONTROL_FD;
    PREPARED_CONTROL_FD = -1;
    let mut nonce = [0; 33];
    libc::memcpy(nonce.as_mut_ptr().cast(), addr_of!(PREPARED_NONCE).cast(), nonce.len());
    secure_zero(addr_of_mut!(PREPARED_NONCE).cast(), 33);
    let result = run_zygote(fd, nonce.as_mut_ptr(), entry, context);
    secure_zero(nonce.as_mut_ptr().cast(), nonce.len());
    result
}

#[unsafe(no_mangle)]
/// Linker entry used by `--wrap=main`.
///
/// # Safety
///
/// The platform startup code must supply a valid C argument vector.
pub unsafe extern "C-unwind" fn __wrap_main(argc: c_int, argv: *mut *mut c_char) -> c_int {
    let mut marker = addr_of!(__start_zygote_rt_mode);
    let end = addr_of!(__stop_zygote_rt_mode);
    let mut prepared = false;
    while marker < end {
        if *marker != 0 {
            prepared = true;
            break;
        }
        marker = marker.add(1);
    }
    if prepared {
        PREPARED_ARGC = argc;
        PREPARED_ARGV = argv;
        return __real_main(argc, argv);
    }
    RESET_CHILD_SIGNALS = true;
    dispatch(argc, argv, real_main_entry, null_mut())
}

#[inline]
/// # Safety
///
/// The platform C runtime must provide a valid thread-local errno location.
unsafe fn errno() -> c_int {
    *libc::__errno_location()
}

#[inline]
/// # Safety
///
/// The platform C runtime must provide a valid thread-local errno location.
unsafe fn set_errno(value: c_int) {
    *libc::__errno_location() = value;
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![expect(
        clippy::borrow_as_ptr,
        clippy::multiple_unsafe_ops_per_block,
        clippy::undocumented_unsafe_blocks,
        reason = "tests exercise compact raw-pointer syscall harnesses with live local storage"
    )]
    use prost::Message;

    use super::*;

    static FAULT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn empty_null_cursor_has_no_remaining_bytes() {
        // SAFETY: a null pointer is permitted only for this zero-length cursor.
        let cursor = unsafe { Cursor::new(null(), 0).unwrap() };
        // SAFETY: the cursor has not advanced from its valid empty state.
        assert_eq!(unsafe { cursor.remaining() }, 0);
    }

    #[test]
    fn cursor_decodes_fields_and_rejects_invalid_wire_data() {
        let encoded = [0x96, 0x01, 0x0a, 0x03, b'a', b'b', b'c'];
        // SAFETY: encoded remains live for the cursor's lifetime.
        let mut cursor = unsafe { Cursor::new(encoded.as_ptr(), encoded.len()).unwrap() };
        // SAFETY: cursor was constructed from live contiguous storage.
        assert_eq!(unsafe { cursor.varint() }, Some(150));
        assert_eq!(unsafe { cursor.key() }, Some((1, 2)));
        let (bytes, len) = unsafe { cursor.bytes() }.unwrap();
        assert_eq!(len, 3);
        // SAFETY: bytes points to the three-byte suffix of encoded.
        assert_eq!(unsafe { std::slice::from_raw_parts(bytes, len) }, b"abc");
        assert_eq!(unsafe { cursor.remaining() }, 0);
        assert_eq!(unsafe { cursor.varint() }, None);

        // SAFETY: null is invalid for a nonempty cursor.
        assert!(unsafe { Cursor::new(null(), 1) }.is_none());
        for invalid in [&[0x80][..], &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02][..]] {
            // SAFETY: invalid remains live while the cursor is used.
            let mut cursor = unsafe { Cursor::new(invalid.as_ptr(), invalid.len()).unwrap() };
            assert_eq!(unsafe { cursor.varint() }, None);
        }
        for invalid_key in [&[0][..], &[0x80][..]] {
            // SAFETY: invalid_key remains live while the cursor is used.
            let mut cursor = unsafe { Cursor::new(invalid_key.as_ptr(), invalid_key.len()).unwrap() };
            assert_eq!(unsafe { cursor.key() }, None);
        }

        let truncated = [4, 1, 2];
        // SAFETY: truncated remains live while the cursor is used.
        let mut cursor = unsafe { Cursor::new(truncated.as_ptr(), truncated.len()).unwrap() };
        assert_eq!(unsafe { cursor.bytes() }, None);

        for (wire, encoded, expected_remaining) in [(0, &[1][..], 0), (1, &[0; 8][..], 0), (2, &[2, 7, 8][..], 0), (5, &[0; 4][..], 0)] {
            // SAFETY: encoded remains live while the cursor is used.
            let mut cursor = unsafe { Cursor::new(encoded.as_ptr(), encoded.len()).unwrap() };
            assert!(unsafe { cursor.skip(wire) });
            assert_eq!(unsafe { cursor.remaining() }, expected_remaining);
        }
        for (wire, encoded) in [(1, &[0; 7][..]), (5, &[0; 3][..]), (3, &[][..])] {
            // SAFETY: encoded remains live while the cursor is used.
            let mut cursor = unsafe { Cursor::new(encoded.as_ptr(), encoded.len()).unwrap() };
            assert!(!unsafe { cursor.skip(wire) });
        }
    }

    #[test]
    fn encoder_preserves_varints_fields_and_capacity_limits() {
        let mut encoder = Encoder::new();
        // SAFETY: encoder owns its initialized fixed buffer.
        assert!(unsafe { encoder.varint(300) });
        assert_eq!(&encoder.bytes[..encoder.len], &[0xac, 0x02]);
        assert!(unsafe { encoder.field_varint(3, 1) });
        assert_eq!(&encoder.bytes[..encoder.len], &[0xac, 0x02, 0x18, 0x01]);
        let value = b"payload";
        assert!(unsafe { encoder.field_bytes(4, value.as_ptr(), value.len()) });
        assert_eq!(&encoder.bytes[4..encoder.len], b"\x22\x07payload");

        let mut full = Encoder::new();
        for value in 0..full.bytes.len() {
            assert!(unsafe { full.byte(u8::try_from(value).unwrap()) });
        }
        assert!(!unsafe { full.byte(0) });
        assert!(!unsafe { full.varint(1) });

        let oversized = [0u8; 256];
        let mut insufficient = Encoder::new();
        assert!(!unsafe { insufficient.field_bytes(1, oversized.as_ptr(), oversized.len()) });

        let values = [0, 1, 300];
        let mut output = [0u8; 16];
        let len = unsafe { encode_varint_body(values.as_ptr(), values.len(), output.as_mut_ptr(), output.len()) }.unwrap();
        assert_eq!(&output[..len], &[0x10, 0x01, 0x18, 0xac, 0x02]);
        assert_eq!(
            unsafe { encode_varint_body(values.as_ptr(), values.len(), output.as_mut_ptr(), len - 1) },
            None
        );
    }

    #[test]
    fn pool_target_decoder_handles_defaults_extensions_and_errors() {
        for (bytes, expected) in [
            (&[][..], Some(0)),
            (&[0x08, 0x07][..], Some(7)),
            (&[0x10, 0x2a, 0x08, 0x03][..], Some(3)),
            (&[0x1a, 0x02, 0xaa, 0xbb, 0x08, 0x05][..], Some(5)),
            (&[0x08, 0x01, 0x08, 0x02][..], None),
            (&[0x0a, 0x00][..], None),
            (&[0x08, 0x80][..], None),
            (&[0x1b][..], None),
        ] {
            // SAFETY: bytes remains live for the complete bounded decode.
            assert_eq!(unsafe { decode_pool_target(bytes.as_ptr(), bytes.len()) }, expected);
        }
    }

    #[test]
    fn prefork_pool_initializes_all_state_from_configuration() {
        let config = PreforkConfig {
            min_idle: 2,
            max_idle: 7,
            refill_threshold: 4,
            refill_delay_millis: 25,
        };
        let pool = PreforkPool::new(config);
        assert_eq!(pool.count, 0);
        assert_eq!(pool.config.min_idle, 2);
        assert_eq!(pool.config.max_idle, 7);
        assert_eq!(pool.config.refill_threshold, 4);
        assert_eq!(pool.config.refill_delay_millis, 25);
        assert_eq!(pool.maximum_capacity, 7);
        assert_eq!(pool.refill_deadline_millis, -1);
        assert_eq!(pool.refills, 0);
        assert_eq!(pool.refill_nanoseconds, 0);
        assert!(pool.workers.iter().all(|worker| worker.pid == 0 && worker.socket == -1));
    }

    #[cfg(feature = "private-test-util")]
    #[test]
    fn reusable_native_decoder_reports_growth_and_rejects_invalid_packets() {
        let small = encoded_launch(1);
        let large = encoded_launch(32);
        let mut decoder = NativeLaunchDecoder::default();

        let first = decoder.decode(&small).unwrap();
        assert_eq!(first.packet_bytes, small.len());
        assert!(first.arena_bytes > 0);
        assert_eq!(first.allocations, 1);

        let repeated = decoder.decode(&small).unwrap();
        assert_eq!(repeated.arena_bytes, first.arena_bytes);
        assert_eq!(repeated.allocations, 1);

        let grown = decoder.decode(&large).unwrap();
        assert!(grown.arena_bytes > first.arena_bytes);
        assert_eq!(grown.allocations, 2);

        assert_eq!(decoder.decode(&[]), None);
        assert_eq!(decoder.decode(&[0x08, 0x01]), None);
        let after_rejection = decoder.decode(&small).unwrap();
        assert_eq!(after_rejection.allocations, 2);
    }

    #[test]
    fn native_parser_materializes_complete_linux_launch_state() {
        use crate::protocol::packet::Body;
        use crate::protocol::{
            DescriptorRole, LinuxSandbox, NamespaceKind, PrivilegeReduction, Rlimit, SeccompInstruction, SeccompPolicy,
            SupplementaryGroups, UnixOptions,
        };

        #[cfg(target_arch = "x86_64")]
        let audit_architecture = 0xc000_003e;
        #[cfg(target_arch = "aarch64")]
        let audit_architecture = 0xc000_00b7;
        let instructions = vec![
            SeccompInstruction {
                code: 0x20,
                jump_true: 0,
                jump_false: 0,
                value: 4,
            },
            SeccompInstruction {
                code: 0x15,
                jump_true: 1,
                jump_false: 0,
                value: audit_architecture,
            },
            SeccompInstruction {
                code: 0x06,
                jump_true: 0,
                jump_false: 0,
                value: 0x8000_0000,
            },
            SeccompInstruction {
                code: 0x06,
                jump_true: 0,
                jump_false: 0,
                value: 0x7fff_0000,
            },
        ];
        let launch = crate::protocol::Launch {
            argv: vec![b"target".to_vec(), b"--flag".to_vec()],
            environment: vec![b"A=one".to_vec(), b"B=two".to_vec()],
            cwd: Some(b"/work".to_vec()),
            unix: Some(UnixOptions {
                uid: Some(123),
                gid: Some(456),
                groups: Some(SupplementaryGroups { gids: vec![7, 8] }),
                process_group: Some(-9),
                new_session: false,
                umask: Some(0o027),
            }),
            rlimits: vec![Rlimit {
                resource: libc::RLIMIT_NOFILE,
                soft: 64,
                hard: 128,
            }],
            sandbox: Some(LinuxSandbox {
                privileges: Some(PrivilegeReduction {
                    no_new_privs: true,
                    disable_dumping: true,
                    clear_capabilities: true,
                    drop_capability_bounding_set: true,
                    lock_securebits: true,
                }),
                seccomp: Some(SeccompPolicy {
                    audit_architecture,
                    instructions: instructions.clone(),
                }),
                landlock: true,
                cgroup: true,
                namespaces: vec![
                    NamespaceKind::Cgroup as i32,
                    NamespaceKind::Ipc as i32,
                    NamespaceKind::Uts as i32,
                    NamespaceKind::Network as i32,
                    NamespaceKind::Time as i32,
                    NamespaceKind::Mount as i32,
                ],
                landlock_abi: 3,
                landlock_handled_access: 0x1234,
            }),
        };
        let roles = vec![
            DescriptorRole::Stdin,
            DescriptorRole::Stdout,
            DescriptorRole::Stderr,
            DescriptorRole::CgroupProcs,
            DescriptorRole::LandlockRuleset,
            DescriptorRole::NamespaceCgroup,
            DescriptorRole::NamespaceIpc,
            DescriptorRole::NamespaceUts,
            DescriptorRole::NamespaceNetwork,
            DescriptorRole::NamespaceTime,
            DescriptorRole::NamespaceMount,
        ];
        let bytes = crate::protocol::packet(42, Body::Launch(launch), roles).encode_to_vec();
        // SAFETY: bytes remains live while packet and its parsed launch are used.
        let packet = unsafe { decoded_packet(&bytes) };
        let mut native = Launch::empty();
        let mut storage = null_mut();
        let mut capacity = 0;
        // SAFETY: packet borrows from bytes and the output state is exclusively owned.
        assert!(unsafe { parse_launch(&packet, &mut native, &mut storage, &mut capacity) });

        assert_eq!(native.argc, 2);
        assert_eq!(native.uid, 123);
        assert_eq!(native.gid, 456);
        assert_eq!(native.group_count, 2);
        assert_eq!(native.process_group, -9);
        assert_eq!(native.umask_value, 0o027);
        assert!(native.has_uid && native.has_gid && native.has_groups && native.has_process_group && native.has_umask);
        assert!(!native.new_session);

        assert_eq!(native.limit_count, 1);
        assert!(native.no_new_privs);
        assert!(native.disable_dumping);
        assert!(native.clear_capabilities);
        assert!(native.drop_capability_bounding_set);
        assert!(native.lock_securebits);
        assert!(native.landlock);
        assert!(native.cgroup);
        assert_eq!(native.namespace_count, 6);
        assert_eq!(native.landlock_abi, 3);
        assert_eq!(native.landlock_handled_access, 0x1234);
        assert_eq!(native.seccomp_architecture, audit_architecture);
        assert_eq!(native.seccomp_count, instructions.len());
        assert_eq!(packet.role_count, 11);
        assert_eq!(
            &packet.roles[..packet.role_count],
            &[
                ROLE_STDIN,
                ROLE_STDOUT,
                ROLE_STDERR,
                ROLE_CGROUP_PROCS,
                ROLE_LANDLOCK_RULESET,
                ROLE_NAMESPACE_CGROUP,
                ROLE_NAMESPACE_IPC,
                ROLE_NAMESPACE_UTS,
                ROLE_NAMESPACE_NETWORK,
                ROLE_NAMESPACE_TIME,
                ROLE_NAMESPACE_MOUNT,
            ]
        );
        assert_eq!(native.arena, storage);
        assert_eq!(native.arena_size, capacity);
        let original_storage = storage;

        // SAFETY: native owns the initialized view into storage.
        unsafe { free_launch(&mut native) };
        assert!(native.arena.is_null());
        // SAFETY: packet remains live and storage retains its allocation.
        assert!(unsafe { parse_launch(&packet, &mut native, &mut storage, &mut capacity) });
        assert_eq!(storage, original_storage);
        assert_eq!(native.arena, original_storage);
        // SAFETY: native owns the initialized view and storage owns the allocation.
        unsafe {
            free_launch(&mut native);
            libc::free(storage);
        }
    }

    #[test]
    fn native_scanner_rejects_sandbox_feature_and_descriptor_mismatches() {
        use crate::protocol::packet::Body;
        use crate::protocol::{DescriptorRole, LinuxSandbox, NamespaceKind, PrivilegeReduction};

        let standard_roles = vec![DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr];
        let launch = |sandbox| crate::protocol::Launch {
            sandbox,
            ..proto_launch(vec![b"target".to_vec()], Vec::new())
        };
        let privileges = || PrivilegeReduction {
            no_new_privs: true,
            disable_dumping: false,
            clear_capabilities: false,
            drop_capability_bounding_set: false,
            lock_securebits: false,
        };

        let mut feature_without_body =
            crate::protocol::packet(7, Body::Launch(launch(Some(LinuxSandbox::default()))), standard_roles.clone());
        let Some(Body::Launch(feature_launch)) = feature_without_body.body.as_mut() else {
            unreachable!()
        };
        feature_launch.sandbox = None;

        let mut body_without_feature =
            crate::protocol::packet(7, Body::Launch(launch(Some(LinuxSandbox::default()))), standard_roles.clone());
        body_without_feature.required_features.truncate(1);

        let missing_cgroup = crate::protocol::packet(
            7,
            Body::Launch(launch(Some(LinuxSandbox {
                cgroup: true,
                ..LinuxSandbox::default()
            }))),
            standard_roles.clone(),
        );
        let missing_landlock = crate::protocol::packet(
            7,
            Body::Launch(launch(Some(LinuxSandbox {
                privileges: Some(privileges()),
                landlock: true,
                landlock_abi: 1,
                landlock_handled_access: 1,
                ..LinuxSandbox::default()
            }))),
            standard_roles.clone(),
        );
        let missing_namespace = crate::protocol::packet(
            7,
            Body::Launch(launch(Some(LinuxSandbox {
                namespaces: vec![NamespaceKind::Network as i32],
                ..LinuxSandbox::default()
            }))),
            standard_roles.clone(),
        );

        let mut extra_role = standard_roles.clone();
        extra_role.push(DescriptorRole::CgroupProcs);
        let extra_capability = crate::protocol::packet(7, Body::Launch(launch(Some(LinuxSandbox::default()))), extra_role);

        let cgroup_and_landlock = LinuxSandbox {
            privileges: Some(privileges()),
            landlock: true,
            cgroup: true,
            landlock_abi: 1,
            landlock_handled_access: 1,
            ..LinuxSandbox::default()
        };
        let mut duplicate_roles = standard_roles.clone();
        duplicate_roles.extend([DescriptorRole::CgroupProcs, DescriptorRole::CgroupProcs]);
        let duplicate_capability = crate::protocol::packet(7, Body::Launch(launch(Some(cgroup_and_landlock.clone()))), duplicate_roles);
        let mut reversed_roles = standard_roles.clone();
        reversed_roles.extend([DescriptorRole::LandlockRuleset, DescriptorRole::CgroupProcs]);
        let wrong_order = crate::protocol::packet(7, Body::Launch(launch(Some(cgroup_and_landlock))), reversed_roles);

        let mut wrong_namespace_role = standard_roles;
        wrong_namespace_role.push(DescriptorRole::NamespaceMount);
        let namespace_mismatch = crate::protocol::packet(
            7,
            Body::Launch(launch(Some(LinuxSandbox {
                namespaces: vec![NamespaceKind::Network as i32],
                ..LinuxSandbox::default()
            }))),
            wrong_namespace_role,
        );

        for packet in [
            feature_without_body,
            body_without_feature,
            missing_cgroup,
            missing_landlock,
            missing_namespace,
            extra_capability,
            duplicate_capability,
            wrong_order,
            namespace_mismatch,
        ] {
            assert!(!native_accepts_launch(&packet.encode_to_vec()));
        }
    }

    unsafe fn wait_until_exited(pid: libc::pid_t) -> bool {
        let mut info: libc::siginfo_t = zeroed();
        loop {
            if libc::waitid(libc::P_PID, pid as libc::id_t, addr_of_mut!(info), libc::WEXITED | libc::WNOWAIT) == 0 {
                return true;
            }
            if errno() != libc::EINTR {
                return false;
            }
        }
    }

    fn encoded_launch(argument_count: usize) -> Vec<u8> {
        use crate::protocol::packet::Body;
        use crate::protocol::{DescriptorRole, Launch as ProtoLaunch, UnixOptions};

        crate::protocol::packet(
            7,
            Body::Launch(ProtoLaunch {
                argv: (0..argument_count).map(|index| format!("arg{index}").into_bytes()).collect(),
                environment: vec![b"A=B".to_vec()],
                cwd: Some(b"/".to_vec()),
                unix: Some(UnixOptions::default()),
                rlimits: Vec::new(),
                sandbox: None,
            }),
            vec![DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr],
        )
        .encode_to_vec()
    }

    /// # Safety
    ///
    /// The returned packet borrows from `bytes`, so the caller must keep the
    /// slice alive while using any packet body pointer.
    unsafe fn decoded_packet(bytes: &[u8]) -> Packet {
        let mut packet = Packet::empty();
        assert!(decode_packet(bytes.as_ptr(), bytes.len(), &raw mut packet));
        packet
    }

    fn native_accepts_launch(bytes: &[u8]) -> bool {
        let mut packet = Packet::empty();
        // SAFETY: bytes and packet remain live for the complete parse.
        if !unsafe { decode_packet(bytes.as_ptr(), bytes.len(), &raw mut packet) } || packet.kind != BODY_LAUNCH {
            return false;
        }
        let mut launch = Launch::empty();
        let mut storage = null_mut();
        let mut capacity = 0;
        // SAFETY: packet borrows from bytes and all outputs remain live.
        let accepted = unsafe { parse_launch(&raw const packet, &mut launch, &mut storage, &mut capacity) };
        if accepted {
            // SAFETY: launch owns the initialized reusable arena view.
            unsafe { free_launch(&raw mut launch) };
        }
        if !storage.is_null() {
            // SAFETY: storage is the sole owner of this calloc allocation.
            unsafe { libc::free(storage) };
        }
        accepted
    }

    fn proto_launch(argv: Vec<Vec<u8>>, environment: Vec<Vec<u8>>) -> crate::protocol::Launch {
        crate::protocol::Launch {
            argv,
            environment,
            cwd: None,
            unix: None,
            rlimits: Vec::new(),
            sandbox: None,
        }
    }

    #[test]
    fn bounded_decoder_accepts_prost_packets_and_limits() {
        for count in [1, MAX_ITEM_COUNT] {
            let bytes = encoded_launch(count);
            let mut packet = Packet::empty();
            // SAFETY: bytes and packet are live for the decoder call.
            assert!(unsafe { decode_packet(bytes.as_ptr(), bytes.len(), &mut packet) });
            assert!(packet.launch_validated);
            assert_eq!(packet.launch_shape.argc, count);
        }
        let bytes = encoded_launch(MAX_ITEM_COUNT + 1);
        let mut packet = Packet::empty();
        // SAFETY: bytes and packet are live for the decoder call.
        assert!(!unsafe { decode_packet(bytes.as_ptr(), bytes.len(), &mut packet) });
    }

    #[test]
    fn bounded_decoder_accepts_lifecycle_pool_state_only_without_request_metadata() {
        use crate::protocol::packet::Body;
        use crate::protocol::{DescriptorRole, PoolState as ProtoPoolState};

        let state = ProtoPoolState {
            idle_workers: 3,
            prefork_refills: 5,
            prefork_refill_nanoseconds: 8,
        };
        let bytes = crate::protocol::packet(0, Body::PoolState(state), Vec::new()).encode_to_vec();
        let packet = unsafe { decoded_packet(&bytes) };
        assert_eq!(packet.kind, BODY_POOL_STATE);
        assert_eq!(packet.request_id, 0);
        assert_eq!(packet.role_count, 0);

        let body_start = packet.body.addr().checked_sub(bytes.as_ptr().addr()).unwrap();
        let body_end = body_start.checked_add(packet.body_len).unwrap();
        let decoded_state = ProtoPoolState::decode(bytes.get(body_start..body_end).unwrap()).unwrap();
        assert_eq!(decoded_state.idle_workers, 3);
        assert_eq!(decoded_state.prefork_refills, 5);
        assert_eq!(decoded_state.prefork_refill_nanoseconds, 8);

        for malformed in [
            crate::protocol::packet(
                1,
                Body::PoolState(ProtoPoolState {
                    idle_workers: 3,
                    prefork_refills: 5,
                    prefork_refill_nanoseconds: 8,
                }),
                Vec::new(),
            ),
            crate::protocol::packet(
                0,
                Body::PoolState(ProtoPoolState {
                    idle_workers: 3,
                    prefork_refills: 5,
                    prefork_refill_nanoseconds: 8,
                }),
                vec![DescriptorRole::Pidfd],
            ),
        ] {
            let bytes = malformed.encode_to_vec();
            let mut packet = Packet::empty();
            assert!(!unsafe { decode_packet(bytes.as_ptr(), bytes.len(), &raw mut packet) });
        }

        let mut mixed_bodies = bytes;
        mixed_bodies.extend_from_slice(&[0x52, 0]);
        let mut packet = Packet::empty();
        assert!(!unsafe { decode_packet(mixed_bodies.as_ptr(), mixed_bodies.len(), &raw mut packet) });
    }

    #[test]
    fn bounded_decoder_rejects_malformed_and_ambiguous_packets() {
        let valid = encoded_launch(1);
        let mut duplicate_version = vec![8, 1];
        duplicate_version.extend_from_slice(&valid);
        // SAFETY: the input slice is live and the temporary packet is writable.
        assert!(!unsafe { decode_packet(duplicate_version.as_ptr(), duplicate_version.len(), &mut Packet::empty(),) });
        let mut unknown_feature = valid;
        unknown_feature.splice(4..4, [24, 127]);
        // SAFETY: the input slice is live and the temporary packet is writable.
        assert!(!unsafe { decode_packet(unknown_feature.as_ptr(), unknown_feature.len(), &mut Packet::empty()) });
    }

    #[test]
    fn native_decoder_matches_fixed_corpus() {
        for case in crate::protocol::test_support::fixed_decoder_corpus() {
            assert_eq!(native_accepts_launch(&case.bytes), case.accepted, "{}", case.name);
        }
    }

    #[test]
    fn native_decoder_matches_generated_prost_oracle() {
        for case in crate::protocol::test_support::generated_decoder_cases() {
            assert_eq!(native_accepts_launch(&case.bytes), case.accepted, "{}", case.name);
        }
    }

    #[test]
    fn environment_validation_scales_and_preserves_bytewise_names() {
        use crate::protocol::DescriptorRole;
        use crate::protocol::packet::Body;

        let environment = (0..MAX_ITEM_COUNT)
            .map(|index| format!("KEY_{index:04x}=value").into_bytes())
            .collect::<Vec<_>>();
        let valid = crate::protocol::packet(
            7,
            Body::Launch(proto_launch(vec![b"target".to_vec()], environment.clone())),
            vec![DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr],
        )
        .encode_to_vec();
        assert!(native_accepts_launch(&valid));

        let mut distinct = environment;
        distinct[1] = b"key_0000=case-distinct".to_vec();
        let distinct = crate::protocol::packet(
            7,
            Body::Launch(proto_launch(vec![b"target".to_vec()], distinct)),
            vec![DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr],
        )
        .encode_to_vec();
        assert!(native_accepts_launch(&distinct));

        let duplicate = crate::protocol::packet(
            7,
            Body::Launch(proto_launch(
                vec![b"target".to_vec()],
                (0..MAX_ITEM_COUNT)
                    .map(|index| {
                        if index + 1 == MAX_ITEM_COUNT {
                            b"KEY_0000=different".to_vec()
                        } else {
                            format!("KEY_{index:04x}=value").into_bytes()
                        }
                    })
                    .collect(),
            )),
            vec![DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr],
        )
        .encode_to_vec();
        assert!(!native_accepts_launch(&duplicate));
    }

    #[test]
    fn child_table_handles_collisions_and_tombstones() {
        let mut children = vec![Child { pid: 0, request_id: 0 }; CHILD_TABLE_SIZE];
        let mut count = 0;
        let first = 1;
        let second = first + CHILD_TABLE_SIZE as i32;
        // SAFETY: children has CHILD_TABLE_SIZE initialized slots and count is exclusive.
        assert!(unsafe { add_child(children.as_mut_ptr(), &mut count, first, 10) });
        // SAFETY: the same table/count invariant remains established.
        assert!(unsafe { add_child(children.as_mut_ptr(), &mut count, second, 20) });
        // SAFETY: the same table/count invariant remains established.
        assert_eq!(unsafe { remove_child(children.as_mut_ptr(), &mut count, first) }, 10);
        // SAFETY: the same table/count invariant remains established.
        assert!(unsafe { add_child(children.as_mut_ptr(), &mut count, first, 30) });
        // SAFETY: the same table/count invariant remains established.
        assert_eq!(unsafe { remove_child(children.as_mut_ptr(), &mut count, second) }, 20);
        // SAFETY: the same table/count invariant remains established.
        assert_eq!(unsafe { remove_child(children.as_mut_ptr(), &mut count, first) }, 30);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn child_reaper_preserves_untracked_children() {
        let mut sockets = [-1; 2];
        // SAFETY: sockets points to storage for both returned descriptors.
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0, sockets.as_mut_ptr()) },
            0
        );
        // SAFETY: each child exits immediately without touching shared Rust state.
        let helper = unsafe { libc::fork() };
        assert!(helper >= 0);
        if helper == 0 {
            // SAFETY: immediate process termination does not unwind copied state.
            unsafe { libc::_exit(17) };
        }
        // SAFETY: helper is this process's child and WNOWAIT preserves its status.
        assert!(unsafe { wait_until_exited(helper) });

        // SAFETY: each child exits immediately without touching shared Rust state.
        let owned = unsafe { libc::fork() };
        assert!(owned >= 0);
        if owned == 0 {
            // SAFETY: immediate process termination does not unwind copied state.
            unsafe { libc::_exit(23) };
        }
        // SAFETY: owned is this process's child and WNOWAIT preserves its status.
        assert!(unsafe { wait_until_exited(owned) });

        let mut children = vec![Child { pid: 0, request_id: 0 }; CHILD_TABLE_SIZE];
        let mut count = 0;
        // SAFETY: children has CHILD_TABLE_SIZE initialized slots and count is exclusive.
        assert!(unsafe { add_child(children.as_mut_ptr(), &mut count, owned, 42) });
        // SAFETY: sockets[0] is live and the table/count invariant is established.
        assert!(matches!(
            unsafe { reap_children(sockets[0], children.as_mut_ptr(), &mut count) },
            ReapResult::Continue
        ));
        assert_eq!(count, 0);

        let mut status = 0;
        // SAFETY: helper must remain this process's unreaped child.
        assert_eq!(unsafe { libc::waitpid(helper, &raw mut status, 0) }, helper);
        assert!(libc::WIFEXITED(status));
        assert_eq!(libc::WEXITSTATUS(status), 17);
        // SAFETY: both socket descriptors remain owned by this test.
        unsafe {
            libc::close(sockets[0]);
            libc::close(sockets[1]);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn child_reaper_reports_detached_control_peer() {
        let mut sockets = [-1; 2];
        // SAFETY: sockets points to storage for both returned descriptors.
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0, sockets.as_mut_ptr()) },
            0
        );
        // SAFETY: the child exits immediately without touching shared Rust state.
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            // SAFETY: immediate process termination does not unwind copied state.
            unsafe { libc::_exit(17) };
        }
        // SAFETY: child is this process's child and WNOWAIT preserves its status.
        assert!(unsafe { wait_until_exited(child) });

        let mut children = vec![Child { pid: 0, request_id: 0 }; CHILD_TABLE_SIZE];
        let mut count = 0;
        // SAFETY: children has CHILD_TABLE_SIZE initialized slots and count is exclusive.
        assert!(unsafe { add_child(children.as_mut_ptr(), &mut count, child, 42) });
        // SAFETY: sockets[1] is owned by this test; shutdown also disables
        // transient duplicates inherited by concurrently forked test children.
        unsafe {
            libc::shutdown(sockets[1], libc::SHUT_RDWR);
            libc::close(sockets[1]);
        }
        // SAFETY: sockets[0] remains live and the table/count invariant is established.
        assert!(matches!(
            unsafe { reap_children(sockets[0], children.as_mut_ptr(), &mut count) },
            ReapResult::Detached
        ));
        assert_eq!(count, 0);
        // SAFETY: sockets[0] remains owned by this test.
        unsafe { libc::close(sockets[0]) };
    }

    #[test]
    fn launch_parser_rejects_inconsistent_arena_ownership() {
        let bytes = encoded_launch(1);
        // SAFETY: bytes remains live while packet is used.
        let packet = unsafe { decoded_packet(&bytes) };
        let mut launch = Launch::empty();
        let mut storage = null_mut();
        let mut capacity = 1;
        // SAFETY: packet and outputs remain live; the intentionally
        // inconsistent arena pair must be rejected before any arena access.
        assert!(!unsafe { parse_launch(&packet, &mut launch, &mut storage, &mut capacity) });

        // SAFETY: calloc returns an allocation owned by this test.
        storage = unsafe { libc::calloc(1, 1) };
        assert!(!storage.is_null());
        capacity = 0;
        // SAFETY: as above, the inconsistent pair must be rejected untouched.
        assert!(!unsafe { parse_launch(&packet, &mut launch, &mut storage, &mut capacity) });
        // SAFETY: storage remains the allocation returned by calloc.
        unsafe { libc::free(storage) };
    }

    #[test]
    fn launch_arena_is_reused_and_erased() {
        let first_bytes = encoded_launch(8);
        let second_bytes = encoded_launch(1);
        // SAFETY: first_bytes remains live while first is used.
        let first = unsafe { decoded_packet(&first_bytes) };
        // SAFETY: second_bytes remains live while second is used.
        let second = unsafe { decoded_packet(&second_bytes) };
        let mut storage = null_mut();
        let mut capacity = 0;
        let mut launch = Launch::empty();

        // SAFETY: packet storage and all writable outputs remain live.
        assert!(unsafe { parse_launch(&first, &mut launch, &mut storage, &mut capacity) });
        let first_storage = NonNull::new(storage).unwrap();
        assert!(capacity > 0);
        assert_eq!(launch.arena_capacity, capacity);
        let _: isize = capacity.try_into().unwrap();
        // SAFETY: successful parsing returned the sole live arena allocation,
        // capacity is its allocation size, and the borrow ends before free_launch.
        let arena = unsafe { std::slice::from_raw_parts_mut(first_storage.as_ptr().cast::<u8>(), capacity) };
        assert!(arena.windows(b"arg7".len()).any(|window| window == b"arg7"));
        arena.fill(0xa5);
        // SAFETY: launch owns the initialized reusable arena view.
        unsafe { free_launch(&mut launch) };
        assert!(capacity != 0);
        // SAFETY: free_launch retains and erases the same allocation, and no
        // mutable arena borrow remains live.
        assert!(
            unsafe { std::slice::from_raw_parts(first_storage.as_ptr().cast::<u8>(), capacity) }
                .iter()
                .all(|&byte| byte == 0)
        );
        // Emulate stale data anywhere in the retained allocation before reuse.
        unsafe { std::slice::from_raw_parts_mut(first_storage.as_ptr().cast::<u8>(), capacity) }.fill(0xa5);
        // SAFETY: packet storage and all writable outputs remain live.
        assert!(unsafe { parse_launch(&second, &mut launch, &mut storage, &mut capacity) });
        assert_eq!(storage, first_storage.as_ptr());
        assert!(launch.arena_size < launch.arena_capacity);
        assert!(
            unsafe {
                std::slice::from_raw_parts(
                    first_storage.as_ptr().cast::<u8>().add(launch.arena_size),
                    capacity - launch.arena_size,
                )
            }
            .iter()
            .all(|&byte| byte == 0)
        );
        unsafe { std::slice::from_raw_parts_mut(first_storage.as_ptr().cast::<u8>(), capacity) }.fill(0x5a);
        // SAFETY: launch owns the initialized reusable arena view.
        unsafe { free_launch(&mut launch) };
        assert!(
            unsafe { std::slice::from_raw_parts(first_storage.as_ptr().cast::<u8>(), capacity) }
                .iter()
                .all(|&byte| byte == 0)
        );
        // SAFETY: storage is the sole remaining calloc allocation owner.
        unsafe { libc::free(storage) };
    }

    #[test]
    fn receive_buffer_clears_each_packet_without_retaining_larger_prior_requests() {
        let mut buffer = vec![0; MAX_PACKET_LEN];
        let mut high_water = 0;
        buffer[..4096].fill(0xa5);
        // SAFETY: the buffer is writable and 4096 is the first received length.
        unsafe { clear_received_packet(buffer.as_mut_ptr(), 4096, &mut high_water) };
        assert!(buffer.iter().all(|&byte| byte == 0));
        assert_eq!(high_water, 4096);

        buffer[..4096].fill(0xa5);
        buffer[..32].fill(0x5a);
        // SAFETY: the same allocation is reused for a shorter packet.
        unsafe { clear_received_packet(buffer.as_mut_ptr(), 32, &mut high_water) };
        assert!(buffer.iter().all(|&byte| byte == 0));
        assert_eq!(high_water, 4096);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn sparse_descriptor_is_closed_by_both_child_cleanup_paths() {
        for close in [
            close_above_acknowledgement_with_close_range as unsafe fn() -> Option<bool>,
            close_above_acknowledgement_with_procfs,
        ] {
            let mut pipe = [-1; 2];
            // SAFETY: pipe points to two valid descriptor slots.
            assert_eq!(unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
            // SAFETY: the pipe reader is open and 4096 is a valid minimum descriptor.
            let sparse = unsafe { libc::fcntl(pipe[0], libc::F_DUPFD_CLOEXEC, 4096) };
            assert!(sparse >= 4096);
            // SAFETY: the child only makes raw syscalls before exiting without unwinding.
            let pid = unsafe { libc::fork() };
            assert!(pid >= 0);
            if pid == 0 {
                // SAFETY: these descriptors belong to this child. Descriptor 3
                // models the acknowledgement pipe used by real specialization.
                unsafe {
                    libc::close(pipe[0]);
                    if libc::dup2(pipe[1], 3) != 3 {
                        libc::_exit(2);
                    }
                    if pipe[1] != 3 {
                        libc::close(pipe[1]);
                    }
                    let closed = close() == Some(true)
                        && libc::fcntl(sparse, libc::F_GETFD) == -1
                        && errno() == libc::EBADF
                        && (0..=3).all(|descriptor| libc::fcntl(descriptor, libc::F_GETFD) >= 0);
                    let outcome = u8::from(closed);
                    libc::write(3, (&raw const outcome).cast(), 1);
                    libc::_exit(i32::from(!closed));
                }
            }
            // SAFETY: only the parent retains these descriptors and owns pid.
            unsafe {
                libc::close(pipe[1]);
                libc::close(sparse);
                let mut outcome = 0u8;
                let bytes = libc::read(pipe[0], (&raw mut outcome).cast(), 1);
                libc::close(pipe[0]);
                let mut status = 0;
                assert_eq!(libc::waitpid(pid, &mut status, 0), pid);
                assert_eq!(bytes, 1);
                assert_eq!(outcome, 1);
                assert!(libc::WIFEXITED(status));
                assert_eq!(libc::WEXITSTATUS(status), 0);
            }
        }
    }

    #[test]
    fn launch_parser_rejects_ambiguous_semantics() {
        use crate::protocol::packet::Body;
        use crate::protocol::{DescriptorRole, Launch as ProtoLaunch, Rlimit, UnixOptions};

        let duplicate_cases = [
            proto_launch(vec![b"target".to_vec()], vec![b"A=1".to_vec(), b"A=2".to_vec()]),
            ProtoLaunch {
                rlimits: vec![
                    Rlimit {
                        resource: libc::RLIMIT_NOFILE,
                        soft: 1,
                        hard: 2,
                    },
                    Rlimit {
                        resource: libc::RLIMIT_NOFILE,
                        soft: 1,
                        hard: 2,
                    },
                ],
                ..proto_launch(vec![b"target".to_vec()], Vec::new())
            },
        ];
        for launch in duplicate_cases {
            let bytes = crate::protocol::packet(
                7,
                Body::Launch(launch),
                vec![DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr],
            )
            .encode_to_vec();
            assert!(!native_accepts_launch(&bytes));
        }

        let scan_cases = [
            proto_launch(vec![b"bad\0arg".to_vec()], Vec::new()),
            proto_launch(vec![b"target".to_vec()], vec![b"NAME=bad\0value".to_vec()]),
            proto_launch(vec![b"target".to_vec()], vec![b"NA\0ME=value".to_vec()]),
            ProtoLaunch {
                unix: Some(UnixOptions {
                    process_group: Some(0),
                    new_session: true,
                    ..UnixOptions::default()
                }),
                ..proto_launch(vec![b"target".to_vec()], Vec::new())
            },
        ];
        for launch in scan_cases {
            let bytes = crate::protocol::packet(
                7,
                Body::Launch(launch),
                vec![DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr],
            )
            .encode_to_vec();
            let mut packet = Packet::empty();
            // SAFETY: bytes and packet remain live for the decoder call.
            assert!(!unsafe { decode_packet(bytes.as_ptr(), bytes.len(), &raw mut packet) });
        }
    }

    #[test]
    fn native_decoder_accepts_omitted_zero_valued_rlimit_fields() {
        use crate::protocol::packet::Body;
        use crate::protocol::{DescriptorRole, Rlimit};

        let mut launch = proto_launch(vec![b"target".to_vec()], Vec::new());
        launch.rlimits = vec![
            Rlimit {
                resource: libc::RLIMIT_CPU,
                soft: 1,
                hard: 2,
            },
            Rlimit {
                resource: libc::RLIMIT_NOFILE,
                soft: 0,
                hard: 0,
            },
        ];
        let bytes = crate::protocol::packet(
            7,
            Body::Launch(launch),
            vec![DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr],
        )
        .encode_to_vec();

        assert!(native_accepts_launch(&bytes));
    }

    #[test]
    fn failure_packets_are_protobuf() {
        let values = [7, libc::EIO as u64];
        let mut body = [0; 32];
        // SAFETY: values is readable and body writable for their stated lengths.
        let len = unsafe { encode_varint_body(values.as_ptr(), 2, body.as_mut_ptr(), body.len()) }.unwrap();
        let decoded = crate::protocol::ErrorMessage::decode(&body[..len]).unwrap();
        assert_eq!(decoded.stage, 7);
        assert_eq!(decoded.error_number, libc::EIO as u32);
    }

    #[test]
    fn specialization_deadline_accounting_never_restarts_the_timeout() {
        let start = libc::timespec {
            tv_sec: 10,
            tv_nsec: 900_000_000,
        };
        let deadline = deadline_after_millis(start, 250);
        assert_eq!(deadline.tv_sec, 11);
        assert_eq!(deadline.tv_nsec, 150_000_000);
        assert_eq!(remaining_timeout_millis(deadline, libc::timespec { tv_sec: 11, tv_nsec: 1 },), 150);
        assert_eq!(remaining_timeout_millis(deadline, deadline), 0);
        assert_eq!(remaining_timeout_millis(deadline, libc::timespec { tv_sec: 12, tv_nsec: 0 },), 0);
    }

    #[test]
    fn specialization_acknowledgement_timeout_is_finite() {
        let mut descriptors = [-1; 2];
        // SAFETY: descriptors has room for both pipe descriptors.
        assert_eq!(unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        let byte = 1u8;
        // SAFETY: descriptors[1] is live and byte is readable.
        assert_eq!(unsafe { libc::write(descriptors[1], (&raw const byte).cast(), 1) }, 1);
        let mut result = ChildSetupResult { stage: 0, error_number: 0 };
        // SAFETY: descriptors[0] is a readable pipe and result is writable.
        // The ready byte proves an expired deadline is rejected before poll.
        let size = unsafe { read_child_setup_result_with_timeout(descriptors[0], &raw mut result, 0) };
        // SAFETY: the read descriptor is owned by this test.
        unsafe { libc::close(descriptors[0]) };
        // SAFETY: the write descriptor is owned by this test.
        unsafe { libc::close(descriptors[1]) };
        assert_eq!(size, -1);
        // SAFETY: errno is read on the current test thread.
        assert_eq!(unsafe { errno() }, libc::ETIMEDOUT);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn timed_out_child_is_terminated_and_reaped() {
        // SAFETY: fork is called by this single-threaded test process branch,
        // and the child immediately pauses without touching shared Rust state.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            // SAFETY: pause has no pointer arguments and the child invokes only
            // async-signal-safe functions after fork.
            unsafe { libc::pause() };
            // SAFETY: the child exits directly and never unwinds through copied state.
            unsafe { libc::_exit(0) };
        }
        // SAFETY: pid identifies this test's unreaped child exclusively.
        assert!(unsafe { terminate_and_reap_child(pid) });
        let mut status = 0;
        // SAFETY: pid has already been reaped and status is writable.
        assert_eq!(unsafe { libc::waitpid(pid, &raw mut status, libc::WNOHANG) }, -1);
        // SAFETY: errno is read on the current test thread.
        assert_eq!(unsafe { errno() }, libc::ECHILD);
    }

    unsafe extern "C-unwind" fn blocking_entry(_: *mut c_void, _: c_int, _: *mut *mut c_char) -> c_int {
        libc::pause();
        0
    }

    fn launch_fault_result(fault_point: Fault) -> (crate::protocol::Packet, usize) {
        let bytes = encoded_launch(1);
        // SAFETY: bytes remains live while packet is handled.
        let mut packet = unsafe { decoded_packet(&bytes) };
        for descriptor in &mut packet.fds[..3] {
            // SAFETY: the path and flags are valid and each returned descriptor
            // becomes owned by the packet.
            *descriptor = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
            assert!(*descriptor >= 0);
        }
        packet.fd_count = 3;
        let mut sockets = [-1; 2];
        // SAFETY: sockets has room for both sequenced-packet descriptors.
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0, sockets.as_mut_ptr()) },
            0
        );
        let mut children = vec![Child { pid: 0, request_id: 0 }; CHILD_TABLE_SIZE];
        let mut child_count = 0;
        let mut untracked_child = 0;
        let mut arena = null_mut();
        let mut capacity = 0;
        let mut pool = PreforkPool::new(PreforkConfig::disabled());
        let single_threaded = SingleThreaded::for_fault_harness();
        set_test_fault(fault_point);
        // SAFETY: all descriptors, packet storage, child table, and arena
        // ownership are local to this harness.
        assert_eq!(
            unsafe {
                handle_launch(
                    sockets[0],
                    -1,
                    &single_threaded,
                    children.as_mut_ptr(),
                    &raw mut child_count,
                    &raw mut untracked_child,
                    &raw mut packet,
                    blocking_entry,
                    null_mut(),
                    &mut arena,
                    &mut capacity,
                    bytes.as_ptr(),
                    bytes.len(),
                    &mut pool,
                )
            },
            0
        );
        set_test_fault(Fault::None);

        let mut response_bytes = [0u8; 256];
        // SAFETY: response is writable and the peer socket contains one error packet.
        let received = unsafe { libc::recv(sockets[1], response_bytes.as_mut_ptr().cast(), response_bytes.len(), 0) };
        assert!(received > 0);
        let response = crate::protocol::decode(&response_bytes[..received as usize]).unwrap();
        // SAFETY: the socket remains live and the nonblocking probe writes
        // only within response.
        assert_eq!(
            unsafe {
                libc::recv(
                    sockets[1],
                    response_bytes.as_mut_ptr().cast(),
                    response_bytes.len(),
                    libc::MSG_DONTWAIT,
                )
            },
            -1
        );
        // SAFETY: errno is read on the current test thread.
        assert_eq!(unsafe { errno() }, libc::EAGAIN);
        // SAFETY: both socket descriptors and any reusable arena remain owned
        // by this harness.
        unsafe {
            libc::close(sockets[0]);
            libc::close(sockets[1]);
            if !arena.is_null() {
                libc::free(arena);
            }
        }
        (response, child_count)
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn launch_faults_report_exact_stage_errno_and_cleanup() {
        use crate::protocol::packet::Body;

        let _guard = FAULT_TEST_LOCK.lock().unwrap();
        let cases = [
            (Fault::Parse, 1, libc::EINVAL),
            (Fault::ThreadCheck, 2, libc::EBUSY),
            (Fault::AckPipe, 3, libc::EMFILE),
            (Fault::Fork, 4, libc::EAGAIN),
            (Fault::ChildTable, 5, libc::EUSERS),
            (Fault::Pidfd, 6, libc::ENOSYS),
            (Fault::ChildSetup, 200, libc::EIO),
            (Fault::Seccomp, 212, libc::EINVAL),
            (Fault::AckRead, 7, libc::ETIMEDOUT),
        ];
        for (fault_point, expected_stage, expected_errno) in cases {
            let (response, child_count) = launch_fault_result(fault_point);
            assert_eq!(response.request_id, 7);
            let Some(Body::Error(error)) = response.body else {
                panic!("fault {fault_point:?} emitted Started instead of Error");
            };
            assert_eq!(error.stage, expected_stage, "{fault_point:?}");
            assert_eq!(error.error_number, expected_errno as u32, "{fault_point:?}");
            assert_eq!(child_count, 0, "{fault_point:?} leaked a child-table entry");
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn startup_faults_send_no_started_packet_and_close_resources() {
        let _guard = FAULT_TEST_LOCK.lock().unwrap();
        for (fault_point, expected_stage) in [
            (Fault::TemplateState, Some(101)),
            (Fault::SignalSetup, Some(105)),
            (Fault::Allocation, Some(107)),
            (Fault::ReadySend, None),
        ] {
            let mut sockets = [-1; 2];
            // SAFETY: sockets has room for both sequenced-packet descriptors.
            assert_eq!(
                unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0, sockets.as_mut_ptr()) },
                0
            );
            set_test_fault(fault_point);
            // SAFETY: the child immediately enters the allocation-free native
            // harness and exits without unwinding copied Rust state.
            let pid = unsafe { libc::fork() };
            assert!(pid >= 0);
            if pid == 0 {
                // SAFETY: descriptors are owned by this fork branch.
                unsafe {
                    libc::close(sockets[1]);
                    if sockets[0] != CONTROL_FD {
                        if libc::dup2(sockets[0], CONTROL_FD) != CONTROL_FD {
                            libc::_exit(127);
                        }
                        libc::close(sockets[0]);
                    }
                    let _ = libc::syscall(libc::SYS_close_range, 3u32, CONTROL_FD as u32 - 1, 0u32);
                    let _ = libc::syscall(libc::SYS_close_range, CONTROL_FD as u32 + 1, u32::MAX, 0u32);
                    let mut nonce = [c_char::try_from(b'x').unwrap(); 33];
                    nonce[32] = 0;
                    let result = run_zygote(CONTROL_FD, nonce.as_mut_ptr(), blocking_entry, null_mut());
                    libc::_exit(result);
                }
            }
            // SAFETY: the parent owns its socket endpoint.
            unsafe { libc::close(sockets[0]) };
            let mut response = [0u8; 256];
            // SAFETY: response is writable and the peer owns the packet boundary.
            let received = unsafe { libc::recv(sockets[1], response.as_mut_ptr().cast(), response.len(), 0) };
            if let Some(expected_stage) = expected_stage {
                use crate::protocol::packet::Body;

                let packet = crate::protocol::decode(&response[..usize::try_from(received).unwrap()]).unwrap();
                let Some(Body::Error(error)) = packet.body else {
                    panic!("startup fault emitted a non-error packet")
                };
                assert_eq!(error.stage, expected_stage);
            } else {
                assert_eq!(received, 0);
            }
            let mut status = 0;
            // SAFETY: pid is this harness's unreaped child.
            assert_eq!(unsafe { libc::waitpid(pid, &raw mut status, 0) }, pid);
            assert!(libc::WIFEXITED(status));
            assert_eq!(libc::WEXITSTATUS(status), 125);
            // SAFETY: the parent owns its remaining endpoint.
            unsafe { libc::close(sockets[1]) };
            set_test_fault(Fault::None);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn launch_thread_diagnostic_rejects_an_additional_thread() {
        // SAFETY: gettid has no pointer arguments.
        let owner_tid = unsafe { libc::syscall(libc::SYS_gettid) as libc::pid_t };
        let proof = SingleThreaded {
            owner_tid,
            bypass_process_check: false,
        };
        let (started_sender, started_receiver) = std::sync::mpsc::channel();
        let (release_sender, release_receiver) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            started_sender.send(()).unwrap();
            release_receiver.recv().unwrap();
        });
        started_receiver.recv().unwrap();
        // SAFETY: the proof names this thread, while the deliberately live
        // helper thread makes the process-level invariant false.
        assert!(!unsafe { proof.is_current() });
        release_sender.send(()).unwrap();
        thread.join().unwrap();
    }

    #[cfg(feature = "private-test-util")]
    #[test]
    #[cfg_attr(miri, ignore)]
    fn specialization_stall_requires_the_private_environment_marker() {
        let mut environment = [c"ORDINARY=value".as_ptr().cast_mut(), null_mut()];
        let mut launch = Launch::empty();
        launch.envp = environment.as_mut_ptr();
        // SAFETY: environment is a live NUL-terminated vector for this call.
        assert!(unsafe { specialization_stall_path(addr_of!(launch)) }.is_null());
    }
}
