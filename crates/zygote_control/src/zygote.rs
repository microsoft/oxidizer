// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::{OsStr, OsString};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard};
use std::time::Duration;
use std::{fmt, io};

use crate::Command;
#[cfg(target_os = "linux")]
use crate::linux::LinuxPool;

pub(super) const MAX_WORKERS: usize = 64;

pub(super) enum LauncherInner {
    #[cfg(test)]
    Test,
    #[cfg(not(target_os = "linux"))]
    Native,
    #[cfg(target_os = "linux")]
    Linux(LinuxPool),
}

/// A cloneable, thread-safe process launcher.
#[derive(Clone)]
pub struct Launcher {
    pub(super) program: Arc<OsString>,
    pub(super) inner: Arc<LauncherInner>,
    pub(super) closed: Arc<RwLock<bool>>,
    pub(super) health: Arc<HealthState>,
}

impl fmt::Debug for Launcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Launcher").field("program", &self.program).finish_non_exhaustive()
    }
}

impl Launcher {
    #[cfg(test)]
    pub(super) fn for_test(program: &str) -> Self {
        Self {
            program: Arc::new(OsString::from(program)),
            inner: Arc::new(LauncherInner::Test),
            closed: Arc::new(RwLock::new(false)),
            health: Arc::new(HealthState::default()),
        }
    }

    pub(super) fn begin_launch(&self) -> io::Result<RwLockReadGuard<'_, bool>> {
        let closed = self.closed.read().map_err(|error| io::Error::other(error.to_string()))?;
        if *closed {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "zygote is shut down"));
        }
        Ok(closed)
    }

    fn close(&self) -> io::Result<()> {
        *self.closed.write().map_err(|error| io::Error::other(error.to_string()))? = true;
        Ok(())
    }

    fn close_for_drop(&self) {
        match self.closed.write() {
            Ok(mut closed) => *closed = true,
            Err(poisoned) => *poisoned.into_inner() = true,
        }
    }

    /// Creates a command for one launch.
    #[must_use]
    pub fn command(&self) -> Command {
        Command::new(self.clone())
    }

    /// Returns a lock-free snapshot of launcher health and operational counters.
    #[must_use]
    pub fn health(&self) -> LauncherHealth {
        self.health.snapshot()
    }
}

/// Policy for dormant, single-use Linux workers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreforkPoolConfig {
    pub(super) min_idle: usize,
    pub(super) max_idle: usize,
    pub(super) refill_threshold: usize,
    pub(super) refill_delay: Duration,
}

impl PreforkPoolConfig {
    /// Creates a pool that maintains up to `max_idle` dormant workers.
    ///
    /// The initial minimum and refill threshold are both `max_idle`, so every
    /// consumed worker is replenished immediately. Use the setters to trade
    /// burst capacity for lower background fork activity.
    ///
    /// # Errors
    ///
    /// Returns an error if `max_idle` exceeds 64.
    pub fn new(max_idle: usize) -> io::Result<Self> {
        if max_idle > 64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "prefork maximum idle worker count cannot exceed 64",
            ));
        }
        Ok(Self {
            min_idle: max_idle,
            max_idle,
            refill_threshold: max_idle,
            refill_delay: Duration::ZERO,
        })
    }

    /// Sets the idle count that triggers immediate replenishment.
    ///
    /// # Errors
    ///
    /// Returns an error when the minimum exceeds the refill threshold.
    pub fn min_idle(&mut self, min_idle: usize) -> io::Result<&mut Self> {
        if min_idle > self.refill_threshold {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "prefork minimum idle count cannot exceed the refill threshold",
            ));
        }
        self.min_idle = min_idle;
        Ok(self)
    }

    /// Sets the idle count at or below which a delayed refill reaches the maximum.
    ///
    /// # Errors
    ///
    /// Returns an error when the threshold is outside `min_idle..=max_idle`.
    pub fn refill_threshold(&mut self, refill_threshold: usize) -> io::Result<&mut Self> {
        if !(self.min_idle..=self.max_idle).contains(&refill_threshold) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "prefork refill threshold must be between the minimum and maximum",
            ));
        }
        self.refill_threshold = refill_threshold;
        Ok(self)
    }

    /// Sets the delay before a threshold refill.
    pub fn refill_delay(&mut self, refill_delay: Duration) -> &mut Self {
        self.refill_delay = refill_delay;
        self
    }
}

impl Default for PreforkPoolConfig {
    fn default() -> Self {
        Self {
            min_idle: 0,
            max_idle: 0,
            refill_threshold: 0,
            refill_delay: Duration::ZERO,
        }
    }
}

/// Bounded Linux template-worker restart policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkerRecoveryPolicy {
    pub(super) max_restarts: usize,
    pub(super) initial_backoff: Duration,
    pub(super) maximum_backoff: Duration,
}

impl WorkerRecoveryPolicy {
    /// Creates a policy allowing at most `max_restarts` replacements per worker.
    #[must_use]
    pub const fn new(max_restarts: usize) -> Self {
        Self {
            max_restarts,
            initial_backoff: Duration::from_millis(10),
            maximum_backoff: Duration::from_secs(5),
        }
    }

    /// Sets exponential restart-backoff bounds.
    ///
    /// # Errors
    ///
    /// Returns an error if the initial delay exceeds the maximum.
    pub fn backoff(&mut self, initial: Duration, maximum: Duration) -> io::Result<&mut Self> {
        if initial > maximum {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "initial worker restart backoff cannot exceed the maximum",
            ));
        }
        self.initial_backoff = initial;
        self.maximum_backoff = maximum;
        Ok(self)
    }
}

impl Default for WorkerRecoveryPolicy {
    fn default() -> Self {
        Self::new(0)
    }
}

/// A bounded snapshot of launcher state and cumulative operational counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct LauncherHealth {
    /// Number of template workers currently ready to receive launches.
    pub ready_workers: usize,
    /// Number of application children known to be active.
    pub active_children: usize,
    /// Number of dormant prefork workers ready for assignment.
    pub idle_prefork_workers: usize,
    /// Successful launches reported by the backend.
    pub launches: u64,
    /// Launches rejected before application entry.
    pub launch_failures: u64,
    /// Launches dispatched to a dormant worker.
    pub prefork_hits: u64,
    /// Launches that synchronously forked because no dormant worker was available.
    pub prefork_misses: u64,
    /// Successfully bootstrapped replacement template workers.
    pub worker_restarts: u64,
    /// Failed template replacement attempts.
    pub worker_restart_failures: u64,
    /// Completed prefork replenishments.
    pub prefork_refills: u64,
    /// Sum of measured prefork replenishment time in nanoseconds.
    pub prefork_refill_nanoseconds: u64,
    /// Sum of measured request-to-entry latency in nanoseconds.
    pub request_to_entry_nanoseconds: u64,
    /// Highest measured request-to-entry latency in nanoseconds.
    pub maximum_request_to_entry_nanoseconds: u64,
    /// Whether shutdown has begun.
    pub shutting_down: bool,
}

#[derive(Default)]
pub(super) struct HealthState {
    pub(super) ready_workers: AtomicUsize,
    pub(super) active_children: AtomicUsize,
    pub(super) idle_prefork_workers: AtomicUsize,
    pub(super) launches: AtomicU64,
    pub(super) launch_failures: AtomicU64,
    pub(super) prefork_hits: AtomicU64,
    pub(super) prefork_misses: AtomicU64,
    pub(super) worker_restarts: AtomicU64,
    pub(super) worker_restart_failures: AtomicU64,
    pub(super) prefork_refills: AtomicU64,
    pub(super) prefork_refill_nanoseconds: AtomicU64,
    pub(super) request_to_entry_nanoseconds: AtomicU64,
    pub(super) maximum_request_to_entry_nanoseconds: AtomicU64,
    pub(super) shutting_down: AtomicBool,
}

impl HealthState {
    fn snapshot(&self) -> LauncherHealth {
        LauncherHealth {
            ready_workers: self.ready_workers.load(Ordering::Relaxed),
            active_children: self.active_children.load(Ordering::Relaxed),
            idle_prefork_workers: self.idle_prefork_workers.load(Ordering::Relaxed),
            launches: self.launches.load(Ordering::Relaxed),
            launch_failures: self.launch_failures.load(Ordering::Relaxed),
            prefork_hits: self.prefork_hits.load(Ordering::Relaxed),
            prefork_misses: self.prefork_misses.load(Ordering::Relaxed),
            worker_restarts: self.worker_restarts.load(Ordering::Relaxed),
            worker_restart_failures: self.worker_restart_failures.load(Ordering::Relaxed),
            prefork_refills: self.prefork_refills.load(Ordering::Relaxed),
            prefork_refill_nanoseconds: self.prefork_refill_nanoseconds.load(Ordering::Relaxed),
            request_to_entry_nanoseconds: self.request_to_entry_nanoseconds.load(Ordering::Relaxed),
            maximum_request_to_entry_nanoseconds: self.maximum_request_to_entry_nanoseconds.load(Ordering::Relaxed),
            shutting_down: self.shutting_down.load(Ordering::Relaxed),
        }
    }
}

/// Configuration for creating a [`Zygote`].
#[derive(Debug)]
pub struct ZygoteBuilder {
    program: OsString,
    workers: NonZeroUsize,
    prefork_pool: PreforkPoolConfig,
    worker_recovery: WorkerRecoveryPolicy,
}

impl ZygoteBuilder {
    /// Starts the configured backend.
    ///
    /// On Linux, the template executable does not inherit the controller
    /// environment. It must resolve its initial shared-library dependencies
    /// without loader-time environment variables such as `LD_LIBRARY_PATH` or
    /// `LD_PRELOAD`. Environment configured on [`Command`] is installed after
    /// the template has loaded, in each launched child.
    ///
    /// # Errors
    ///
    /// Returns an error if the Linux zygote cannot be started or its bootstrap
    /// handshake fails.
    #[cfg_attr(windows, expect(clippy::unnecessary_wraps, reason = "Linux backend startup can fail"))]
    pub fn spawn(&self) -> io::Result<Zygote> {
        #[cfg(target_os = "linux")]
        {
            crate::linux::start(&self.program, self.workers, self.prefork_pool, self.worker_recovery)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let launcher = Launcher {
                program: Arc::new(self.program.clone()),
                inner: Arc::new(LauncherInner::Native),
                closed: Arc::new(RwLock::new(false)),
                health: Arc::new(HealthState {
                    ready_workers: AtomicUsize::new(1),
                    ..HealthState::default()
                }),
            };
            Ok(Zygote { launcher })
        }
    }

    /// Sets the number of independent zygote workers used to distribute
    /// concurrent launches.
    ///
    /// A value greater than one starts that many prepared templates and routes
    /// launches round-robin. Native backends accept the setting but already
    /// create each process independently.
    ///
    /// # Errors
    ///
    /// Returns an error when `workers` is zero or greater than 64.
    pub fn workers(&mut self, workers: usize) -> io::Result<&mut Self> {
        self.workers = Self::validate_worker_count(workers)?;
        Ok(self)
    }

    /// Configures the optional Linux prefork pool.
    ///
    /// Other backends accept the setting but do not use it.
    pub fn prefork_pool(&mut self, policy: PreforkPoolConfig) -> &mut Self {
        self.prefork_pool = policy;
        self
    }

    pub(super) fn validate_worker_count(workers: usize) -> io::Result<NonZeroUsize> {
        if !(1..=MAX_WORKERS).contains(&workers) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "zygote worker count must be between 1 and 64",
            ));
        }
        NonZeroUsize::new(workers).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "zygote worker count must be nonzero"))
    }

    /// Enables bounded replacement of failed Linux template workers.
    ///
    /// Other backends accept the setting but do not use it.
    pub fn worker_recovery(&mut self, policy: WorkerRecoveryPolicy) -> &mut Self {
        self.worker_recovery = policy;
        self
    }
}

/// An owning handle to a reusable process launcher.
pub struct Zygote {
    pub(super) launcher: Launcher,
}

impl fmt::Debug for Zygote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Zygote")
            .field("program", &self.launcher.program)
            .finish_non_exhaustive()
    }
}

impl Zygote {
    #[cfg(target_os = "linux")]
    fn shutdown_backend(&self) -> io::Result<()> {
        match self.launcher.inner.as_ref() {
            LauncherInner::Linux(pool) => pool.shutdown(),
            #[cfg(test)]
            LauncherInner::Test => Ok(()),
        }
    }

    /// Creates a zygote builder for a target program.
    pub fn builder<S: AsRef<OsStr>>(program: S) -> ZygoteBuilder {
        ZygoteBuilder {
            program: program.as_ref().to_owned(),
            workers: NonZeroUsize::MIN,
            prefork_pool: PreforkPoolConfig::default(),
            worker_recovery: WorkerRecoveryPolicy::default(),
        }
    }

    /// Returns a cloneable launcher.
    #[must_use]
    pub fn launcher(&self) -> Launcher {
        self.launcher.clone()
    }

    /// Creates a command for one launch.
    #[must_use]
    pub fn command(&self) -> Command {
        self.launcher.command()
    }

    /// Returns a lock-free launcher health snapshot.
    #[must_use]
    pub fn health(&self) -> LauncherHealth {
        self.launcher.health()
    }

    /// Immediately changes the dormant-worker target on every Linux template.
    ///
    /// This can trim copy-on-write and kernel-resource overhead under memory
    /// pressure, or restore a previously configured target. The target cannot
    /// exceed the maximum supplied through [`ZygoteBuilder::prefork_pool`].
    /// Native backends have no dormant workers and accept only zero.
    ///
    /// # Errors
    ///
    /// Returns an error for an out-of-range target or if a Linux control
    /// packet cannot be delivered.
    #[cfg_attr(
        not(target_os = "linux"),
        expect(clippy::unused_self, reason = "the receiver preserves the cross-platform Zygote API")
    )]
    pub fn trim_prefork_pool(&self, idle_workers: usize) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        {
            match self.launcher.inner.as_ref() {
                LauncherInner::Linux(pool) => pool.trim_prefork_pool(idle_workers),
                #[cfg(test)]
                LauncherInner::Test => Ok(()),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            if idle_workers == 0 {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "prefork workers are available only on Linux",
                ))
            }
        }
    }

    /// Stops accepting launches and terminates the zygote.
    ///
    /// Running children remain alive. On accelerated Linux, a child whose exit
    /// was not reported before shutdown remains queryable and terminable
    /// through its pidfd, but its full exit status may no longer be available.
    ///
    /// # Errors
    ///
    /// Returns an error if the shutdown request cannot be delivered or the
    /// Linux zygote cannot be reaped.
    pub fn shutdown(self) -> io::Result<()> {
        self.launcher.close()?;
        self.launcher.health.shutting_down.store(true, Ordering::Relaxed);
        #[cfg(target_os = "linux")]
        {
            self.shutdown_backend()
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(())
        }
    }
}

impl Drop for Zygote {
    fn drop(&mut self) {
        self.launcher.close_for_drop();
        self.launcher.health.shutting_down.store(true, Ordering::Relaxed);
        #[cfg(target_os = "linux")]
        {
            let _ = self.shutdown_backend();
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn prefork_minimum_cannot_exceed_refill_threshold() {
        let mut policy = PreforkPoolConfig::new(10).unwrap();
        let error = policy.refill_threshold(5).and_then(|policy| policy.min_idle(10)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

        policy.min_idle(0).unwrap();
        policy.refill_threshold(5).unwrap();
        assert_eq!(policy.min_idle(10).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert_eq!(policy.min_idle, 0);
        assert_eq!(policy.refill_threshold, 5);
        assert_eq!(policy.max_idle, 10);
    }

    #[test]
    fn worker_count_is_bounded() {
        let mut builder = Zygote::builder("fixture");
        assert_eq!(builder.workers(0).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        builder.workers(1).unwrap();
        builder.workers(MAX_WORKERS).unwrap();
        assert_eq!(builder.workers(MAX_WORKERS + 1).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert_eq!(builder.workers.get(), MAX_WORKERS);
    }
}
