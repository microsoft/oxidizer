// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compares `WinHTTP`'s small-write behavior with calibrated Nagle controls.

#[cfg(not(windows))]
fn main() {
    eprintln!("This WinHTTP experiment only runs on Windows.");
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    windows::run()
}

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;
    use std::{ptr, thread};

    use anyhow::{Context, Result, anyhow, ensure};
    use windows_sys::Win32::Networking::WinHttp::{
        WINHTTP_ACCESS_TYPE_NO_PROXY, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpReceiveResponse,
        WinHttpSendRequest, WinHttpWriteData,
    };

    const TRIALS: usize = 7;

    pub(super) fn run() -> Result<()> {
        let address = std::env::var("NAGLE_RECEIVER")
            .context("set NAGLE_RECEIVER to the Linux receiver's IP address and port")?
            .parse()
            .context("NAGLE_RECEIVER must be an IP address and port")?;
        for _ in 0..TRIALS {
            raw_trial(address, false)?;
        }
        for _ in 0..TRIALS {
            raw_trial(address, true)?;
        }
        for _ in 0..TRIALS {
            winhttp_trial(address)?;
        }
        println!("External receiver completed all {TRIALS} trials per client.");
        Ok(())
    }

    fn raw_trial(address: SocketAddr, no_delay: bool) -> Result<()> {
        let mut stream = TcpStream::connect(address)?;
        stream.set_nodelay(no_delay)?;
        thread::sleep(Duration::from_millis(100));
        stream.write_all(b"a")?;
        thread::sleep(Duration::from_millis(5));
        stream.write_all(b"b")?;
        let mut completion = [0_u8; 1];
        stream.read_exact(&mut completion)?;
        ensure!(completion == *b"K", "external receiver returned an invalid completion");
        Ok(())
    }

    fn winhttp_trial(address: SocketAddr) -> Result<()> {
        let client = WinHttpUpload::open(address)?;
        client.send_headers()?;
        thread::sleep(Duration::from_millis(100));
        client.write(b"a")?;
        thread::sleep(Duration::from_millis(5));
        client.write(b"b")?;
        client.receive_response()
    }

    struct InternetHandle(*mut c_void);

    impl InternetHandle {
        fn new(handle: *mut c_void, operation: &'static str) -> Result<Self> {
            if handle.is_null() {
                return Err(last_error(operation));
            }
            Ok(Self(handle))
        }
    }

    impl Drop for InternetHandle {
        fn drop(&mut self) {
            // SAFETY: The handle is non-null, owned by this wrapper, and closed exactly once.
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }

    struct WinHttpUpload {
        _session: InternetHandle,
        _connection: InternetHandle,
        request: InternetHandle,
    }

    impl WinHttpUpload {
        fn open(address: SocketAddr) -> Result<Self> {
            let agent = wide("fetch-winhttp-nagle-probe");
            // SAFETY: All pointers reference valid, null-terminated UTF-16 strings for the call.
            let session = unsafe { WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_NO_PROXY, ptr::null(), ptr::null(), 0) };
            let session = InternetHandle::new(session, "WinHttpOpen")?;

            let host = wide(&address.ip().to_string());
            // SAFETY: The session is live and the host pointer remains valid for the call.
            let connection = unsafe { WinHttpConnect(session.0, host.as_ptr(), address.port(), 0) };
            let connection = InternetHandle::new(connection, "WinHttpConnect")?;

            let verb = wide("POST");
            let path = wide("/");
            // SAFETY: The connection is live and all UTF-16 pointers remain valid for the call.
            let request =
                unsafe { WinHttpOpenRequest(connection.0, verb.as_ptr(), path.as_ptr(), ptr::null(), ptr::null(), ptr::null(), 0) };
            let request = InternetHandle::new(request, "WinHttpOpenRequest")?;

            Ok(Self {
                _session: session,
                _connection: connection,
                request,
            })
        }

        fn send_headers(&self) -> Result<()> {
            // SAFETY: The request is live and this fixed-size upload supplies no initial body.
            if unsafe { WinHttpSendRequest(self.request.0, ptr::null(), 0, ptr::null_mut(), 0, 2, 0) } == 0 {
                return Err(last_error("WinHttpSendRequest"));
            }
            Ok(())
        }

        fn write(&self, bytes: &[u8]) -> Result<()> {
            let mut written = 0_u32;
            // SAFETY: The request is live and the byte slice remains valid for this synchronous
            // call. The output pointer refers to writable storage.
            if unsafe { WinHttpWriteData(self.request.0, bytes.as_ptr().cast(), bytes.len().try_into()?, &raw mut written) } == 0 {
                return Err(last_error("WinHttpWriteData"));
            }
            ensure!(written as usize == bytes.len(), "WinHttpWriteData performed a partial write");
            Ok(())
        }

        fn receive_response(&self) -> Result<()> {
            // SAFETY: The request is live and the reserved argument must be null.
            if unsafe { WinHttpReceiveResponse(self.request.0, ptr::null_mut()) } == 0 {
                return Err(last_error("WinHttpReceiveResponse"));
            }
            Ok(())
        }
    }

    fn last_error(operation: &'static str) -> anyhow::Error {
        anyhow!(
            "{operation} failed with Win32 error {}",
            std::io::Error::last_os_error().raw_os_error().unwrap_or_default()
        )
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
}
