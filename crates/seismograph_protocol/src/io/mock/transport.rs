// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;
use crate::monitor::MonitorDescriptor;

// Match the native pipe buffer limit so write deadlines exercise backpressure.
const CAPACITY: usize = 65_536;

#[derive(Debug)]
pub(super) struct PendingListener {
    owner: MonitorOwner,
    process_id: u32,
    nonblocking: bool,
    clients: VecDeque<Connection>,
}

#[derive(Debug)]
pub(crate) struct Listener {
    runtime: Runtime,
    path: PathBuf,
}

impl Listener {
    pub(crate) fn bind(runtime: &Runtime, descriptor: &MonitorDescriptor) -> io::Result<Self> {
        let mut state = runtime.state();
        let path = descriptor.endpoint.path();
        #[cfg(unix)]
        {
            let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
            Runtime::directory(&state, parent, Some(&descriptor.owner))?;
        }
        if state.listeners.contains_key(path) {
            return Err(io::ErrorKind::AddrInUse.into());
        }
        let owner = state.owner.clone();
        let process_id = state.process_id;
        state.listeners.insert(
            path.to_owned(),
            PendingListener {
                owner,
                process_id,
                nonblocking: false,
                clients: VecDeque::new(),
            },
        );
        Ok(Self {
            runtime: runtime.clone(),
            path: path.to_owned(),
        })
    }

    pub(crate) fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.runtime
            .state()
            .listeners
            .get_mut(&self.path)
            .ok_or(io::ErrorKind::NotConnected)?
            .nonblocking = nonblocking;
        Ok(())
    }

    pub(crate) fn accept(&self) -> io::Result<Connection> {
        let mut state = self.runtime.state();
        loop {
            let listener = state.listeners.get_mut(&self.path).ok_or(io::ErrorKind::NotConnected)?;
            if let Some(client) = listener.clients.pop_front() {
                return Ok(client);
            }
            if listener.nonblocking {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            state.operations.push(IoOperation::Wait);
            state = self
                .runtime
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        // Readers hold the channel lock while consulting the registry clock.
        // Release the registry lock before queued clients acquire channel locks.
        let pending = self.runtime.state().listeners.remove(&self.path);
        drop(pending);
        self.runtime.shared.changed.notify_all();
    }
}

#[derive(Debug)]
pub(super) struct Channel {
    pub(super) state: Mutex<ChannelState>,
    pub(super) changed: Condvar,
}

#[derive(Debug)]
pub(super) struct ChannelState {
    incoming: [VecDeque<u8>; 2],
    closed: [bool; 2],
    handles: [usize; 2],
}

#[derive(Debug)]
pub(crate) struct Connection {
    runtime: Weak<Shared>,
    channel: Arc<Channel>,
    side: usize,
    read_timeout: Mutex<Option<Duration>>,
    write_timeout: Mutex<Option<Duration>>,
}

impl Connection {
    pub(crate) fn pair(runtime: &Runtime) -> (Self, Self) {
        let channel = Arc::new(Channel {
            state: Mutex::new(ChannelState {
                incoming: [VecDeque::new(), VecDeque::new()],
                closed: [false, false],
                handles: [1, 1],
            }),
            changed: Condvar::new(),
        });
        runtime.state().channels.push(Arc::downgrade(&channel));
        let connection = |side| Self {
            runtime: Arc::downgrade(&runtime.shared),
            channel: Arc::clone(&channel),
            side,
            read_timeout: Mutex::new(None),
            write_timeout: Mutex::new(None),
        };
        (connection(0), connection(1))
    }

    pub(crate) fn connect(runtime: &Runtime, descriptor: &MonitorDescriptor) -> io::Result<Self> {
        {
            let state = runtime.state();
            let listener = state.listeners.get(descriptor.endpoint.path()).ok_or(io::ErrorKind::NotFound)?;
            if listener.owner != descriptor.owner || listener.process_id != descriptor.process_id {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "connected monitor has a different OS identity",
                ));
            }
            if state.owner != descriptor.owner && !Runtime::administrative(&state) {
                return Err(access_denied());
            }
        }
        let (client, server) = Self::pair(runtime);
        runtime
            .state()
            .listeners
            .get_mut(descriptor.endpoint.path())
            .ok_or(io::ErrorKind::ConnectionRefused)?
            .clients
            .push_back(server);
        runtime.shared.changed.notify_all();
        Ok(client)
    }

    fn runtime(&self) -> io::Result<Runtime> {
        self.runtime
            .upgrade()
            .map(|shared| Runtime { shared })
            .ok_or_else(|| io::ErrorKind::ConnectionAborted.into())
    }

    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        let mut state = self.channel.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.handles[self.side] = state.handles[self.side]
            .checked_add(1)
            .ok_or_else(|| io::Error::other("mock connection clone count exhausted"))?;
        Ok(Self {
            runtime: Weak::clone(&self.runtime),
            channel: Arc::clone(&self.channel),
            side: self.side,
            read_timeout: Mutex::new(*self.read_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner)),
            write_timeout: Mutex::new(*self.write_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner)),
        })
    }

    pub(crate) fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.check_timeout(timeout)?;
        *self.read_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = timeout;
        Ok(())
    }

    pub(crate) fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.check_timeout(timeout)?;
        *self.write_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = timeout;
        Ok(())
    }

    fn check_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        if timeout.is_some_and(|timeout| {
            timeout.is_zero() || self.runtime().is_ok_and(|runtime| runtime.instant().checked_add(timeout).is_none())
        }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "mock timeout must be positive and bounded",
            ));
        }
        Ok(())
    }

    fn wait<'a>(
        &self,
        state: MutexGuard<'a, ChannelState>,
        runtime: &Runtime,
        deadline: Option<Instant>,
    ) -> io::Result<MutexGuard<'a, ChannelState>> {
        if deadline.is_some_and(|deadline| runtime.instant() >= deadline) {
            return Err(io::ErrorKind::TimedOut.into());
        }
        runtime.record(IoOperation::Wait);
        Ok(self.channel.changed.wait(state).unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    fn read_bytes(&self, buf: &mut [u8], consume: bool) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let runtime = self.runtime()?;
        let max_chunk = runtime.state().max_chunk;
        let timeout = *self.read_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let deadline = timeout
            .map(|timeout| runtime.instant().checked_add(timeout).ok_or(io::ErrorKind::InvalidInput))
            .transpose()?;
        let mut state = self.channel.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if state.closed[self.side] {
                return Err(io::ErrorKind::ConnectionAborted.into());
            }
            if !state.incoming[self.side].is_empty() {
                let count = buf.len().min(state.incoming[self.side].len()).min(max_chunk);
                for (target, byte) in buf[..count].iter_mut().zip(state.incoming[self.side].iter()) {
                    *target = *byte;
                }
                if consume {
                    state.incoming[self.side].drain(..count);
                    self.channel.changed.notify_all();
                }
                return Ok(count);
            }
            if state.closed[1 - self.side] {
                return Ok(0);
            }
            state = self.wait(state, &runtime, deadline)?;
        }
    }

    pub(crate) fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_bytes(buf, false)
    }

    pub(crate) fn close(&self) {
        self.channel.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).closed[self.side] = true;
        self.channel.changed.notify_all();
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let mut state = self.channel.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.handles[self.side] -= 1;
        if state.handles[self.side] == 0 {
            state.closed[self.side] = true;
            self.channel.changed.notify_all();
        }
    }
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_bytes(buf, true)
    }
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let runtime = self.runtime()?;
        let max_chunk = runtime.state().max_chunk;
        let timeout = *self.write_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let deadline = timeout
            .map(|timeout| runtime.instant().checked_add(timeout).ok_or(io::ErrorKind::InvalidInput))
            .transpose()?;
        let mut state = self.channel.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if state.closed[self.side] {
                return Err(io::ErrorKind::ConnectionAborted.into());
            }
            if state.closed[1 - self.side] {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            let available = CAPACITY.saturating_sub(state.incoming[1 - self.side].len());
            if available != 0 {
                let count = buf.len().min(available).min(max_chunk);
                state.incoming[1 - self.side].extend(&buf[..count]);
                self.channel.changed.notify_all();
                return Ok(count);
            }
            state = self.wait(state, &runtime, deadline)?;
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        let state = self.channel.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed[self.side] {
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn blocking_accept_wakes_when_a_connection_arrives() {
        let runtime = Runtime::new();
        let owner = runtime.owner();
        let directory = runtime.current_directory(&owner).unwrap();
        let instance_id = runtime.instance_id();
        let descriptor = MonitorDescriptor {
            name: "worker".into(),
            instance: None,
            process_id: runtime.process_id(),
            instance_id,
            endpoint: crate::monitor::MonitorEndpoint::for_instance(&directory, &owner, runtime.process_id(), instance_id),
            owner,
        };
        let listener = Listener::bind(&runtime, &descriptor).unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || send.send(listener.accept()).unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !runtime.state().operations.contains(&IoOperation::Wait) {
            assert!(Instant::now() < deadline, "mock accept did not wait");
            std::thread::yield_now();
        }
        let mut client = Connection::connect(&runtime, &descriptor).unwrap();
        let mut server = receive.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        client.write_all(b"ready").unwrap();
        let mut bytes = [0; 5];
        server.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"ready");
        worker.join().unwrap();
    }

    #[test]
    fn closed_streams_reject_io_without_changing_empty_buffer_semantics() {
        let runtime = Runtime::new();
        let (mut client, mut server) = Connection::pair(&runtime);
        client.close();
        assert_eq!(client.read(&mut []).unwrap(), 0);
        assert_eq!(client.peek(&mut []).unwrap(), 0);
        assert_eq!(client.write(&[]).unwrap(), 0);
        assert_eq!(client.read(&mut [0]).unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(client.write(&[0]).unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(client.flush().unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(server.write(&[0]).unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(server.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn deadlines_must_be_positive_and_representable() {
        let runtime = Runtime::new();
        let (client, _server) = Connection::pair(&runtime);
        for timeout in [Duration::ZERO, Duration::MAX] {
            assert_eq!(
                client.set_read_timeout(Some(timeout)).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
            assert_eq!(
                client.set_write_timeout(Some(timeout)).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        client.set_read_timeout(None).unwrap();
        client.set_write_timeout(None).unwrap();
    }

    #[test]
    fn a_connection_does_not_keep_a_destroyed_mock_runtime_alive() {
        let runtime = Runtime::new();
        let (mut client, _server) = Connection::pair(&runtime);
        drop(runtime);
        assert_eq!(client.read(&mut [0]).unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(client.write(&[0]).unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
    }

    #[test]
    fn listener_drops_pending_clients_without_holding_the_registry_lock() {
        let runtime = Runtime::new();
        let owner = runtime.owner();
        let process_id = runtime.process_id();
        let instance_id = runtime.instance_id();
        let directory = runtime.current_directory(&owner).unwrap();
        let descriptor = MonitorDescriptor {
            name: "worker".into(),
            instance: None,
            endpoint: crate::monitor::MonitorEndpoint::for_instance(&directory, &owner, process_id, instance_id),
            owner,
            process_id,
            instance_id,
        };
        let listener = Listener::bind(&runtime, &descriptor).unwrap();
        let client = Connection::connect(&runtime, &descriptor).unwrap();
        let guard = client.channel.state.lock().unwrap();
        let worker = std::thread::spawn(move || drop(listener));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(state) = runtime.shared.state.try_lock()
                && !state.listeners.contains_key(descriptor.endpoint.path())
            {
                break;
            }
            assert!(Instant::now() < deadline, "pending client teardown retained the registry lock");
            std::thread::yield_now();
        }
        drop(guard);
        worker.join().unwrap();
    }
}
