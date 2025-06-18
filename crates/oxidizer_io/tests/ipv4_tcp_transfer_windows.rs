// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(windows)]
#![cfg(feature = "unstable-testing")]
#![cfg(not(miri))] // Miri cannot talk to real OS.

use std::ffi::c_void;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::{mem, ptr, slice};

use bytes::BufMut;
use futures::executor::LocalSpawner;
use futures::task::LocalSpawnExt;
use oxidizer_io::mem::SequenceBuilder;
use oxidizer_io::testing::{IoPumpMode, with_io_test_harness_ex};
use oxidizer_io::{
    AsNativePrimitiveExt, BeginResult, BoundPrimitive, Context, Error, ReserveOptions,
    SystemTaskCategory, winsock,
};
use oxidizer_testing::log_to_console_and_file;
use scopeguard::{ScopeGuard, guard};
use tracing::{Level, event};
use windows::Win32::Networking::WinSock::{
    AF_INET, AcceptEx, GetAcceptExSockaddrs, IN_ADDR, INADDR_LOOPBACK, IPPROTO_TCP,
    SIO_GET_EXTENSION_FUNCTION_POINTER, SO_UPDATE_ACCEPT_CONTEXT, SOCK_STREAM, SOCKADDR,
    SOCKADDR_IN, SOCKET, SOL_SOCKET, SOMAXCONN, WSA_FLAG_OVERLAPPED, WSABUF, WSAENOTCONN,
    WSAID_CONNECTEX, WSAIoctl, WSARecv, WSASend, WSASendDisconnect, WSASocketW, bind, closesocket,
    getsockname, htonl, htons, listen, ntohl, ntohs, setsockopt,
};
use windows::Win32::System::IO::OVERLAPPED;
use windows::core::{BOOL, PSTR};

// We run this test using all the I/O models we support, to demonstrate:
// 1. That all the I/O models work.
// 2. How you can write generic code that works with all of them.
#[test]
fn ipv4_tcp_transfer_windows() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = log_to_console_and_file("ipv4_tcp_transfer_windows.log");

    with_io_test_harness_ex(None, IoPumpMode::Always, async move |harness| {
        // We setup a TCP server that will listen for one connection and receive a bunch of data
        // from that connection. Then we start a TCP client that will connect to the server and
        // transmit a bunch of data. Once done, both parties gracefully disconnect and we are done.
        let (port, server) = start_tcp_server(&harness.context, &harness.spawner).await?;
        let client = run_tcp_client(port, &harness.context);

        let (server_result, client_result) = futures::future::join(server, client).await;
        server_result?;
        Ok(client_result?)
    })
}

/// Starts the TCP server on a random port, returning the port number and
/// a future that can be awaited to detect when the server has completed its work.
#[expect(clippy::too_many_lines, reason = "test code")]
async fn start_tcp_server(
    io_context: &Context,
    spawner: &LocalSpawner,
) -> oxidizer_io::Result<(u16, impl Future<Output = oxidizer_io::Result<()>>)> {
    winsock::ensure_initialized();

    // We create the listen socket in a system task because it involves blocking syscalls.
    let (port, listen_socket_raw) = io_context
        .execute_system_task(
            SystemTaskCategory::Default,
            move || -> oxidizer_io::Result<_> {
                // We need a socket to receive incoming connection on. IPv4 only in this test.
                let listen_socket_raw = guard(
                    // SAFETY: No safety requirements.
                    unsafe {
                        WSASocketW(
                            AF_INET.0.into(),
                            SOCK_STREAM.0,
                            IPPROTO_TCP.0,
                            None,
                            0,
                            WSA_FLAG_OVERLAPPED,
                        )?
                    },
                    |socket| {
                        // SAFETY: No safety requirements.
                        unsafe {
                            closesocket(socket);
                        }
                    },
                );

                event!(
                    Level::INFO,
                    message = "server socket created",
                    ?listen_socket_raw
                );

                // We have a socket. Now we need to bind it to some port to start listening on.

                let mut addr = IN_ADDR::default();

                // We only listen on localhost to keep the example simple.
                // SAFETY: No safety requirements.
                addr.S_un.S_addr = unsafe { htonl(INADDR_LOOPBACK) };

                let socket_addr = SOCKADDR_IN {
                    sin_family: AF_INET,
                    // Have the OS choose a random free port for us.
                    sin_port: 0,
                    sin_addr: addr,
                    sin_zero: [0; 8],
                };

                // SAFETY: No safety requirements - the pointer simply needs to be valid during the call.
                winsock::status_code_to_result(unsafe {
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_possible_wrap,
                        reason = "known constant in safe range"
                    )]
                    bind(
                        *listen_socket_raw,
                        ptr::from_ref(&socket_addr).cast::<SOCKADDR>(),
                        mem::size_of::<SOCKADDR_IN>() as i32,
                    )
                })?;

                // Binding successful! What did we bind to? Let's find out what port we were assigned.

                let mut socket_addr = SOCKADDR_IN::default();

                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_possible_wrap,
                    reason = "known constant in safe range"
                )]
                let mut socket_addr_size: i32 = mem::size_of::<SOCKADDR_IN>() as i32;

                // SAFETY: No safety requirements - the pointer simply needs to be valid during the call.
                winsock::status_code_to_result(unsafe {
                    getsockname(
                        *listen_socket_raw,
                        (&raw mut socket_addr).cast(),
                        &raw mut socket_addr_size,
                    )
                })?;

                // SAFETY: No safety requirements.
                let port = unsafe { ntohs(socket_addr.sin_port) };

                event!(Level::INFO, message = "server socket bound", ?port);

                // Enter listening mode on the socket. This starts accepting incoming TCP connections.
                #[expect(
                    clippy::cast_possible_wrap,
                    reason = "safe cast - just Win32 API being silly with types"
                )]
                // SAFETY: No safety requirements.
                winsock::status_code_to_result(unsafe {
                    listen(*listen_socket_raw, SOMAXCONN as i32)
                })?;

                event!(Level::INFO, message = "server socket listening");

                Ok((port, listen_socket_raw))
            },
        )
        .await?;

    // The server socket is ready to receive connections. We asynchronously kick off the connection
    // accept+processing loop as another task and return to the test entrypoint to start the client.
    // This channel will be used by the server logic once it has completed its part of the test.
    let (done_tx, done_rx) = oneshot::channel();

    spawner
        .spawn_local({
            let io_context = io_context.clone();

            async move {
                let result = listen_and_process_incoming(
                    // Callee takes over responsibility for proper disposal of socket.
                    ScopeGuard::into_inner(listen_socket_raw),
                    &io_context,
                )
                .await;

                // We report the result to the test entrypoint here, which will verify
                // that both client and server parts of the test logic declared success.
                _ = done_tx.send(result);
            }
        })
        .unwrap();

    Ok((port, async move {
        #[expect(
            clippy::map_err_ignore,
            reason = "original error is useless 'broken channel'"
        )]
        done_rx.await.map_err(|_| {
            oxidizer_io::Error::ContractViolation(
                "TCP server task died before reporting result".to_string(),
            )
        })?
    }))
}

#[expect(clippy::too_many_lines, reason = "test code")]
async fn listen_and_process_incoming(
    listen_socket_raw: SOCKET,
    io_context: &Context,
) -> oxidizer_io::Result<()> {
    // The AcceptEx function supports receiving a piece of data as part of the accept call,
    // which may provide a performance boost when accepting the connection. This is optional
    // and for now we disable this via setting dwReceiveDataLength to 0.
    //
    // Buffer contents (not in order):
    // * Local address
    // * Remote address
    // * (Optional) first block of data received
    //
    // Reference of relevant length calculations:
    // bRetVal = lpfnAcceptEx(ListenSocket, AcceptSocket, lpOutputBuf,
    //      outBufLen - ((sizeof (sockaddr_in) + 16) * 2),
    //      sizeof (sockaddr_in) + 16, sizeof (sockaddr_in) + 16,
    //      &dwBytes, &olOverlap);

    // The data length in the accept buffer (if we were to want to use some) would be the buffer
    // size minus double of this (local + remote address).
    const ADDRESS_LENGTH: usize = mem::size_of::<SOCKADDR_IN>() + 16;

    // As we have no data size, our buffer is just two addresses.
    const BUFFER_LENGTH: usize = 2 * ADDRESS_LENGTH;

    event!(
        Level::INFO,
        message = "preparing to listen for incoming connection"
    );

    // The I/O subsystem takes responsibility for proper release of the socket, our hands are clean.
    let listen_socket = io_context.bind_primitive(listen_socket_raw)?;

    // We need to create a new socket for the incoming connection.
    let connection_socket_raw = Arc::new(guard(
        // SAFETY: No safety requirements, just need to avoid leaking the handle.
        unsafe {
            WSASocketW(
                AF_INET.0.into(),
                SOCK_STREAM.0,
                IPPROTO_TCP.0,
                None,
                0,
                WSA_FLAG_OVERLAPPED,
            )?
        },
        |s| {
            // SAFETY: No safety requirements.
            unsafe {
                closesocket(s);
            }
        },
    ));

    // Since we need a fixed-size contiguous buffer here, we cannot use dedicated I/O memory as
    // it does not guarantee contiguousness. Instead, we allocate our own memory for this. This
    // buffer does not need to be aligned, just some bytes will do.
    //
    // We are the only one accessing this, the Arc/Mutex is simply used to extend the lifetime
    // to cover the entire operation, even if the future is dropped by our caller.
    //
    // User resources must be thread-safe due to limitations of the Rust language - it is today
    // not possible to write generic code that adapts to the thread-safety of the I/O model because
    // that would require generic behavior over constraints (traits) but generics only work over
    // types.
    //
    // The Mutex is only to make the compiler happy - it is never actually contended.
    let accept_buffer = Arc::new(Mutex::new(Vec::<u8>::with_capacity(BUFFER_LENGTH)));

    event!(Level::INFO, message = "listening for incoming connection");

    // NOTE: accept() is an operation on the **listen socket**, not on the connection socket, so it
    // is bound to the completion port of the listen socket. Note that we have not yet bound the
    // connection socket to any completion port - it remains unbound until it is fully connected.
    listen_socket
        .control()
        // We need to keep our buffer alive for the duration of the operation.
        .with_resources(Arc::clone(&accept_buffer))
        .begin({
            let accept_buffer = Arc::clone(&accept_buffer);
            let connection_socket_raw = Arc::clone(&connection_socket_raw);

            move |primitive, mut args| {
                // Not used; we receive 0 bytes of payload, just need something to reference here.
                let mut bytes_received: u32 = 0;

                // SAFETY: We are not allowed to reuse this for multiple calls and we are only
                // allowed to use it with the primitive given to this callback. We obey the rules.
                let overlapped = unsafe { args.overlapped() };

                // SAFETY: The buffer must be kept alive for the duration of the operation, which is
                // guaranteed by the I/O subsystem via the Rc we gave to with_resources(). The buffer
                // must have the correct size, which it does, see calculations at function start.
                let accept_result = unsafe {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "Tiny constants fit in u32 just fine"
                    )]
                    AcceptEx(
                        *primitive
                            .try_as_socket()
                            .expect("we know that we are working on a socket"),
                        **connection_socket_raw,
                        accept_buffer
                            .lock()
                            .expect("poisoned lock")
                            .as_mut_ptr()
                            .cast(),
                        0,
                        ADDRESS_LENGTH as u32,
                        ADDRESS_LENGTH as u32,
                        &raw mut bytes_received,
                        overlapped,
                    )
                };

                event!(
                    Level::TRACE,
                    message = "accept operation started",
                    result = ?accept_result
                );

                assert_eq!(0, bytes_received);

                // Note that this returned a BOOL, not a Winsock result code,
                // so result handling follows Win32 style instead of Winsock style.
                BeginResult::from_bool(accept_result)
            }
        })
        .await?;

    let connection_socket_raw = Arc::into_inner(connection_socket_raw)
        .expect("the only other reference was the callback above and we awaited it already");

    // AcceptEx filled our buffer with some addresses (and would also have put some payload data
    // in there if we asked - maybe a future optimization in some iteration). Now we need to extract
    // this data. There is a convenience function provided for this in Winsock.
    event!(
        Level::TRACE,
        "incoming connection accepted; identifying addresses"
    );

    // There are read-only views over the data in our accept_buffer.
    let mut local_addr: *const SOCKADDR = ptr::null_mut();
    let mut local_addr_len: i32 = 0;
    let mut remote_addr: *const SOCKADDR = ptr::null_mut();
    let mut remote_addr_len: i32 = 0;

    // This function will replace the pointers above to point to the actual data in question.
    // SAFETY: No safety requirements beyond passing valid inputs.
    unsafe {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "Tiny constants fit in u32 just fine"
        )]
        GetAcceptExSockaddrs(
            accept_buffer.lock().expect("poisoned lock").as_ptr().cast(),
            0,
            ADDRESS_LENGTH as u32,
            ADDRESS_LENGTH as u32,
            (&raw mut local_addr).cast(),
            &raw mut local_addr_len,
            (&raw mut remote_addr).cast(),
            &raw mut remote_addr_len,
        );
    }

    event!(
        Level::INFO,
        message = "connection accepted",
        local_addr = sockaddr_to_string(local_addr),
        remote_addr = sockaddr_to_string(remote_addr)
    );

    // We need to refer to this via pointer, so let's copy the raw value out to a place first.
    let listen_socket_raw = listen_socket
        .try_as_socket()
        .expect("we know we are dealing with a socket")
        .0;

    // SAFETY: The size is right, so creating the slice is OK. We only use it for the single
    // call on the next line, so no lifetime concerns - the slice is gone before the storage
    // goes away in all cases.
    let listen_socket_as_slice = unsafe {
        slice::from_raw_parts(
            ptr::from_ref::<usize>(&listen_socket_raw).cast(),
            mem::size_of::<usize>(),
        )
    };

    // This does some internal updates in the socket. The documentation is a little vague about
    // what this accomplishes but if we have to do it, we have to do it.
    //
    // SAFETY: No safety requirements beyond passing valid pointers that outlive this call.
    winsock::status_code_to_result(unsafe {
        setsockopt(
            *connection_socket_raw,
            SOL_SOCKET,
            SO_UPDATE_ACCEPT_CONTEXT,
            Some(listen_socket_as_slice),
        )
    })?;

    // Our connection socket is now ready to be used! The I/O subsystem takes ownership
    // of the socket here and becomes responsible for its proper disposal.
    let connection_socket =
        io_context.bind_primitive(ScopeGuard::into_inner(connection_socket_raw))?;

    // Oh and we do not need the listen socket anymore because this
    // test is not going to be listening for any more connections.
    drop(listen_socket);

    // We just read from the socket in a loop and exit when we see the connection is closed.
    let mut total_received: usize = 0;

    loop {
        const MAX_READ_SIZE: usize = 64 * 1024;

        let buffer = io_context.reserve(MAX_READ_SIZE, ReserveOptions::default());

        let received_bytes = socket_recv(&connection_socket, buffer).await?;

        if received_bytes.is_empty() {
            event!(
                Level::INFO,
                message = "client has closed the connection - server loop terminating",
                total_received
            );
            break;
        }

        total_received = total_received.saturating_add(received_bytes.len());

        event!(
            Level::INFO,
            message = "received packet of data",
            byte_count = received_bytes.len(),
            total_received
        );
    }

    event!(Level::INFO, message = "disconnecting from client");

    // Graceful shutdown requires that disconnect be signaled first. We do not perhaps strictly
    // need it in this test because we know the client has already disconnected first but as a
    // matter of general sockets API hygiene, let's do this call even if it is useless here.

    // SAFETY: No safety requirements.
    let disconnect_status_code = unsafe {
        WSASendDisconnect(
            *connection_socket
                .try_as_socket()
                .expect("we know it is a socket"),
            None,
        )
    };
    winsock::status_code_to_result(disconnect_status_code)?;

    Ok(())
}

/// Executes the TCP client logic, connecting to the given port and sending a bunch of data,
/// returning once all data has been successfully sent and the connection closed.
#[expect(clippy::too_many_lines, reason = "test code")]
async fn run_tcp_client(port: u16, io_context: &Context) -> oxidizer_io::Result<()> {
    winsock::ensure_initialized();

    // We first need a socket before we can even start connecting to something.
    let socket_raw = guard(
        // SAFETY: No safety requirements, just need to avoid leaking the handle.
        unsafe {
            WSASocketW(
                AF_INET.0.into(),
                SOCK_STREAM.0,
                IPPROTO_TCP.0,
                None,
                0,
                WSA_FLAG_OVERLAPPED,
            )?
        },
        |s| {
            // SAFETY: No safety requirements.
            unsafe {
                closesocket(s);
            }
        },
    );

    event!(Level::INFO, message = "client socket created", ?socket_raw);

    // We will need this later, might as well grab it now.
    let connectex_fn = extract_connectex_fn(*socket_raw)?;

    // This socket needs to be bound to a local address, so let's do that.
    let mut local_addr = IN_ADDR::default();

    // This test is only over the loopback adapter.
    // SAFETY: No safety requirements.
    local_addr.S_un.S_addr = unsafe { htonl(INADDR_LOOPBACK) };

    let socket_addr = SOCKADDR_IN {
        sin_family: AF_INET,
        // Have the OS choose a random free port for us.
        sin_port: 0,
        sin_addr: local_addr,
        sin_zero: [0; 8],
    };

    // SAFETY: No safety requirements - the pointer simply needs to be valid during the call.
    winsock::status_code_to_result(unsafe {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_possible_wrap,
            reason = "No way size_of will overflow i32"
        )]
        bind(
            *socket_raw,
            ptr::from_ref(&socket_addr).cast::<SOCKADDR>(),
            mem::size_of::<SOCKADDR_IN>() as i32,
        )
    })?;

    // We have been bound to a local port, now we can connect. This is the start of asynchronous
    // I/O so we also need to bind the socket to the I/O context now. The I/O subsystem takes
    // responsibility for proper disposal of the socket.
    let socket = io_context.bind_primitive(ScopeGuard::into_inner(socket_raw))?;

    // We need to create the remote address of the endpoint we are going to connect to.
    let mut remote_addr = IN_ADDR::default();

    // We are connecting to localhost.
    // SAFETY: No safety requirements.
    remote_addr.S_un.S_addr = unsafe { htonl(INADDR_LOOPBACK) };

    let remote_socket_addr = SOCKADDR_IN {
        sin_family: AF_INET,
        // The port number is whatever the caller says the port number is.
        // SAFETY: No safety requirements.
        sin_port: unsafe { htons(port) },
        sin_addr: remote_addr,
        sin_zero: [0; 8],
    };

    event!(
        Level::INFO,
        message = "connecting to server",
        remote_socket_addr = sockaddr_in_to_string(&remote_socket_addr)
    );

    // ConnectEx() can theoretically also take the first piece of data to send as part of a fast
    // connect logic. However, we do not make use of this capability here. Option for the future.
    socket
        .control()
        .begin(move |primitive, mut args| {
            // SAFETY: We are not allowed to reuse this for multiple calls and we are only
            // allowed to use it with the primitive given to this callback. We obey the rules.
            let overlapped = unsafe { args.overlapped() };

            // SAFETY: We do not need to keep any of the inputs alive after the call, so there are
            // no safety requirements to fulfill beyond passing valid inputs.
            let connectex_result = unsafe {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "Tiny constants fit in u32 just fine"
                )]
                connectex_fn(
                    *primitive
                        .try_as_socket()
                        .expect("we know that we are working on a socket"),
                    ptr::from_ref(&remote_socket_addr).cast(),
                    mem::size_of::<SOCKADDR_IN>() as u32,
                    ptr::null(),
                    0,
                    ptr::null_mut(),
                    overlapped,
                )
            };

            event!(
                Level::TRACE,
                message = "connect operation started",
                result = ?connectex_result
            );

            // Note that this returned a BOOL, not a Winsock result code,
            // so result handling follows Win32 style instead of Winsock style.
            BeginResult::from_bool(connectex_result)
        })
        .await?;

    event!(Level::INFO, message = "connected to server");

    // We are connected! Send some data in a loop now.
    let mut bytes_sent = 0;

    loop {
        const STOP_AFTER_BYTES: usize = 1024 * 1024;
        const MAX_SEND_SIZE: usize = 100_000;

        // How many chunks we can process per one vectored I/O operation.
        // Arbitrary choice; most real-world writes might not be vectored.
        // Bigger is better but only if used, and also increases consumed stack space.
        const MAX_CHUNKS: usize = 16;

        let send_size = MAX_SEND_SIZE.min(STOP_AFTER_BYTES.saturating_sub(bytes_sent));

        if send_size == 0 {
            break;
        }

        let mut buffer = io_context.reserve(send_size, ReserveOptions::default());
        // Fill the buffer with some arbitrary payload data.
        buffer.put_bytes(123, send_size);
        let buffer = buffer.consume_all();

        socket
            .write_bytes::<MAX_CHUNKS>(buffer)
            .begin(move |primitive, mut args| {
                let mut buffers = heapless::Vec::<WSABUF, MAX_CHUNKS>::new();

                for chunk in args.chunks() {
                    buffers
                        .push(WSABUF {
                            #[expect(
                                clippy::cast_possible_truncation,
                                reason = "Tiny constants fit in u32 just fine"
                            )]
                            len: chunk.len() as u32,
                            // PSTR is defined as a *mut but it is only used for reading here, so it's fine.
                            buf: PSTR::from_raw(chunk.as_ptr().cast_mut()),
                        })
                        .expect("guarded by MAX_CHUNKS");
                }

                // SAFETY: We are not allowed to reuse this for multiple calls and we are only
                // allowed to use it with the primitive given to this callback. We obey the rules.
                let overlapped = unsafe { args.overlapped() };

                // SAFETY: Buffer safety is taken care of by I/O subsystem, nothing else we need
                // to worry about - none of the other inputs need to outlive the call.
                let status_code = unsafe {
                    WSASend(
                        *primitive.try_as_socket().expect("we know it is a socket"),
                        buffers.as_slice(),
                        Some(args.bytes_written_synchronously_as_mut()),
                        0,
                        Some(overlapped),
                        None,
                    )
                };

                winsock::status_code_to_begin_result(status_code)
            })
            .await?;

        bytes_sent = bytes_sent
            .checked_add(send_size)
            .expect("usize overflow is inconceivable here");

        event!(
            Level::INFO,
            message = "sent packet of data",
            byte_count = send_size,
            bytes_sent
        );
    }

    event!(
        Level::INFO,
        message = "we sent everything we wanted to send; disconnecting from server",
        bytes_sent
    );

    socket_graceful_shutdown(socket, io_context).await?;

    Ok(())
}

async fn socket_recv(
    socket: &BoundPrimitive,
    buffer: SequenceBuilder,
) -> oxidizer_io::Result<SequenceBuilder> {
    let (_bytes_read, sb) = socket
        .read_bytes(buffer)
        .begin(move |primitive, mut args| {
            // Hardcoded count to avoid dynamic memory allocation.
            const MAX_BUFFERS_PER_READ: usize = 16;

            let mut buffers = heapless::Vec::<WSABUF, MAX_BUFFERS_PER_READ>::new();

            for chunk in args.iter_chunks() {
                if buffers.len() == MAX_BUFFERS_PER_READ {
                    // We cannot use more buffers for this read.
                    break;
                }

                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "truncation unrealistic - nobody is giving us a buffer greater than u32 here because this is strictly controlled test logic (but real implementations should be more careful)"
                )]
                let buffer = WSABUF {
                    len: chunk.len() as u32,
                    buf: PSTR::from_raw(chunk.as_mut_ptr().cast()),
                };

                buffers
                    .push(buffer)
                    .expect("we checked we have enough capacity at top of loop");
            }

            let mut flags: u32 = 0;

            // SAFETY: We are not allowed to reuse this for multiple calls and we are only
            // allowed to use it with the primitive given to this callback. We obey the rules.
            let overlapped = unsafe { args.overlapped() };

            // SAFETY: The array of buffers only needs to outlive this call. The real memory is
            // managed by the I/O subsystem. Nothing else to worry about.
            let status_code = unsafe {
                WSARecv(
                    *primitive.try_as_socket().expect("we know it is a socket"),
                    buffers.as_slice(),
                    Some(&raw mut *args.bytes_read_synchronously_as_mut()),
                    &raw mut flags,
                    Some(overlapped),
                    None,
                )
            };

            winsock::status_code_to_begin_result(status_code)
        })
        .await?;

    Ok(sb)
}

async fn socket_graceful_shutdown(
    socket: BoundPrimitive,
    io_context: &Context,
) -> oxidizer_io::Result<()> {
    // A graceful disconnect is a multi-step process:
    // 1. Signal that we are done sending data.
    // 2. Read data until we get a 0 byte response, indicating the peer has received all the data.
    // 3. Close the socket.
    // Step 2 is critical to avoid dirty shutdown.

    event!(Level::INFO, message = "disconnecting from peer", ?socket);

    // System task returns None if the socket is already closed and no further action is needed.
    let Some(socket) = io_context
        .execute_system_task(SystemTaskCategory::Default, move || {
            let socket_raw = socket.try_as_socket().expect("we know it is a socket");

            // SAFETY: No safety requirements.
            let disconnect_status_code = unsafe { WSASendDisconnect(*socket_raw, None) };

            event!(
                Level::INFO,
                message = "disconnect command issued",
                result = ?disconnect_status_code,
                ?socket_raw
            );

            match winsock::status_code_to_result(disconnect_status_code) {
                // It is a bit strange to get a "socket is not connected" error from a "disconnect"
                // call but this does seem to happen, at least on the client side of things, even when
                // us calling this is the reason the socket becomes "not connected" in the first place.
                // Whatever. If the socket is not connected, we can just return immediately here. On the
                // server side, this does not seem to always return the error, at least.
                Err(Error::Winsock(e)) if e == WSAENOTCONN => {
                    event!(Level::INFO, message = "disconnected with immediate effect");
                    // We are already disconnected - no point doing the read-until-nothing dance.
                    Ok(None)
                }
                // Disconnect is in progress. We may need to issue a read to get it to take effect.
                Ok(()) => Ok(Some(socket)),
                Err(e) => Err(e),
            }
        })
        .await?
    else {
        // Already disconnected
        return Ok(());
    };

    let buffer = io_context.reserve(1, ReserveOptions::default());
    let received_bytes = socket_recv(&socket, buffer).await?;

    if received_bytes.is_empty() {
        event!(Level::INFO, message = "disconnected from peer");
        // The peer has closed the connection.
        return Ok(());
    }

    event!(
        Level::ERROR,
        message = "encountered incoming data from peer during graceful disconnect"
    );

    // There was data remaining?! This may be normal for some protocols (e.g. with an infinite data
    // stream that never ends) but those protocols should not be calling this shutdown method.
    Err(oxidizer_io::Error::ContractViolation(
        "we had not completed processing data from the peer when graceful shutdown was initiated"
            .to_string(),
    ))
}

fn sockaddr_to_string(addr: *const SOCKADDR) -> String {
    // SAFETY: We read what we were given; it is the caller's problem if it is invalid.
    let a = unsafe { &*addr };

    match a.sa_family {
        AF_INET => sockaddr_in_to_string(addr.cast()),
        _ => "(unknown address family)".to_string(),
    }
}

fn sockaddr_in_to_string(addr: *const SOCKADDR_IN) -> String {
    // SAFETY: We read what we were given; it is the caller's problem if it is invalid.
    let a = unsafe { &*addr };

    // SAFETY: No safety requirements.
    let port = unsafe { ntohs(a.sin_port) };

    // SAFETY: This union is always valid in every variant.
    let ip_bits_n = unsafe { a.sin_addr.S_un.S_addr };
    // SAFETY: No safty requirements.
    let ip_bits = unsafe { ntohl(ip_bits_n) };
    let ip_address = Ipv4Addr::from_bits(ip_bits);

    format!("{ip_address}:{port}")
}

// https://learn.microsoft.com/en-us/windows/win32/api/mswsock/nc-mswsock-lpfn_connectex
type ConnectExFn = unsafe extern "C" fn(
    s: SOCKET,
    name: *const SOCKADDR,
    namelen: u32,
    lpsendbuffer: *const c_void,
    dwsenddatalength: u32,
    lpdwbytessend: *mut u32,
    lpoverlapped: *mut OVERLAPPED,
) -> BOOL;

/// It is obviously too easy if all functions are exposed in Windows header files, digging for gold
/// is far more rewarding and fun, so let's dig for the function among the bits and bytes here.
fn extract_connectex_fn(s: SOCKET) -> oxidizer_io::Result<ConnectExFn> {
    let payload = WSAID_CONNECTEX;
    let mut result: *mut c_void = ptr::null_mut();
    let mut bytes_returned: u32 = 0;

    // SAFETY: No safety requirements - this is a synchronous call, so we expect it to not take
    // any long-lived hold on the data, so have no lifetime considerations to worry about.
    let status_code = unsafe {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "Tiny constants fit in u32 just fine"
        )]
        WSAIoctl(
            s,
            SIO_GET_EXTENSION_FUNCTION_POINTER,
            Some(ptr::from_ref(&payload).cast()),
            mem::size_of_val(&payload) as u32,
            Some((&raw mut result).cast()),
            mem::size_of_val(&result) as u32,
            &raw mut bytes_returned,
            // Technically, this supports OVERLAPPED but since all we are doing is "get a function
            // pointer" there is no conceivable async activity happening here, so we do not use
            // the asynchronous path to keep things maximally simple.
            None,
            None,
        )
    };

    winsock::status_code_to_result(status_code)?;

    #[expect(
        clippy::cast_possible_truncation,
        reason = "Tiny constants fit in u32 just fine"
    )]
    {
        assert_eq!(bytes_returned, mem::size_of::<*mut c_void>() as u32);
    }

    // SAFETY: Windows Sockets API promises this is the result it gives us.
    let result_fn = unsafe { mem::transmute::<*mut c_void, ConnectExFn>(result) };

    Ok(result_fn)
}