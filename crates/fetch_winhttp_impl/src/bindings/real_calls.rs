// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Thread-local interception of native calls for the private wrapper tests.
//! Unintercepted calls still use WinHTTP, so other unit tests retain their backend.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::NonNull;

use windows::Win32::Networking::WinHttp as native;
use windows::core::{PCWSTR, Result};

use super::super::StatusCallback;

#[derive(Debug, PartialEq)]
enum Call {
    Open(usize, u32, usize, usize, u32),
    Timeouts(usize, i32, i32, i32, i32),
    Callback(usize, Option<usize>, u32, usize),
    Connect(usize, usize, u16, u32),
    Request(usize, usize, usize, usize, usize, usize, u32),
    Option(Option<usize>, u32, Option<Vec<u8>>),
    Send(usize, Option<Vec<u16>>, Option<usize>, u32, u32, usize),
    Write(usize, Option<usize>, u32, usize),
    Receive(usize, usize),
    Headers(usize, u32, usize, Option<usize>, u32, usize),
    Query(usize, u32, Option<usize>, u32),
    Read(usize, usize, u32, usize, u64, u32, bool),
    Close(usize),
}

struct Reply {
    handle: *mut c_void,
    callback: StatusCallback,
    result: Result<()>,
    status: u32,
}

type Interceptor = Box<dyn FnMut(Call) -> Reply>;
thread_local! {
    static INTERCEPTOR: RefCell<Option<Interceptor>> = const { RefCell::new(None) };
}

fn intercept(call: Call) -> Option<Reply> {
    INTERCEPTOR.with_borrow_mut(|hook| hook.as_mut().map(|hook| hook(call)))
}

pub(super) unsafe fn open(agent: PCWSTR, access: native::WINHTTP_ACCESS_TYPE, proxy: PCWSTR, bypass: PCWSTR, flags: u32) -> *mut c_void {
    if let Some(reply) = intercept(Call::Open(
        agent.as_ptr().addr(),
        access.0,
        proxy.as_ptr().addr(),
        bypass.as_ptr().addr(),
        flags,
    )) {
        return reply.handle;
    }
    // SAFETY: this wrapper forwards the caller's unchanged native arguments.
    unsafe { native::WinHttpOpen(agent, access, proxy, bypass, flags) }
}

pub(super) unsafe fn set_timeouts(handle: *mut c_void, resolve: i32, connect: i32, send: i32, receive: i32) -> Result<()> {
    if let Some(reply) = intercept(Call::Timeouts(handle.addr(), resolve, connect, send, receive)) {
        return reply.result;
    }
    // SAFETY: this wrapper forwards the caller's unchanged native arguments.
    unsafe { native::WinHttpSetTimeouts(handle, resolve, connect, send, receive) }
}

#[expect(clippy::fn_to_numeric_cast_any, reason = "the native callback ABI is compared by address")]
pub(super) unsafe fn set_status_callback(handle: *mut c_void, callback: StatusCallback, flags: u32, reserved: usize) -> StatusCallback {
    if let Some(reply) = intercept(Call::Callback(
        handle.addr(),
        callback.map(|callback| callback as usize),
        flags,
        reserved,
    )) {
        return reply.callback;
    }
    // SAFETY: this wrapper forwards the caller's unchanged native arguments.
    unsafe { native::WinHttpSetStatusCallback(handle, callback, flags, reserved) }
}

pub(super) unsafe fn connect(handle: *mut c_void, host: PCWSTR, port: u16, reserved: u32) -> *mut c_void {
    if let Some(reply) = intercept(Call::Connect(handle.addr(), host.as_ptr().addr(), port, reserved)) {
        return reply.handle;
    }
    // SAFETY: this wrapper forwards the caller's unchanged native arguments.
    unsafe { native::WinHttpConnect(handle, host, port, reserved) }
}

pub(super) unsafe fn open_request(
    handle: *mut c_void,
    method: PCWSTR,
    path: PCWSTR,
    version: PCWSTR,
    referer: PCWSTR,
    accept: *const PCWSTR,
    flags: native::WINHTTP_OPEN_REQUEST_FLAGS,
) -> *mut c_void {
    if let Some(reply) = intercept(Call::Request(
        handle.addr(),
        method.as_ptr().addr(),
        path.as_ptr().addr(),
        version.as_ptr().addr(),
        referer.as_ptr().addr(),
        accept.addr(),
        flags.0,
    )) {
        return reply.handle;
    }
    // SAFETY: this wrapper forwards the caller's unchanged native arguments.
    unsafe { native::WinHttpOpenRequest(handle, method, path, version, referer, accept, flags) }
}

pub(super) unsafe fn set_option(handle: Option<*const c_void>, option: u32, value: Option<&[u8]>) -> Result<()> {
    if let Some(reply) = intercept(Call::Option(handle.map(<*const c_void>::addr), option, value.map(<[u8]>::to_vec))) {
        return reply.result;
    }
    // SAFETY: this wrapper forwards the caller's unchanged native arguments.
    unsafe { native::WinHttpSetOption(handle, option, value) }
}

pub(super) unsafe fn send_request(
    handle: *mut c_void,
    headers: Option<&[u16]>,
    optional: Option<*const c_void>,
    optional_len: u32,
    total: u32,
    context: usize,
) -> Result<()> {
    if let Some(reply) = intercept(Call::Send(
        handle.addr(),
        headers.map(<[u16]>::to_vec),
        optional.map(<*const c_void>::addr),
        optional_len,
        total,
        context,
    )) {
        return reply.result;
    }
    // SAFETY: this wrapper forwards the caller's unchanged native arguments.
    unsafe { native::WinHttpSendRequest(handle, headers, optional, optional_len, total, context) }
}

pub(super) unsafe fn write_data(handle: *mut c_void, buffer: Option<*const c_void>, len: u32, written: *mut u32) -> Result<()> {
    if let Some(reply) = intercept(Call::Write(handle.addr(), buffer.map(<*const c_void>::addr), len, written.addr())) {
        return reply.result;
    }
    // SAFETY: this wrapper forwards the caller's unchanged native arguments.
    unsafe { native::WinHttpWriteData(handle, buffer, len, written) }
}

pub(super) unsafe fn receive_response(handle: *mut c_void, reserved: *mut c_void) -> Result<()> {
    if let Some(reply) = intercept(Call::Receive(handle.addr(), reserved.addr())) {
        return reply.result;
    }
    // SAFETY: this wrapper forwards the caller's unchanged native arguments.
    unsafe { native::WinHttpReceiveResponse(handle, reserved) }
}

pub(super) unsafe fn query_headers(
    handle: *mut c_void,
    level: u32,
    name: PCWSTR,
    buffer: Option<*mut c_void>,
    len: &mut u32,
    index: *mut u32,
) -> Result<()> {
    if let Some(reply) = intercept(Call::Headers(
        handle.addr(),
        level,
        name.as_ptr().addr(),
        buffer.map(<*mut c_void>::addr),
        *len,
        index.addr(),
    )) {
        return reply.result;
    }
    // SAFETY: this wrapper forwards the caller's unchanged native arguments.
    unsafe { native::WinHttpQueryHeaders(handle, level, name, buffer, len, index) }
}

pub(super) unsafe fn query_option(handle: *mut c_void, option: u32, buffer: Option<*mut c_void>, len: &mut u32) -> Result<()> {
    if let Some(reply) = intercept(Call::Query(handle.addr(), option, buffer.map(<*mut c_void>::addr), *len)) {
        return reply.result;
    }
    // SAFETY: this wrapper forwards the caller's unchanged native arguments.
    unsafe { native::WinHttpQueryOption(handle, option, buffer, len) }
}

pub(super) unsafe fn read_data_ex(
    handle: *mut c_void,
    buffer: *mut c_void,
    len: u32,
    read: *mut u32,
    flags: u64,
    property_count: u32,
    properties: Option<*const c_void>,
) -> u32 {
    if let Some(reply) = intercept(Call::Read(
        handle.addr(),
        buffer.addr(),
        len,
        read.addr(),
        flags,
        property_count,
        properties.is_some(),
    )) {
        return reply.status;
    }
    // SAFETY: this wrapper forwards the caller's unchanged native arguments.
    unsafe { native::WinHttpReadDataEx(handle, buffer, len, read, flags, property_count, properties) }
}

pub(super) unsafe fn close_handle(handle: *mut c_void) -> Result<()> {
    if let Some(reply) = intercept(Call::Close(handle.addr())) {
        return reply.result;
    }
    // SAFETY: this wrapper forwards the caller's unchanged native arguments.
    unsafe { native::WinHttpCloseHandle(handle) }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use widestring::U16CString;
    use windows::Win32::Foundation::{ERROR_IO_PENDING, SetLastError, WIN32_ERROR};
    use windows::Win32::Networking::WinHttp::WINHTTP_ACCESS_TYPE_NO_PROXY;
    use windows::core::HRESULT;

    use super::*;
    use crate::bindings::real::RealBindings;
    use crate::bindings::{Bindings, WINHTTP_READ_DATA_EX_FLAG_FILL_BUFFER};
    use crate::error::{Result as HttpResult, WinHttpOperation};
    use crate::handle::RawHandle;

    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            INTERCEPTOR.with_borrow_mut(|hook| *hook = None);
        }
    }

    fn check(expected: Call, fail: bool, operation: WinHttpOperation, run: impl FnOnce() -> HttpResult<()>) {
        let called = Rc::new(Cell::new(0));
        let observed = Rc::clone(&called);
        INTERCEPTOR.with_borrow_mut(|hook| {
            assert!(hook.is_none());
            *hook = Some(Box::new(move |actual| {
                observed.set(observed.get() + 1);
                assert_eq!(actual, expected);
                // SAFETY: the last-error slot is local to this test's thread.
                unsafe { SetLastError(WIN32_ERROR(1234)) };
                let callback = fail.then(|| {
                    // SAFETY: WinHTTP's documented invalid-callback sentinel is
                    // non-null and is only compared by address, never invoked.
                    unsafe {
                        std::mem::transmute::<usize, unsafe extern "system" fn(*mut c_void, usize, u32, *mut c_void, u32)>(usize::MAX)
                    }
                });
                Reply {
                    handle: if fail {
                        std::ptr::null_mut()
                    } else {
                        std::ptr::dangling_mut::<c_void>()
                    },
                    callback,
                    result: if fail {
                        Err(windows::core::Error::from_hresult(HRESULT::from_win32(1234)))
                    } else {
                        Ok(())
                    },
                    status: if fail { 1234 } else { ERROR_IO_PENDING.0 },
                }
            }));
        });
        let _reset = Reset;
        let result = run();
        assert_eq!(called.get(), 1);
        if fail {
            let error = result.unwrap_err();
            assert_eq!(error.code(), 1234);
            assert_eq!(error.operation(), operation);
        } else {
            result.unwrap();
        }
    }

    unsafe extern "system" fn callback(_: *mut c_void, _: usize, _: u32, _: *mut c_void, _: u32) {}

    #[test]
    #[expect(clippy::fn_to_numeric_cast_any, reason = "the native callback ABI is compared by address")]
    fn native_wrappers_preserve_arguments_and_failures() {
        let bindings = RealBindings;
        let pointer = std::ptr::dangling_mut::<c_void>();
        let handle = RawHandle::new(pointer).unwrap();
        let address = pointer.addr();
        let agent = U16CString::from_str("agent").unwrap();
        let method = U16CString::from_str("PATCH").unwrap();
        let host = U16CString::from_str("example.test").unwrap();
        let path = U16CString::from_str("/resource?q=1").unwrap();
        let bytes = [1_u8, 2, 3];
        for fail in [false, true] {
            check(
                Call::Open(agent.as_ptr().addr(), WINHTTP_ACCESS_TYPE_NO_PROXY.0, 0, 0, 0x1000_0000),
                fail,
                WinHttpOperation::Open,
                || {
                    // SAFETY: interception prevents native access and agent stays live.
                    unsafe { bindings.open(&agent, 0x1000_0000) }.map(|result| assert_eq!(result.as_ptr(), pointer))
                },
            );
            check(Call::Timeouts(address, -1, 17, 29, 41), fail, WinHttpOperation::SetTimeouts, || {
                // SAFETY: the exact call is intercepted before any native access.
                unsafe { bindings.set_timeouts(handle, -1, 17, 29, 41) }
            });
            check(
                Call::Connect(address, host.as_ptr().addr(), 8443, 0),
                fail,
                WinHttpOperation::Connect,
                || {
                    // SAFETY: interception prevents native access and host stays live.
                    unsafe { bindings.connect(handle, &host, 8443) }.map(|result| assert_eq!(result.as_ptr(), pointer))
                },
            );
            check(
                Call::Request(address, method.as_ptr().addr(), path.as_ptr().addr(), 0, 0, 0, 0x0080_0000),
                fail,
                WinHttpOperation::OpenRequest,
                || {
                    // SAFETY: interception prevents native access; both strings stay live.
                    unsafe { bindings.open_request(handle, &method, &path, 0x0080_0000) }.map(|result| assert_eq!(result.as_ptr(), pointer))
                },
            );
            check(
                Call::Option(Some(address), 31, Some(bytes.to_vec())),
                fail,
                WinHttpOperation::SetOption,
                // SAFETY: interception prevents native access and bytes stays live.
                || unsafe { bindings.set_option(handle, 31, &bytes) },
            );
            check(Call::Close(address), fail, WinHttpOperation::CloseHandle, || {
                // SAFETY: interception prevents closing the fake handle.
                unsafe { bindings.close_handle(handle) }
            });
        }
        for fail in [false, true] {
            check(
                Call::Callback(address, Some(callback as *const () as usize), 0x0038_0000, 0),
                fail,
                WinHttpOperation::SetStatusCallback,
                // SAFETY: interception prevents native access; callback is a static function.
                || unsafe { bindings.set_status_callback(handle, Some(callback), 0x0038_0000) },
            );
        }
    }

    #[test]
    fn native_data_wrappers_preserve_arguments_and_failures() {
        let bindings = RealBindings;
        let pointer = std::ptr::dangling_mut::<c_void>();
        let handle = RawHandle::new(pointer).unwrap();
        let address = pointer.addr();
        let headers = U16CString::from_str("x-test: value\r\n").unwrap();
        let mut bytes = [1_u8, 2, 3];
        let buffer = NonNull::from(&mut bytes[0]);
        for fail in [false, true] {
            check(
                Call::Send(address, Some(headers.as_slice().to_vec()), None, 0, 79, 0x1234),
                fail,
                WinHttpOperation::SendRequest,
                // SAFETY: interception prevents native access; headers stays live.
                || unsafe { bindings.send_request(handle, &headers, 79, 0x1234) },
            );
            for (data, len) in [(Some(buffer), 3), (None, 0)] {
                check(
                    Call::Write(address, data.map(|pointer| pointer.as_ptr().addr()), len, 0),
                    fail,
                    WinHttpOperation::WriteData,
                    // SAFETY: interception prevents native access; buffers stay live.
                    || unsafe { bindings.write_data(handle, data, len) },
                );
            }
            check(Call::Receive(address, 0), fail, WinHttpOperation::ReceiveResponse, || {
                // SAFETY: interception prevents native access to the fake handle.
                unsafe { bindings.receive_response(handle) }
            });
            for data in [Some(buffer), None] {
                let mut len = 3;
                check(
                    Call::Headers(address, 19, 0, data.map(|pointer| pointer.as_ptr().addr()), 3, 0),
                    fail,
                    WinHttpOperation::QueryHeaders,
                    // SAFETY: interception prevents native access; buffers stay live.
                    || unsafe { bindings.query_headers(handle, 19, data, &mut len) },
                );
                check(
                    Call::Query(address, 37, data.map(|pointer| pointer.as_ptr().addr()), 3),
                    fail,
                    WinHttpOperation::QueryOption,
                    // SAFETY: interception prevents native access; buffers stay live.
                    || unsafe { bindings.query_option(handle, 37, data, &mut len) },
                );
            }
            for fill in [false, true] {
                let flags = if fill { WINHTTP_READ_DATA_EX_FLAG_FILL_BUFFER } else { 0 };
                check(
                    Call::Read(address, buffer.as_ptr().addr(), 3, 0, flags, 0, false),
                    fail,
                    WinHttpOperation::ReadData,
                    // SAFETY: interception prevents native access; the buffer stays live.
                    || unsafe { bindings.read_data_ex(handle, buffer, 3, fill) },
                );
            }
        }
    }
}
