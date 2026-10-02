// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(unsafe_op_in_unsafe_fn, reason = "benchmark constructor uses raw libc before Rust startup")]

use core::ffi::{c_char, c_int, c_void};
use core::mem::zeroed;
use core::ptr::{addr_of, addr_of_mut, null, null_mut};

const PR_SET_NO_NEW_PRIVS: c_int = 38;
const PR_SET_SECCOMP: c_int = 22;
const SECCOMP_MODE_FILTER: c_int = 2;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JMP_JEQ_K: u16 = 0x15;
const BPF_RET_K: u16 = 0x06;

#[repr(C)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *mut SockFilter,
}

#[used]
#[cfg_attr(target_os = "linux", unsafe(link_section = ".init_array"))]
static STARTUP: unsafe extern "C" fn() = configure;

unsafe extern "C" fn signal_handler(_: c_int) {}

#[expect(
    clippy::fn_to_numeric_cast_any,
    reason = "libc represents sigaction handlers as the integer type sighandler_t"
)]
// This benchmark-only constructor is observed by external process probes; the
// mutation test harness does not execute benchmark binaries or inspect signals.
#[cfg_attr(test, mutants::skip)]
fn signal_handler_address(handler: unsafe extern "C" fn(c_int)) -> libc::sighandler_t {
    handler as libc::sighandler_t
}

unsafe extern "C" fn configure() {
    let mut path = [0; 4096];
    let length = libc::readlink(c"/proc/self/exe".as_ptr(), path.as_mut_ptr(), path.len() - 1);
    if length < 0 {
        return;
    }
    *path.as_mut_ptr().add(length.cast_unsigned()) = 0;
    let mut name = path.as_ptr();
    let mut cursor = path.as_ptr();
    while *cursor != 0 {
        if *cursor == c_char::try_from(b'/').expect("ASCII slash fits in c_char") {
            name = cursor.add(1);
        }
        cursor = cursor.add(1);
    }
    if !libc::strstr(name, c"close_fallback".as_ptr()).is_null() && (!configure_limit(name) || !install_close_range_enosys()) {
        libc::_exit(124);
    }
    let handler = if !libc::strstr(name, c"signals_ignored".as_ptr()).is_null() {
        libc::SIG_IGN
    } else if !libc::strstr(name, c"signals_custom".as_ptr()).is_null() {
        signal_handler_address(signal_handler)
    } else {
        return;
    };
    let mut action: libc::sigaction = zeroed();
    action.sa_sigaction = handler;
    if libc::sigemptyset(addr_of_mut!(action.sa_mask)) != 0
        || libc::sigaction(libc::SIGUSR1, addr_of!(action), null_mut()) != 0
        || libc::sigaction(libc::SIGUSR2, addr_of!(action), null_mut()) != 0
    {
        libc::_exit(124);
    }
}

unsafe fn configure_limit(name: *const c_char) -> bool {
    let value = if !libc::strstr(name, c"_512".as_ptr()).is_null() {
        512
    } else if !libc::strstr(name, c"_4096".as_ptr()).is_null() {
        4096
    } else {
        return true;
    };
    let limit = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    libc::setrlimit(libc::RLIMIT_NOFILE, addr_of!(limit)) == 0
}

unsafe fn install_close_range_enosys() -> bool {
    let mut filter = [
        SockFilter {
            code: BPF_LD_W_ABS,
            jt: 0,
            jf: 0,
            k: 0,
        },
        SockFilter {
            code: BPF_JMP_JEQ_K,
            jt: 0,
            jf: 1,
            k: u32::try_from(libc::SYS_close_range).unwrap_or(u32::MAX),
        },
        SockFilter {
            code: BPF_RET_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_ERRNO | libc::ENOSYS as u32,
        },
        SockFilter {
            code: BPF_RET_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_ALLOW,
        },
    ];
    let program = SockFprog {
        len: u16::try_from(filter.len()).unwrap_or(u16::MAX),
        filter: filter.as_mut_ptr(),
    };
    libc::prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == 0
        && libc::prctl(
            PR_SET_SECCOMP,
            SECCOMP_MODE_FILTER,
            addr_of!(program).cast::<c_void>(),
            null::<c_void>(),
            null::<c_void>(),
        ) == 0
}
