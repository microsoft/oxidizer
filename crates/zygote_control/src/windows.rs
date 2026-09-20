// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Windows native process creation and creation-time sandbox mechanisms.
//!
//! Import [`CommandExt`] to attach Job Object limits, a restricted token, an
//! existing `AppContainer` identity, or process mitigations to a
//! [`Command`]. The backend creates the process suspended,
//! applies all requested creation-time attributes, and resumes it only after
//! setup succeeds. Unsupported native controls and invalid policy
//! combinations fail the launch rather than being ignored.
//!
//! ```no_run
//! use zygote_control::Zygote;
//! use zygote_control::windows::{CommandExt as _, JobPolicy};
//!
//! fn configure() -> std::io::Result<()> {
//!     let zygote = Zygote::builder(r"C:\path\to\target.exe").spawn()?;
//!     let job = JobPolicy::builder()
//!         .active_process_limit(Some(1))
//!         .kill_on_close(true)
//!         .build()?;
//!     zygote.command().job(job)?;
//!     Ok(())
//! }
//! ```

#![expect(
    clippy::cast_possible_truncation,
    clippy::items_after_statements,
    clippy::multiple_unsafe_ops_per_block,
    clippy::too_many_lines,
    clippy::unreadable_literal,
    reason = "thin Win32 FFI backend validates bounded inputs before native conversions"
)]

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::ffi::{OsStr, c_void};
use std::fs::{File, OpenOptions};
use std::mem::{ManuallyDrop, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::process::ExitStatus;
use std::ptr::{null, null_mut};
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};
use std::{fmt, io, iter, thread};

#[cfg(test)]
use windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER;
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE, INVALID_HANDLE_VALUE, LUID, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
#[cfg(not(miri))]
use windows_sys::Win32::Globalization::CompareStringOrdinal;
use windows_sys::Win32::Security::{
    CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, DuplicateTokenEx, GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation,
    IsValidSid, LUID_AND_ATTRIBUTES, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES, SetTokenInformation,
    TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenIntegrityLevel, TokenPrimary,
};
#[cfg(test)]
use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_READ, FILE_GENERIC_WRITE};
use windows_sys::Win32::System::Console::GetStdHandle;
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_LIMIT_WORKINGSET,
    JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectCpuRateControlInformation,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::SystemServices::{SE_GROUP_ENABLED, SE_GROUP_INTEGRITY, SE_GROUP_USE_FOR_DENY_ONLY};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess, GetProcessId, InitializeProcThreadAttributeList, OpenProcessToken,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST, PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};
#[cfg(test)]
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

use crate::PrivilegeIntent;
use crate::child::{Child, ChildInner, ChildStderr, ChildStdin, ChildStdout};
use crate::command::Command;
use crate::stdio::{Stdio, StdioInner};

const INFINITE: u32 = u32::MAX;
const RESUME_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
const CLEANUP_INTAKE_BATCH: usize = 64;

static RESUME_CLEANUP_SERVICE: OnceLock<ResumeCleanupService> = OnceLock::new();
static RESUME_CLEANUP_SERVICE_INIT: Mutex<()> = Mutex::new(());
#[cfg(test)]
static CLEANUP_RELEASE_SENDER: Mutex<Option<mpsc::Sender<()>>> = Mutex::new(None);

#[cfg(test)]
#[derive(Clone, Copy, Default)]
struct BackendFaults {
    resume_error: Option<i32>,
    terminate_error: Option<i32>,
    cleanup_timeout: Option<Duration>,
}

#[cfg(test)]
thread_local! {
    static BACKEND_FAULTS: std::cell::Cell<BackendFaults> = const { std::cell::Cell::new(BackendFaults {
        resume_error: None,
        terminate_error: None,
        cleanup_timeout: None,
    }) };
    static LAST_CREATED_PROCESS_ID: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
struct BackendFaultGuard(BackendFaults);

#[cfg(test)]
impl BackendFaultGuard {
    fn install(faults: BackendFaults) -> Self {
        let previous = BACKEND_FAULTS.replace(faults);
        Self(previous)
    }
}

#[cfg(test)]
impl Drop for BackendFaultGuard {
    fn drop(&mut self) {
        BACKEND_FAULTS.set(self.0);
    }
}

#[cfg(test)]
struct CleanupReleaseGuard;

#[cfg(test)]
impl CleanupReleaseGuard {
    fn install(sender: mpsc::Sender<()>) -> Self {
        *CLEANUP_RELEASE_SENDER.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sender);
        Self
    }
}

#[cfg(test)]
impl Drop for CleanupReleaseGuard {
    fn drop(&mut self) {
        *CLEANUP_RELEASE_SENDER.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

#[cfg(test)]
struct CleanupReleaseProbe(Option<mpsc::Sender<()>>);

#[cfg(test)]
impl CleanupReleaseProbe {
    fn capture() -> Self {
        Self(
            CLEANUP_RELEASE_SENDER
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
    }
}

#[cfg(test)]
impl Drop for CleanupReleaseProbe {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

/// A validated binary Windows SID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sid(Box<[u8]>);

impl Sid {
    /// Copies and validates a self-contained SID.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] when the byte slice is not a
    /// complete structurally valid SID.
    pub fn from_bytes(bytes: &[u8]) -> io::Result<Self> {
        let expected_len = bytes
            .get(1)
            .copied()
            .filter(|count| *count <= 15)
            .map(|count| 8 + usize::from(count) * 4);
        // SAFETY: the length check proves bytes contains one complete SID, and
        // IsValidSid only reads that live allocation.
        if expected_len != Some(bytes.len()) || unsafe { IsValidSid(bytes.as_ptr().cast_mut().cast()) } == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid Windows SID"));
        }
        Ok(Self(bytes.into()))
    }

    fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr().cast_mut().cast()
    }
}

/// A Windows privilege locally unique identifier to remove.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Privilege {
    /// Low 32 bits.
    pub low_part: u32,
    /// Signed high 32 bits.
    pub high_part: i32,
}

/// Mandatory integrity level for a restricted token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegrityLevel {
    /// Untrusted integrity.
    Untrusted,
    /// Low integrity.
    Low,
    /// Medium integrity.
    Medium,
}

/// Reduction-only restricted-token configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestrictedTokenPolicy {
    disable_sids: Vec<Sid>,
    remove_privileges: Vec<Privilege>,
    restricting_sids: Vec<Sid>,
    integrity: Option<IntegrityLevel>,
}

/// Builder for a validated [`RestrictedTokenPolicy`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RestrictedTokenPolicyBuilder {
    disable_sids: Vec<Sid>,
    remove_privileges: Vec<Privilege>,
    restricting_sids: Vec<Sid>,
    integrity: Option<IntegrityLevel>,
}

impl RestrictedTokenPolicy {
    /// Starts building a restricted-token policy.
    #[must_use]
    pub const fn builder() -> RestrictedTokenPolicyBuilder {
        RestrictedTokenPolicyBuilder::new()
    }

    fn reduces_privileges(&self) -> bool {
        !self.disable_sids.is_empty() || !self.remove_privileges.is_empty() || !self.restricting_sids.is_empty() || self.integrity.is_some()
    }
}

impl RestrictedTokenPolicyBuilder {
    /// Creates an empty policy builder.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            disable_sids: Vec::new(),
            remove_privileges: Vec::new(),
            restricting_sids: Vec::new(),
            integrity: None,
        }
    }

    /// Replaces the SIDs to mark deny-only.
    #[must_use]
    pub fn disable_sids(mut self, sids: Vec<Sid>) -> Self {
        self.disable_sids = sids;
        self
    }

    /// Replaces the privileges to remove.
    #[must_use]
    pub fn remove_privileges(mut self, privileges: Vec<Privilege>) -> Self {
        self.remove_privileges = privileges;
        self
    }

    /// Replaces the restricting SID set.
    #[must_use]
    pub fn restricting_sids(mut self, sids: Vec<Sid>) -> Self {
        self.restricting_sids = sids;
        self
    }

    /// Sets the lower mandatory integrity level.
    ///
    /// Launch rejects a level that is not strictly below the controller
    /// process's current token integrity.
    #[must_use]
    pub const fn integrity(mut self, integrity: Option<IntegrityLevel>) -> Self {
        self.integrity = integrity;
        self
    }

    /// Validates and creates the policy.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] when no reduction is requested
    /// or any SID/privilege list exceeds 64 entries.
    pub fn build(self) -> io::Result<RestrictedTokenPolicy> {
        let policy = RestrictedTokenPolicy {
            disable_sids: self.disable_sids,
            remove_privileges: self.remove_privileges,
            restricting_sids: self.restricting_sids,
            integrity: self.integrity,
        };
        validate_restricted_token(&policy)?;
        Ok(policy)
    }
}

/// Existing `AppContainer` identity and explicit capabilities.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppContainerPolicy {
    /// `AppContainer` SID. Profile creation and ACL provisioning are external.
    pub appcontainer_sid: Sid,
    /// Explicit capability SIDs; no capability is added implicitly.
    pub capabilities: Vec<Sid>,
}

/// Hard Windows Job Object controls.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct JobPolicy {
    process_commit_limit: Option<usize>,
    job_commit_limit: Option<usize>,
    working_set: Option<(usize, usize)>,
    active_process_limit: Option<u32>,
    cpu_rate: Option<u32>,
    kill_on_close: bool,
}

/// Builder for a validated [`JobPolicy`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct JobPolicyBuilder {
    policy: JobPolicy,
}

impl JobPolicy {
    /// Starts building Job Object limits.
    #[must_use]
    pub const fn builder() -> JobPolicyBuilder {
        JobPolicyBuilder::new()
    }

    /// Checks whether Windows accepts these exact Job Object limits.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid limit combinations or when Windows
    /// rejects creation or configuration of the Job Object.
    pub fn probe(self) -> io::Result<()> {
        validate_job(self)?;
        drop(create_job(self)?);
        Ok(())
    }
}

impl JobPolicyBuilder {
    /// Creates an empty Job Object policy builder.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            policy: JobPolicy {
                process_commit_limit: None,
                job_commit_limit: None,
                working_set: None,
                active_process_limit: None,
                cpu_rate: None,
                kill_on_close: false,
            },
        }
    }

    /// Sets the per-process committed-memory limit.
    #[must_use]
    pub const fn process_commit_limit(mut self, limit: Option<usize>) -> Self {
        self.policy.process_commit_limit = limit;
        self
    }

    /// Sets the aggregate job committed-memory limit.
    #[must_use]
    pub const fn job_commit_limit(mut self, limit: Option<usize>) -> Self {
        self.policy.job_commit_limit = limit;
        self
    }

    /// Sets minimum and maximum working-set sizes.
    #[must_use]
    pub const fn working_set(mut self, limits: Option<(usize, usize)>) -> Self {
        self.policy.working_set = limits;
        self
    }

    /// Sets the maximum active process count.
    #[must_use]
    pub const fn active_process_limit(mut self, limit: Option<u32>) -> Self {
        self.policy.active_process_limit = limit;
        self
    }

    /// Sets the hard CPU rate in hundredths of a percent.
    #[must_use]
    pub const fn cpu_rate(mut self, rate: Option<u32>) -> Self {
        self.policy.cpu_rate = rate;
        self
    }

    /// Configures kill-on-close containment.
    #[must_use]
    pub const fn kill_on_close(mut self, enabled: bool) -> Self {
        self.policy.kill_on_close = enabled;
        self
    }

    /// Validates and creates the policy.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] for zero limits, an inverted
    /// working-set range, or a CPU rate outside 1 through 10,000.
    pub fn build(self) -> io::Result<JobPolicy> {
        validate_job(self.policy)?;
        Ok(self.policy)
    }
}

/// Creation-time process mitigation bits.
///
/// Values are the documented `PROCESS_CREATION_MITIGATION_POLICY_*` flags.
/// Unknown bits are rejected rather than masked.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MitigationPolicy {
    policy: [u64; 2],
}

impl MitigationPolicy {
    /// Checks whether Windows accepts this exact mitigation attribute.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error when the process-attribute list
    /// cannot be allocated or Windows rejects these mitigation bits.
    pub fn probe(self) -> io::Result<()> {
        let mut words = self.words();
        let mut attributes = AttributeList::new(1)?;
        attributes.update(
            PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY as usize,
            words.as_mut_ptr().cast(),
            size_of::<[u64; 2]>(),
        )
    }

    /// Enables DEP from the first instruction.
    #[must_use]
    pub const fn dep(mut self) -> Self {
        self.policy[0] |= 0x1;
        self
    }

    /// Requires relocation of images that do not opt into ASLR.
    #[must_use]
    pub const fn force_aslr(mut self) -> Self {
        self.policy[0] |= 0x100;
        self
    }

    /// Enables bottom-up and high-entropy ASLR.
    #[must_use]
    pub const fn high_entropy_aslr(mut self) -> Self {
        self.policy[0] |= 0x10000 | 0x100000;
        self
    }

    /// Terminates the process on invalid handle use.
    #[must_use]
    pub const fn strict_handle_checks(mut self) -> Self {
        self.policy[0] |= 0x1000000;
        self
    }

    /// Disables Win32k system calls.
    #[must_use]
    pub const fn disable_win32k(mut self) -> Self {
        self.policy[0] |= 0x10000000;
        self
    }

    /// Prohibits dynamic code generation and modification.
    #[must_use]
    pub const fn prohibit_dynamic_code(mut self) -> Self {
        self.policy[0] |= 0x1000000000;
        self
    }

    /// Requires Control Flow Guard.
    #[must_use]
    pub const fn control_flow_guard(mut self) -> Self {
        self.policy[0] |= 0x10000000000;
        self
    }

    /// Blocks remote and low-integrity image mappings.
    #[must_use]
    pub const fn restrict_image_loading(mut self) -> Self {
        self.policy[0] |= 0x10000000000000 | 0x100000000000000;
        self
    }

    /// Prohibits creation of child processes.
    #[must_use]
    pub const fn prohibit_child_processes(mut self) -> Self {
        self.policy[1] |= 0x1;
        self
    }

    fn words(self) -> [u64; 2] {
        self.policy
    }
}

/// Windows-specific sandbox options.
#[derive(Debug, Default)]
pub(super) struct SandboxOptions {
    job: Option<JobPolicy>,
    token: Option<RestrictedTokenPolicy>,
    appcontainer: Option<AppContainerPolicy>,
    mitigations: Option<MitigationPolicy>,
}

/// Windows-only sandbox extensions for [`Command`].
///
/// This trait is sealed and cannot be implemented outside this crate.
///
/// ```compile_fail
/// struct ForeignCommand;
/// impl zygote_control::windows::CommandExt for ForeignCommand {}
/// ```
pub trait CommandExt: crate::sealed::Sealed {
    /// Applies a Job Object before the primary thread can run.
    ///
    /// # Errors
    ///
    /// Returns an error when the policy is invalid or unsupported.
    fn job(&mut self, policy: JobPolicy) -> io::Result<&mut Self>;
    /// Launches with a reduction-only restricted primary token.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] when the policy is empty or a
    /// list exceeds 64 entries.
    fn restricted_token(&mut self, policy: RestrictedTokenPolicy) -> io::Result<&mut Self>;
    /// Launches in an existing `AppContainer` with explicit capabilities.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] when more than 64 capability
    /// SIDs are supplied.
    fn appcontainer(&mut self, policy: AppContainerPolicy) -> io::Result<&mut Self>;
    /// Applies immutable process mitigations during process creation.
    fn mitigations(&mut self, policy: MitigationPolicy) -> &mut Self;
}

/// Availability of the extended Windows process-creation backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Support {
    /// Extended startup attribute lists are available.
    pub startup_attributes: bool,
}

impl Support {
    /// Probes the creation-time attribute API without launching a process.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when an attribute list cannot be
    /// initialized.
    pub fn probe() -> io::Result<Self> {
        let attributes = AttributeList::new(1)?;
        drop(attributes);
        Ok(Self { startup_attributes: true })
    }
}

impl CommandExt for Command {
    fn job(&mut self, policy: JobPolicy) -> io::Result<&mut Self> {
        validate_job(policy)?;
        self.windows_sandbox.job = Some(policy);
        Ok(self)
    }

    fn restricted_token(&mut self, policy: RestrictedTokenPolicy) -> io::Result<&mut Self> {
        validate_restricted_token(&policy)?;
        self.windows_sandbox.token = Some(policy);
        Ok(self)
    }

    fn appcontainer(&mut self, policy: AppContainerPolicy) -> io::Result<&mut Self> {
        if policy.capabilities.len() > 64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "AppContainer capability list exceeds 64 entries",
            ));
        }
        self.windows_sandbox.appcontainer = Some(policy);
        Ok(self)
    }

    fn mitigations(&mut self, policy: MitigationPolicy) -> &mut Self {
        self.windows_sandbox.mitigations = Some(policy);
        self
    }
}

fn validate_restricted_token(policy: &RestrictedTokenPolicy) -> io::Result<()> {
    if !policy.reduces_privileges() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "restricted-token policy must remove authority",
        ));
    }
    if policy.disable_sids.len() > 64 || policy.remove_privileges.len() > 64 || policy.restricting_sids.len() > 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "restricted-token list exceeds 64 entries",
        ));
    }
    Ok(())
}

fn validate_job(policy: JobPolicy) -> io::Result<()> {
    if policy.process_commit_limit == Some(0)
        || policy.job_commit_limit == Some(0)
        || policy.active_process_limit == Some(0)
        || policy.cpu_rate.is_some_and(|rate| !(1..=10_000).contains(&rate))
        || policy
            .working_set
            .is_some_and(|(minimum, maximum)| minimum == 0 || minimum > maximum)
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid Job Object limits"));
    }
    Ok(())
}

pub(super) struct WindowsChild {
    process: OwnedHandle,
    _job: Option<OwnedHandle>,
    status: Option<ExitStatus>,
}

impl WindowsChild {
    pub(super) fn id(&self) -> u32 {
        // SAFETY: the handle is owned by this child and remains live for the call.
        unsafe { GetProcessId(self.process.as_raw_handle().cast()) }
    }

    pub(super) fn kill(&mut self) -> io::Result<()> {
        // SAFETY: the owned process handle includes termination access.
        cvt(unsafe { TerminateProcess(self.process.as_raw_handle().cast(), 1) })
    }

    pub(super) fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        // SAFETY: the process handle remains live and INFINITE is a valid timeout.
        if unsafe { WaitForSingleObject(self.process.as_raw_handle().cast(), INFINITE) } != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        self.read_terminal_status()
    }

    pub(super) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_some() {
            return Ok(self.status);
        }
        // SAFETY: the process handle remains live and a zero timeout only observes state.
        match unsafe { WaitForSingleObject(self.process.as_raw_handle().cast(), 0) } {
            WAIT_OBJECT_0 => self.read_terminal_status().map(Some),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }

    fn read_terminal_status(&mut self) -> io::Result<ExitStatus> {
        let mut code = 0;
        // SAFETY: code is writable and the signaled process handle is live.
        cvt(unsafe { GetExitCodeProcess(self.process.as_raw_handle().cast(), &raw mut code) })?;
        let status = terminal_exit_status(code);
        self.status = Some(status);
        Ok(status)
    }
}

fn terminal_exit_status(code: u32) -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(code)
}

impl fmt::Debug for WindowsChild {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowsChild").field("id", &self.id()).finish_non_exhaustive()
    }
}

pub(super) fn spawn(command: &Command, stdin: Stdio, stdout: Stdio, stderr: Stdio) -> io::Result<Child> {
    validate_job(command.windows_sandbox.job.unwrap_or_default())?;
    let cleanup_service = resume_cleanup_service()?;

    let prepared = PreparedStdio::new(stdin, stdout, stderr)?;
    let job = command.windows_sandbox.job.map(create_job).transpose()?;
    let force_portable_reduction = command.sandbox.privilege_intent() == PrivilegeIntent::Reduce;
    let token_policy = command.windows_sandbox.token.as_ref();
    let portable_policy = force_portable_reduction.then_some(RestrictedTokenPolicy {
        disable_sids: Vec::new(),
        remove_privileges: Vec::new(),
        restricting_sids: Vec::new(),
        integrity: None,
    });
    let token_policy = token_policy.or(portable_policy.as_ref());
    let token = token_policy
        .map(|policy| create_restricted_token(policy, force_portable_reduction))
        .transpose()?;
    let mut appcontainer = command.windows_sandbox.appcontainer.as_ref().map(SecurityCapabilities::new);

    let standard_handles = prepared.child_handles();
    let mut inherited_handles = prepared.inherited_handles();
    let inherit_handles = !inherited_handles.is_empty();
    let mut attributes = AttributeList::new(
        usize::from(!inherited_handles.is_empty())
            + usize::from(job.is_some())
            + usize::from(command.windows_sandbox.mitigations.is_some())
            + usize::from(appcontainer.is_some()),
    )?;
    if !inherited_handles.is_empty() {
        attributes.update(
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            inherited_handles.as_mut_ptr().cast(),
            inherited_handles.len() * size_of::<HANDLE>(),
        )?;
    }
    let mut job_handles = job.as_ref().map(|job| [job.as_raw_handle().cast::<c_void>()]);
    if let Some(handles) = &mut job_handles {
        attributes.update(
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            handles.as_mut_ptr().cast(),
            size_of_val(handles),
        )?;
    }
    let mut mitigation = command.windows_sandbox.mitigations.map(MitigationPolicy::words);
    if let Some(words) = &mut mitigation {
        attributes.update(
            PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY as usize,
            words.as_mut_ptr().cast(),
            size_of::<[u64; 2]>(),
        )?;
    }
    if let Some(capabilities) = &mut appcontainer {
        attributes.update(
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
            (&raw mut capabilities.raw).cast(),
            size_of::<SECURITY_CAPABILITIES>(),
        )?;
    }

    // SAFETY: STARTUPINFOEXW permits zero initialization before its size and
    // selected fields are populated below.
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = standard_handles[0];
    startup.StartupInfo.hStdOutput = standard_handles[1];
    startup.StartupInfo.hStdError = standard_handles[2];
    startup.lpAttributeList = attributes.pointer;
    // SAFETY: PROCESS_INFORMATION is an output-only Win32 structure.
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    let mut application = wide_nul(command.get_program())?;
    let mut command_line = command_line(command)?;
    let environment = environment_block(command)?;
    let directory = command.get_current_dir().map(wide_nul).transpose()?;
    let flags = CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED;
    // SAFETY: every pointer refers to mutable, NUL-terminated storage or a
    // live attribute/stdio object retained through process creation.
    // inherited_handles, job_handles, mitigation, and appcontainer remain allocated
    // until CreateProcess* consumes the attribute list. Handle inheritance is
    // disabled when the list is empty and otherwise restricted by the list.
    let created = unsafe {
        match &token {
            Some(token) => CreateProcessAsUserW(
                token.as_raw_handle().cast(),
                application.as_mut_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                inherit_handles.into(),
                flags,
                environment.as_ptr().cast(),
                directory.as_ref().map_or(null(), Vec::as_ptr),
                &raw const startup.StartupInfo,
                &raw mut process,
            ),
            None => CreateProcessW(
                application.as_mut_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                inherit_handles.into(),
                flags,
                environment.as_ptr().cast(),
                directory.as_ref().map_or(null(), Vec::as_ptr),
                &raw const startup.StartupInfo,
                &raw mut process,
            ),
        }
    };
    cvt(created)?;
    #[cfg(test)]
    LAST_CREATED_PROCESS_ID.set(process.dwProcessId);
    // SAFETY: successful process creation transfers one owned reference for
    // each non-null handle in PROCESS_INFORMATION.
    let process_handle = unsafe { OwnedHandle::from_raw_handle(process.hProcess.cast()) };
    // SAFETY: as above, hThread is a distinct owned handle.
    let thread_handle = unsafe { OwnedHandle::from_raw_handle(process.hThread.cast()) };
    if let Err(resume_error) = resume_primary_thread(&thread_handle) {
        return Err(match cleanup_failed_resume(&process_handle, resume_error) {
            ResumeCleanupOutcome::Complete(error) => error,
            ResumeCleanupOutcome::NeedsReaper(error) => {
                let request = ResumeCleanupRequest {
                    process: process_handle,
                    _thread: thread_handle,
                    #[cfg(test)]
                    _release_probe: CleanupReleaseProbe::capture(),
                };
                match cleanup_service.transfer(request) {
                    Ok(()) => io::Error::new(
                        error.kind(),
                        format!("{error}; cleanup ownership transferred to the durable cleanup service"),
                    ),
                    Err(request) => preserve_cleanup_ownership_after_disconnect(request),
                }
            }
        });
    }
    drop(thread_handle);
    let (parent_stdin, parent_stdout, parent_stderr) = prepared.into_parent();
    Ok(Child {
        inner: ChildInner::Windows(WindowsChild {
            process: process_handle,
            _job: job,
            status: None,
        }),
        stdin: parent_stdin.map(|file| ChildStdin { inner: Box::new(file) }),
        stdout: parent_stdout.map(|file| ChildStdout { inner: Box::new(file) }),
        stderr: parent_stderr.map(|file| ChildStderr { inner: Box::new(file) }),
        output_limits: command.output_limits,
        active_health: None,
    })
}

fn resume_primary_thread(thread: &OwnedHandle) -> io::Result<()> {
    #[cfg(test)]
    if let Some(error) = BACKEND_FAULTS.get().resume_error {
        return Err(io::Error::from_raw_os_error(error));
    }
    // SAFETY: thread owns the suspended primary thread.
    if unsafe { ResumeThread(thread.as_raw_handle().cast()) } == u32::MAX {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

enum ResumeCleanupOutcome {
    Complete(io::Error),
    NeedsReaper(io::Error),
}

fn cleanup_failed_resume(process: &OwnedHandle, resume_error: io::Error) -> ResumeCleanupOutcome {
    let timeout = resume_cleanup_timeout();
    let deadline = Instant::now() + timeout;
    cleanup_failed_resume_with(
        resume_error,
        || terminate_process(process),
        |timeout| wait_for_process(process, timeout),
        || {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                0
            } else {
                u32::try_from(remaining.as_millis().saturating_add(1)).unwrap_or(u32::MAX)
            }
        },
    )
}

fn resume_cleanup_timeout() -> Duration {
    #[cfg(test)]
    if let Some(timeout) = BACKEND_FAULTS.get().cleanup_timeout {
        return timeout;
    }
    RESUME_CLEANUP_TIMEOUT
}

fn terminate_process(process: &OwnedHandle) -> io::Result<()> {
    #[cfg(test)]
    if let Some(error) = BACKEND_FAULTS.get().terminate_error {
        return Err(io::Error::from_raw_os_error(error));
    }
    // SAFETY: process is live and its handle carries PROCESS_TERMINATE access.
    cvt(unsafe { TerminateProcess(process.as_raw_handle().cast(), 1) })
}

fn wait_for_process(process: &OwnedHandle, timeout: u32) -> io::Result<bool> {
    // SAFETY: process remains live for the wait and timeout is a valid value.
    match unsafe { WaitForSingleObject(process.as_raw_handle().cast(), timeout) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(io::Error::last_os_error()),
    }
}

fn cleanup_failed_resume_with(
    resume_error: io::Error,
    mut terminate: impl FnMut() -> io::Result<()>,
    mut wait: impl FnMut(u32) -> io::Result<bool>,
    mut remaining_millis: impl FnMut() -> u32,
) -> ResumeCleanupOutcome {
    let mut first_cleanup_error = None;
    let mut termination_requested = false;
    loop {
        let remaining = remaining_millis();
        if remaining == 0 {
            return ResumeCleanupOutcome::NeedsReaper(resume_cleanup_timeout_error(&resume_error, first_cleanup_error));
        }
        if !termination_requested {
            match terminate() {
                Ok(()) => termination_requested = true,
                Err(cleanup_error) => {
                    if first_cleanup_error.is_none() {
                        first_cleanup_error = Some(cleanup_error);
                    }
                }
            }
        }
        let remaining = remaining_millis();
        if remaining == 0 {
            return ResumeCleanupOutcome::NeedsReaper(resume_cleanup_timeout_error(&resume_error, first_cleanup_error));
        }
        let wait_timeout = if termination_requested { remaining } else { remaining.min(10) };
        match wait(wait_timeout) {
            Ok(true) => {
                return ResumeCleanupOutcome::Complete(match first_cleanup_error {
                    Some(cleanup_error) => io::Error::other(format!(
                        "ResumeThread failed: {resume_error}; an earlier TerminateProcess attempt failed: {cleanup_error}"
                    )),
                    None => resume_error,
                });
            }
            Ok(false) => {}
            Err(wait_error) => {
                if first_cleanup_error.is_none() {
                    first_cleanup_error = Some(wait_error);
                }
            }
        }
    }
}

struct ResumeCleanupRequest {
    process: OwnedHandle,
    _thread: OwnedHandle,
    // Fields drop in declaration order, so this notification follows both handles.
    #[cfg(test)]
    _release_probe: CleanupReleaseProbe,
}

struct ResumeCleanupService {
    sender: mpsc::Sender<ResumeCleanupRequest>,
}

impl ResumeCleanupService {
    fn start() -> io::Result<Self> {
        let sender = initialize_cleanup_sender(|receiver| {
            thread::Builder::new()
                .name("zygote-resume-cleanup".to_owned())
                .spawn(move || resume_cleanup_worker(&receiver))
                .map(drop)
                .map_err(|error| io::Error::new(error.kind(), format!("failed to initialize resume cleanup service: {error}")))
        })?;
        Ok(Self { sender })
    }

    fn transfer(&self, request: ResumeCleanupRequest) -> Result<(), ResumeCleanupRequest> {
        send_owned(&self.sender, request)
    }
}

fn initialize_cleanup_sender<T>(start: impl FnOnce(mpsc::Receiver<T>) -> io::Result<()>) -> io::Result<mpsc::Sender<T>> {
    let (sender, receiver) = mpsc::channel();
    start(receiver)?;
    Ok(sender)
}

fn send_owned<T>(sender: &mpsc::Sender<T>, value: T) -> Result<(), T> {
    sender.send(value).map_err(|mpsc::SendError(value)| value)
}

fn get_or_initialize<'a, T, E>(
    cell: &'a OnceLock<T>,
    initialization_lock: &Mutex<()>,
    initialize: impl FnOnce() -> Result<T, E>,
) -> Result<&'a T, E> {
    if let Some(value) = cell.get() {
        return Ok(value);
    }
    let _guard = match initialization_lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(value) = cell.get() {
        return Ok(value);
    }
    let value = initialize()?;
    Ok(cell.get_or_init(|| value))
}

fn resume_cleanup_service() -> io::Result<&'static ResumeCleanupService> {
    get_or_initialize(&RESUME_CLEANUP_SERVICE, &RESUME_CLEANUP_SERVICE_INIT, ResumeCleanupService::start)
}

fn resume_cleanup_worker(receiver: &mpsc::Receiver<ResumeCleanupRequest>) {
    let mut pending = VecDeque::new();
    loop {
        if pending.is_empty() {
            match receiver.recv() {
                Ok(request) => pending.push_back(request),
                Err(_disconnected) => loop {
                    thread::park();
                },
            }
        }
        for _ in 0..CLEANUP_INTAKE_BATCH {
            match receiver.try_recv() {
                Ok(request) => pending.push_back(request),
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => break,
            }
        }
        service_pending_round(&mut pending, service_cleanup_request);
        if !pending.is_empty() {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

fn service_pending_round<T>(pending: &mut VecDeque<T>, mut service: impl FnMut(&mut T) -> bool) {
    let count = pending.len();
    for _ in 0..count {
        let Some(mut request) = pending.pop_front() else {
            break;
        };
        if !service(&mut request) {
            pending.push_back(request);
        }
    }
}

fn service_cleanup_request(request: &mut ResumeCleanupRequest) -> bool {
    let _ = terminate_process(&request.process);
    wait_for_process(&request.process, 0).unwrap_or(false)
}

fn preserve_cleanup_ownership_after_disconnect(request: ResumeCleanupRequest) -> ! {
    // RESUME_CLEANUP_SERVICE owns a process-lifetime sender, and its worker
    // never exits for an individual cleanup error, so disconnection indicates
    // a violated process-wide invariant. Keep the request on this stack and
    // continue cleanup forever rather than returning and dropping handles for
    // a possibly live suspended process.
    let mut request = ManuallyDrop::new(request);
    loop {
        if service_cleanup_request(&mut request) {
            loop {
                thread::park();
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn resume_cleanup_timeout_error(resume_error: &io::Error, cleanup_error: Option<io::Error>) -> io::Error {
    match cleanup_error {
        Some(cleanup_error) => io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "ResumeThread failed: {resume_error}; cleanup deadline expired after {RESUME_CLEANUP_TIMEOUT:?}; cleanup also failed: {cleanup_error}"
            ),
        ),
        None => io::Error::new(
            io::ErrorKind::TimedOut,
            format!("ResumeThread failed: {resume_error}; cleanup deadline expired after {RESUME_CLEANUP_TIMEOUT:?}"),
        ),
    }
}

fn create_job(policy: JobPolicy) -> io::Result<OwnedHandle> {
    // SAFETY: null security/name pointers request an unnamed job with defaults.
    let handle = unsafe { CreateJobObjectW(null(), null()) };
    let job = owned(handle)?;
    // SAFETY: the job limit structure is valid when zero initialized.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    if policy.kill_on_close {
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    }
    if let Some(limit) = policy.process_commit_limit {
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        limits.ProcessMemoryLimit = limit;
    }
    if let Some(limit) = policy.job_commit_limit {
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
        limits.JobMemoryLimit = limit;
    }
    if let Some((minimum, maximum)) = policy.working_set {
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_WORKINGSET;
        limits.BasicLimitInformation.MinimumWorkingSetSize = minimum;
        limits.BasicLimitInformation.MaximumWorkingSetSize = maximum;
    }
    if let Some(limit) = policy.active_process_limit {
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.BasicLimitInformation.ActiveProcessLimit = limit;
    }
    // SAFETY: job is live and limits points to the exact structure size.
    cvt(unsafe {
        SetInformationJobObject(
            job.as_raw_handle().cast(),
            JobObjectExtendedLimitInformation,
            (&raw mut limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    })?;
    if let Some(rate) = policy.cpu_rate {
        // SAFETY: the CPU-rate structure is valid when zero initialized.
        let mut cpu: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = unsafe { zeroed() };
        cpu.ControlFlags = JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
        cpu.Anonymous.CpuRate = rate;
        // SAFETY: job is live and cpu points to the exact structure size.
        cvt(unsafe {
            SetInformationJobObject(
                job.as_raw_handle().cast(),
                JobObjectCpuRateControlInformation,
                (&raw mut cpu).cast(),
                size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
            )
        })?;
    }
    Ok(job)
}

fn sid_attributes(sids: &[Sid], attributes: u32) -> Vec<SID_AND_ATTRIBUTES> {
    sids.iter()
        .map(|sid| SID_AND_ATTRIBUTES {
            Sid: sid.as_ptr(),
            Attributes: attributes,
        })
        .collect()
}

fn disabled_sid_attributes(sids: &[Sid]) -> Vec<SID_AND_ATTRIBUTES> {
    sid_attributes(sids, SE_GROUP_USE_FOR_DENY_ONLY.cast_unsigned())
}

fn restricting_sid_attributes(sids: &[Sid]) -> Vec<SID_AND_ATTRIBUTES> {
    sid_attributes(sids, 0)
}

fn create_restricted_token(policy: &RestrictedTokenPolicy, force_portable_reduction: bool) -> io::Result<OwnedHandle> {
    let mut process_token = null_mut();
    // SAFETY: process_token is a writable output pointer and the requested
    // rights are the rights consumed by restriction, duplication, and label setup.
    cvt(unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
            &raw mut process_token,
        )
    })?;
    let process_token = owned(process_token)?;
    let integrity = portable_integrity(policy.integrity, force_portable_reduction)
        .map(|level| effective_integrity_level(token_integrity_rid(&process_token)?, level, force_portable_reduction))
        .transpose()?
        .flatten();
    let mut disabled = disabled_sid_attributes(&policy.disable_sids);
    let mut restricted = restricting_sid_attributes(&policy.restricting_sids);
    let mut privileges: Vec<LUID_AND_ATTRIBUTES> = policy
        .remove_privileges
        .iter()
        .map(|privilege| LUID_AND_ATTRIBUTES {
            Luid: LUID {
                LowPart: privilege.low_part,
                HighPart: privilege.high_part,
            },
            Attributes: 0,
        })
        .collect();
    let mut restricted_token = null_mut();
    // SAFETY: all slice-backed arrays remain alive for the call and each count
    // exactly matches its initialized array.
    cvt(unsafe {
        CreateRestrictedToken(
            process_token.as_raw_handle().cast(),
            restricted_token_flags(force_portable_reduction),
            disabled.len() as u32,
            disabled.as_mut_ptr(),
            privileges.len() as u32,
            privileges.as_mut_ptr(),
            restricted.len() as u32,
            restricted.as_mut_ptr(),
            &raw mut restricted_token,
        )
    })?;
    let restricted_token = owned(restricted_token)?;
    let mut primary = null_mut();
    // SAFETY: restricted_token is live, primary is writable, and the requested
    // rights are exactly those consumed by SetTokenInformation and
    // CreateProcessAsUserW.
    cvt(unsafe {
        DuplicateTokenEx(
            restricted_token.as_raw_handle().cast(),
            restricted_token_desired_access(),
            null(),
            2,
            TokenPrimary,
            &raw mut primary,
        )
    })?;
    let primary = owned(primary)?;
    if let Some(level) = integrity {
        set_integrity(&primary, level)?;
    }

    Ok(primary)
}

const fn portable_integrity(requested: Option<IntegrityLevel>, portable: bool) -> Option<IntegrityLevel> {
    if !portable {
        return requested;
    }
    match requested {
        Some(IntegrityLevel::Untrusted) => Some(IntegrityLevel::Untrusted),
        Some(IntegrityLevel::Low | IntegrityLevel::Medium) | None => Some(IntegrityLevel::Low),
    }
}

const fn restricted_token_flags(portable: bool) -> u32 {
    if portable { DISABLE_MAX_PRIVILEGE } else { 0 }
}

const fn restricted_token_desired_access() -> u32 {
    TOKEN_ASSIGN_PRIMARY | TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ADJUST_DEFAULT
}

fn effective_integrity_level(current: u32, requested: IntegrityLevel, portable: bool) -> io::Result<Option<IntegrityLevel>> {
    if integrity_rid(requested) < current {
        return Ok(Some(requested));
    }
    if portable {
        return Ok(None);
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "restricted-token integrity level must be lower than the controller token",
    ))
}

fn token_integrity_rid(token: &OwnedHandle) -> io::Result<u32> {
    let mut required = 0;
    // SAFETY: the null-buffer probe writes only the required byte count.
    let result = unsafe { GetTokenInformation(token.as_raw_handle().cast(), TokenIntegrityLevel, null_mut(), 0, &raw mut required) };
    if result != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "token integrity query unexpectedly accepted a null buffer",
        ));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER.cast_signed()) {
        return Err(error);
    }
    let mut buffer = vec![0u8; required as usize];
    // SAFETY: buffer has the size requested by the successful probe and remains
    // live while the returned SID is inspected.
    cvt(unsafe {
        GetTokenInformation(
            token.as_raw_handle().cast(),
            TokenIntegrityLevel,
            buffer.as_mut_ptr().cast(),
            required,
            &raw mut required,
        )
    })?;
    if buffer.len() < size_of::<TOKEN_MANDATORY_LABEL>() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "token integrity label is truncated"));
    }
    // SAFETY: GetTokenInformation initialized a TOKEN_MANDATORY_LABEL at the
    // start of buffer, which may not have Rust alignment.
    let label = unsafe { buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>().read_unaligned() };
    if label.Label.Sid.is_null() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "token integrity SID is missing"));
    }
    // SAFETY: the SID pointer was returned in the live token-information buffer.
    let count = unsafe { GetSidSubAuthorityCount(label.Label.Sid) };
    if count.is_null() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "token integrity SID is invalid"));
    }
    // SAFETY: count points into the valid SID returned by Windows.
    let count = unsafe { *count };
    if count == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "token integrity SID has no authority"));
    }
    // SAFETY: the last sub-authority exists because count is nonzero.
    let rid = unsafe { GetSidSubAuthority(label.Label.Sid, u32::from(count - 1)) };
    if rid.is_null() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "token integrity SID is truncated"));
    }
    // SAFETY: rid points into the live SID and identifies its integrity RID.
    Ok(unsafe { *rid })
}

const fn integrity_rid(level: IntegrityLevel) -> u32 {
    match level {
        IntegrityLevel::Untrusted => 0,
        IntegrityLevel::Low => 0x1000,
        IntegrityLevel::Medium => 0x2000,
    }
}

fn set_integrity(token: &OwnedHandle, level: IntegrityLevel) -> io::Result<()> {
    let rid = integrity_rid(level);
    #[repr(C)]
    struct IntegrityBuffer {
        label: TOKEN_MANDATORY_LABEL,
        sid: [u8; 12],
    }
    // SAFETY: IntegrityBuffer consists of zero-valid Win32 fields and bytes.
    let mut buffer: IntegrityBuffer = unsafe { zeroed() };
    buffer.sid[0] = 1;
    buffer.sid[1] = 1;
    buffer.sid[7] = 16;
    buffer.sid[8..12].copy_from_slice(&rid.to_ne_bytes());
    buffer.label.Label = SID_AND_ATTRIBUTES {
        Sid: buffer.sid.as_mut_ptr().cast(),
        Attributes: SE_GROUP_INTEGRITY as u32,
    };
    // SAFETY: token is live and buffer contains a complete SID whose storage
    // remains valid through SetTokenInformation.
    cvt(unsafe {
        SetTokenInformation(
            token.as_raw_handle().cast(),
            TokenIntegrityLevel,
            (&raw mut buffer).cast(),
            size_of::<IntegrityBuffer>() as u32,
        )
    })
}

struct SecurityCapabilities {
    raw: SECURITY_CAPABILITIES,
    _capabilities: Vec<SID_AND_ATTRIBUTES>,
}

impl SecurityCapabilities {
    fn new(policy: &AppContainerPolicy) -> Self {
        let mut capabilities = sid_attributes(&policy.capabilities, SE_GROUP_ENABLED.cast_unsigned());
        let raw = SECURITY_CAPABILITIES {
            AppContainerSid: policy.appcontainer_sid.as_ptr(),
            Capabilities: capabilities.as_mut_ptr(),
            CapabilityCount: capabilities.len() as u32,
            Reserved: 0,
        };
        Self {
            raw,
            _capabilities: capabilities,
        }
    }
}

struct AttributeList {
    storage: Vec<u8>,
    pointer: *mut c_void,
}

impl AttributeList {
    fn new(count: usize) -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: the documented sizing call accepts a null list and writes
        // only the required byte count.
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), count as u32, 0, &raw mut bytes);
        }
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0u8; bytes];
        let pointer: *mut c_void = storage.as_mut_ptr().cast();
        // SAFETY: storage has the size returned by the sizing call and remains
        // pinned by Vec's allocation until this AttributeList is dropped.
        cvt(unsafe { InitializeProcThreadAttributeList(pointer.cast(), count as u32, 0, &raw mut bytes) })?;
        Ok(Self { storage, pointer })
    }

    fn update(&mut self, attribute: usize, value: *mut c_void, size: usize) -> io::Result<()> {
        // SAFETY: self.pointer names an initialized list; callers retain each
        // value allocation through CreateProcess and pass its exact size.
        cvt(unsafe { UpdateProcThreadAttribute(self.pointer.cast(), 0, attribute, value, size, null_mut(), null_mut()) })
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        let _ = self.storage.len();
        // SAFETY: pointer was initialized exactly once and is deleted before
        // its backing storage is released.
        unsafe { DeleteProcThreadAttributeList(self.pointer.cast()) };
    }
}

struct PreparedStdio {
    child: [Option<OwnedHandle>; 3],
    parent_stdin: Option<File>,
    parent_stdout: Option<File>,
    parent_stderr: Option<File>,
}

impl PreparedStdio {
    fn new(stdin: Stdio, stdout: Stdio, stderr: Stdio) -> io::Result<Self> {
        let (stdin_child, parent_stdin) = prepare_stream(stdin, 0)?;
        let (stdout_child, parent_stdout) = prepare_stream(stdout, 1)?;
        let (stderr_child, parent_stderr) = prepare_stream(stderr, 2)?;
        Ok(Self {
            child: [stdin_child, stdout_child, stderr_child],
            parent_stdin,
            parent_stdout,
            parent_stderr,
        })
    }

    fn child_handles(&self) -> [HANDLE; 3] {
        self.child
            .each_ref()
            .map(|handle| handle.as_ref().map_or(null_mut(), |handle| handle.as_raw_handle().cast()))
    }

    fn inherited_handles(&self) -> Vec<HANDLE> {
        self.child
            .iter()
            .filter_map(|handle| handle.as_ref().map(|handle| handle.as_raw_handle().cast()))
            .collect()
    }

    fn into_parent(self) -> (Option<File>, Option<File>, Option<File>) {
        (self.parent_stdin, self.parent_stdout, self.parent_stderr)
    }
}

fn prepare_stream(configuration: Stdio, stream: usize) -> io::Result<(Option<OwnedHandle>, Option<File>)> {
    let input = stream == 0;
    match configuration.inner {
        StdioInner::Piped => {
            let mut read = null_mut();
            let mut write = null_mut();
            let attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: null_mut(),
                bInheritHandle: 1,
            };
            // SAFETY: both outputs are writable and attributes is initialized.
            cvt(unsafe { CreatePipe(&raw mut read, &raw mut write, &raw const attributes, 0) })?;
            let read = owned(read)?;
            let write = owned(write)?;
            if input {
                // SAFETY: into_raw_handle transfers the sole owned handle.
                Ok((Some(read), Some(unsafe { File::from_raw_handle(write.into_raw_handle()) })))
            } else {
                // SAFETY: into_raw_handle transfers the sole owned handle.
                Ok((Some(write), Some(unsafe { File::from_raw_handle(read.into_raw_handle()) })))
            }
        }
        StdioInner::Null => {
            let file = if input {
                OpenOptions::new().read(true).open("NUL")?
            } else {
                OpenOptions::new().write(true).open("NUL")?
            };
            duplicate_inheritable(file.as_raw_handle())
        }
        StdioInner::Inherit => {
            let identifier = [(-10i32).cast_unsigned(), (-11i32).cast_unsigned(), (-12i32).cast_unsigned()][stream];
            // SAFETY: identifier is one of the three documented standard handles.
            let handle = unsafe { GetStdHandle(identifier) };
            if handle.is_null() {
                Ok((None, None))
            } else if handle == INVALID_HANDLE_VALUE {
                Err(io::Error::last_os_error())
            } else {
                duplicate_inheritable(handle.cast())
            }
        }
        StdioInner::File(file) => duplicate_inheritable(file.as_raw_handle()),
    }
}

fn duplicate_inheritable(handle: RawHandle) -> io::Result<(Option<OwnedHandle>, Option<File>)> {
    let mut duplicate = null_mut();
    // SAFETY: duplicate is writable; source and target processes are current
    // and handle is borrowed for the duration of the call.
    cvt(unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle.cast(),
            GetCurrentProcess(),
            &raw mut duplicate,
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    })?;
    Ok((Some(owned(duplicate)?), None))
}

fn command_line(command: &Command) -> io::Result<Vec<u16>> {
    let capacity = command_line_len(command)?;
    let mut output = Vec::with_capacity(capacity);
    for argument in iter::once(command.get_program()).chain(command.get_args()) {
        if !output.is_empty() {
            output.push(u16::from(b' '));
        }
        quote_argument(argument, &mut output)?;
    }
    output.push(0);
    Ok(output)
}

fn command_line_len(command: &Command) -> io::Result<usize> {
    let (capacity, _) =
        iter::once(command.get_program())
            .chain(command.get_args())
            .try_fold((1usize, true), |(length, first), argument| {
                let argument_length = quoted_argument_len(argument)?;
                let length = length
                    .checked_add(argument_length)
                    .and_then(|length| length.checked_add(usize::from(!first)))
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Windows command line is too long"))?;
                Ok::<_, io::Error>((length, false))
            })?;
    Ok(capacity)
}

fn quote_argument(argument: &OsStr, output: &mut Vec<u16>) -> io::Result<()> {
    output.reserve(quoted_argument_len(argument)?);
    output.push(u16::from(b'"'));
    let mut slashes = 0;
    for unit in argument.encode_wide() {
        if unit == u16::from(b'\\') {
            slashes += 1;
        } else {
            if unit == u16::from(b'"') {
                output.extend(std::iter::repeat_n(u16::from(b'\\'), slashes + 1));
            }
            output.extend(std::iter::repeat_n(u16::from(b'\\'), slashes));
            slashes = 0;
            output.push(unit);
        }
    }
    output.extend(std::iter::repeat_n(u16::from(b'\\'), slashes * 2));
    output.push(u16::from(b'"'));
    Ok(())
}

fn quoted_argument_len(argument: &OsStr) -> io::Result<usize> {
    let mut length = 2usize;
    let mut slashes = 0usize;
    for unit in argument.encode_wide() {
        if unit == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Windows argument contains NUL"));
        }
        if unit == u16::from(b'\\') {
            slashes = slashes
                .checked_add(1)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Windows argument is too long"))?;
            continue;
        }
        let escaped_prefix = if unit == u16::from(b'"') {
            slashes
                .checked_mul(2)
                .and_then(|length| length.checked_add(2))
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Windows argument is too long"))?
        } else {
            slashes
                .checked_add(1)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Windows argument is too long"))?
        };
        length = length
            .checked_add(escaped_prefix)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Windows argument is too long"))?;
        slashes = 0;
    }
    length
        .checked_add(
            slashes
                .checked_mul(2)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Windows argument is too long"))?,
        )
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Windows argument is too long"))
}

struct EncodedEnvironmentEntry {
    key: Vec<u16>,
    value: Vec<u16>,
}

impl EncodedEnvironmentEntry {
    fn new(key: &OsStr, value: &OsStr) -> Self {
        Self {
            key: key.encode_wide().collect(),
            value: value.encode_wide().collect(),
        }
    }

    fn serialized_len(&self) -> io::Result<usize> {
        validate_environment_entry(&self.key, &self.value)?;
        self.key
            .len()
            .checked_add(self.value.len())
            .and_then(|length| length.checked_add(2))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Windows environment block is too long"))
    }
}

fn environment_block(command: &Command) -> io::Result<Vec<u16>> {
    let inherited: Vec<EncodedEnvironmentEntry> = if command.clear_environment {
        Vec::new()
    } else {
        std::env::vars_os()
            .map(|(key, value)| EncodedEnvironmentEntry::new(&key, &value))
            .collect()
    };
    let environment = merge_environment(inherited, command);
    let capacity = environment.iter().try_fold(1usize, |length, entry| {
        length
            .checked_add(entry.serialized_len()?)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Windows environment block is too long"))
    })?;
    let mut block = Vec::with_capacity(capacity.max(2));
    for entry in environment {
        block.extend(entry.key);
        block.push(u16::from(b'='));
        block.extend(entry.value);
        block.push(0);
    }
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    Ok(block)
}

fn merge_environment(mut inherited: Vec<EncodedEnvironmentEntry>, command: &Command) -> Vec<EncodedEnvironmentEntry> {
    if command.environment.len() <= 1 {
        for (key, value) in &command.environment {
            let key: Vec<u16> = key.encode_wide().collect();
            inherited.retain(|entry| !environment_name_units_equal(&entry.key, &key));
            if let Some(value) = value {
                inherited.push(EncodedEnvironmentEntry {
                    key,
                    value: value.encode_wide().collect(),
                });
            }
        }
        inherited.sort_by(|left, right| compare_environment_name_units(&left.key, &right.key));
        return inherited;
    }

    // OsString's BTreeMap order is not Windows ordinal case-insensitive order;
    // sorting encoded keys once avoids both a custom map key and repeated scans.
    let mut overrides: Vec<_> = command
        .environment
        .iter()
        .map(|(key, value)| {
            (
                key.encode_wide().collect::<Vec<_>>(),
                value.as_ref().map(|value| value.encode_wide().collect::<Vec<_>>()),
            )
        })
        .collect();
    inherited.sort_by(|left, right| compare_environment_name_units(&left.key, &right.key));
    overrides.sort_by(|left, right| compare_environment_name_units(&left.0, &right.0));

    let mut inherited = inherited.into_iter().peekable();
    let mut overrides = overrides.into_iter().peekable();
    let mut merged = Vec::new();
    while let (Some(entry), Some((key, _))) = (inherited.peek(), overrides.peek()) {
        // The sort comparator breaks case-insensitive ties by spelling.
        if environment_name_units_equal(&entry.key, key) {
            inherited.next();
            continue;
        }
        match compare_environment_name_units(&entry.key, key) {
            Ordering::Less | Ordering::Equal => merged.push(
                inherited
                    .next()
                    .expect("peek returned an inherited entry and the iterator has not advanced"),
            ),
            Ordering::Greater => {
                let (key, value) = overrides
                    .next()
                    .expect("peek returned an override and the iterator has not advanced");
                if let Some(value) = value {
                    merged.push(EncodedEnvironmentEntry { key, value });
                }
            }
        }
    }
    merged.extend(inherited);
    merged.extend(overrides.filter_map(|(key, value)| value.map(|value| EncodedEnvironmentEntry { key, value })));
    merged
}

fn validate_environment_entry(key: &[u16], value: &[u16]) -> io::Result<()> {
    let invalid_equals = key.iter().enumerate().any(|(index, unit)| *unit == u16::from(b'=') && index != 0);
    if key.is_empty() || key.contains(&0) || invalid_equals || value.contains(&0) {
        Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid Windows environment entry"))
    } else {
        Ok(())
    }
}

#[cfg(not(miri))]
fn compare_environment_name_units(left: &[u16], right: &[u16]) -> Ordering {
    // SAFETY: both pointers and explicit lengths describe initialized UTF-16
    // slices; TRUE requests Windows ordinal case-insensitive comparison.
    match unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            left.len().try_into().unwrap_or(i32::MAX),
            right.as_ptr(),
            right.len().try_into().unwrap_or(i32::MAX),
            1,
        )
    } {
        1 => Ordering::Less,
        3 => Ordering::Greater,
        _ => left.cmp(right),
    }
}

#[cfg(miri)]
fn ascii_uppercase(unit: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&unit) {
        unit - u16::from(b'a') + u16::from(b'A')
    } else {
        unit
    }
}

#[cfg(miri)]
fn compare_environment_name_units(left: &[u16], right: &[u16]) -> Ordering {
    left.iter()
        .copied()
        .map(ascii_uppercase)
        .cmp(right.iter().copied().map(ascii_uppercase))
        .then_with(|| left.cmp(right))
}

#[cfg(not(miri))]
fn environment_name_units_equal(left: &[u16], right: &[u16]) -> bool {
    // SAFETY: both pointers and explicit lengths describe initialized UTF-16
    // slices; TRUE requests Windows ordinal case-insensitive comparison.
    unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            left.len().try_into().unwrap_or(i32::MAX),
            right.as_ptr(),
            right.len().try_into().unwrap_or(i32::MAX),
            1,
        ) == 2
    }
}

#[cfg(miri)]
fn environment_name_units_equal(left: &[u16], right: &[u16]) -> bool {
    left.iter()
        .copied()
        .map(ascii_uppercase)
        .eq(right.iter().copied().map(ascii_uppercase))
}

pub(super) fn environment_names_equal(left: &OsStr, right: &OsStr) -> bool {
    let left: Vec<u16> = left.encode_wide().collect();
    let right: Vec<u16> = right.encode_wide().collect();
    environment_name_units_equal(&left, &right)
}

fn wide_nul(value: impl AsRef<OsStr>) -> io::Result<Vec<u16>> {
    let value = value.as_ref();
    let length = value.encode_wide().try_fold(1usize, |length, unit| {
        if unit == 0 {
            Err(io::Error::new(io::ErrorKind::InvalidInput, "Windows path contains NUL"))
        } else {
            length
                .checked_add(1)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Windows path is too long"))
        }
    })?;
    let mut wide = Vec::with_capacity(length);
    for unit in value.encode_wide() {
        wide.push(unit);
    }
    wide.push(0);
    Ok(wide)
}

fn cvt(result: i32) -> io::Result<()> {
    if result == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: the caller passes a newly returned non-null Win32 handle and
        // transfers its sole ownership to the result.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle.cast()) })
    }
}

trait IntoRawHandle {
    fn into_raw_handle(self) -> RawHandle;
}

impl IntoRawHandle for OwnedHandle {
    fn into_raw_handle(self) -> RawHandle {
        let handle = self.as_raw_handle();
        std::mem::forget(self);
        handle
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn restricted_token_requests_only_required_access() {
        assert_eq!(
            restricted_token_desired_access(),
            TOKEN_ASSIGN_PRIMARY | TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ADJUST_DEFAULT
        );
    }

    #[test]
    fn resume_cleanup_success_preserves_original_error() {
        let ResumeCleanupOutcome::Complete(error) = cleanup_failed_resume_with(
            io::Error::from_raw_os_error(6),
            || Ok(()),
            |timeout| {
                assert_eq!(timeout, 100);
                Ok(true)
            },
            || 100,
        ) else {
            panic!("successful cleanup must not require a reaper");
        };
        assert_eq!(error.raw_os_error(), Some(6));
    }

    #[test]
    fn resume_cleanup_retries_and_surfaces_an_earlier_failure() {
        use std::cell::Cell;

        let original = io::Error::from_raw_os_error(6);
        let attempts = Cell::new(0);
        let ResumeCleanupOutcome::Complete(error) = cleanup_failed_resume_with(
            original,
            || {
                let attempt = attempts.get();
                attempts.set(attempt + 1);
                if attempt == 0 {
                    Err(io::Error::from_raw_os_error(5))
                } else {
                    Ok(())
                }
            },
            |timeout| {
                if attempts.get() == 1 {
                    assert_eq!(timeout, 10);
                    Ok(false)
                } else {
                    assert_eq!(timeout, 100);
                    Ok(true)
                }
            },
            || 100,
        ) else {
            panic!("successful retry must not require a reaper");
        };
        let message = error.to_string();
        assert!(message.contains("os error 6"));
        assert!(message.contains("os error 5"));
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn resume_cleanup_deadline_exhaustion_reports_both_failures() {
        let mut remaining = [20, 10, 0].into_iter();
        let ResumeCleanupOutcome::NeedsReaper(error) = cleanup_failed_resume_with(
            io::Error::from_raw_os_error(6),
            || Err(io::Error::from_raw_os_error(5)),
            |timeout| {
                assert_eq!(timeout, 10);
                Ok(false)
            },
            || remaining.next().unwrap_or(0),
        ) else {
            panic!("deadline exhaustion must transfer cleanup ownership");
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("os error 6"));
        assert!(error.to_string().contains("os error 5"));
        assert!(error.to_string().contains("cleanup deadline expired"));
    }

    #[test]
    fn cleanup_service_initialization_retries_after_failure() {
        let cell = OnceLock::new();
        let lock = Mutex::new(());
        let first = get_or_initialize(&cell, &lock, || Err::<usize, _>(io::Error::other("injected worker-start failure")));
        let error = first.expect_err("worker-start failure must prevent service creation");
        assert_eq!(error.to_string(), "injected worker-start failure");
        assert!(cell.get().is_none());

        let second = get_or_initialize(&cell, &lock, || Ok::<_, io::Error>(7)).unwrap();
        assert_eq!(*second, 7);
        assert_eq!(*get_or_initialize(&cell, &lock, || Ok::<_, io::Error>(9)).unwrap(), 7);
    }

    #[test]
    fn cleanup_service_round_robin_does_not_starve_later_requests() {
        struct Pending {
            id: u8,
            completes_after: Option<usize>,
        }

        let mut pending = VecDeque::from([
            Pending {
                id: 1,
                completes_after: None,
            },
            Pending {
                id: 2,
                completes_after: Some(0),
            },
            Pending {
                id: 3,
                completes_after: Some(1),
            },
        ]);
        let mut serviced = Vec::new();
        service_pending_round(&mut pending, |request| {
            serviced.push(request.id);
            match &mut request.completes_after {
                None => false,
                Some(remaining) if *remaining == 0 => true,
                Some(remaining) => {
                    *remaining -= 1;
                    false
                }
            }
        });
        assert_eq!(serviced, [1, 2, 3]);
        assert_eq!(pending.iter().map(|request| request.id).collect::<Vec<_>>(), [1, 3]);

        service_pending_round(&mut pending, |request| {
            serviced.push(request.id);
            match &mut request.completes_after {
                None => false,
                Some(remaining) if *remaining == 0 => true,
                Some(remaining) => {
                    *remaining -= 1;
                    false
                }
            }
        });
        assert_eq!(serviced, [1, 2, 3, 1, 3]);
        assert_eq!(pending.iter().map(|request| request.id).collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn cleanup_channel_transfers_or_returns_ownership_without_dropping_it() {
        use std::cell::Cell;
        use std::rc::Rc;

        struct DropProbe(Rc<Cell<usize>>);

        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let drops = Rc::new(Cell::new(0));
        let (sender, receiver) = mpsc::channel();
        assert!(send_owned(&sender, DropProbe(Rc::clone(&drops))).is_ok());
        assert_eq!(drops.get(), 0);
        drop(receiver.recv().unwrap());
        assert_eq!(drops.get(), 1);

        let (sender, receiver) = mpsc::channel();
        drop(receiver);
        let retained = send_owned(&sender, DropProbe(Rc::clone(&drops))).expect_err("disconnection must return ownership");
        assert_eq!(drops.get(), 1);
        drop(retained);
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn quotes_windows_arguments() {
        for (argument, expected) in [
            ("", r#""""#),
            ("plain", r#""plain""#),
            ("a b", r#""a b""#),
            (r#"a"b"#, r#""a\"b""#),
            (r#"a\"b"#, r#""a\\\"b""#),
            (r#"a\\"b"#, r#""a\\\\\"b""#),
            (r#"a\"b\\"c"#, r#""a\\\"b\\\\\"c""#),
            (r"trailing\\", r#""trailing\\\\""#),
        ] {
            let mut output = Vec::new();
            quote_argument(OsStr::new(argument), &mut output).unwrap();
            assert_eq!(String::from_utf16(&output).unwrap(), expected);
            assert_eq!(output.len(), quoted_argument_len(OsStr::new(argument)).unwrap());
        }
        for slash_count in 1..=16 {
            let argument = format!(r#"left{}"middle{}"right"#, "\\".repeat(slash_count), "\\".repeat(slash_count + 1));
            let mut output = Vec::new();
            quote_argument(OsStr::new(&argument), &mut output).unwrap();
            assert_eq!(output.len(), quoted_argument_len(OsStr::new(&argument)).unwrap());
        }
    }

    #[test]
    fn command_line_matches_its_precomputed_serialized_length() {
        let mut command = Command::new(crate::zygote::Launcher::for_test("fixture"));
        command.args(["", "a b", r#"a\"b"#, r"trailing\\"]);
        let line = command_line(&command).unwrap();
        assert_eq!(line.last(), Some(&0));
        assert_eq!(line.len(), command_line_len(&command).unwrap());
    }

    #[test]
    fn empty_environment_block_is_double_nul_terminated() {
        let mut command = Command::new(crate::zygote::Launcher::for_test("fixture"));
        command.env_clear();
        assert_eq!(environment_block(&command).unwrap(), [0, 0]);
    }

    #[test]
    fn environment_updates_are_case_insensitive() {
        let mut command = Command::new(crate::zygote::Launcher::for_test("fixture"));
        command
            .env_clear()
            .env("Path", "first")
            .env("PATH", "second")
            .env("Secret", "value")
            .env_remove("sEcReT");
        let block = String::from_utf16_lossy(&environment_block(&command).unwrap());
        assert!(block.contains("PATH=second\0"));
        assert!(!block.contains("Path=first"));
        assert!(!block.to_ascii_lowercase().contains("secret="));
    }

    #[test]
    fn environment_block_has_exact_length_and_preserves_boundary_entries() {
        let mut command = Command::new(crate::zygote::Launcher::for_test("fixture"));
        command.env_clear().env("=C:", r"C:\work").env("Alpha", "").env("omega", "last");
        let block = environment_block(&command).unwrap();
        assert_eq!(String::from_utf16_lossy(&block), "=C:=C:\\work\0Alpha=\0omega=last\0\0");
    }

    #[test]
    fn environment_merge_preserves_inherited_collisions_and_replaces_all_matching_keys() {
        let inherited = || {
            ["path", "PATH", "=C:", "=c:", "keep"]
                .into_iter()
                .enumerate()
                .map(|(index, key)| EncodedEnvironmentEntry::new(OsStr::new(key), OsStr::new(&index.to_string())))
                .collect::<Vec<_>>()
        };
        let mut command = Command::new(crate::zygote::Launcher::for_test("fixture"));
        command.env_clear().env("PaTh", "new").env_remove("=C:");
        let entries = merge_environment(inherited(), &command);
        let decoded = entries
            .iter()
            .map(|entry| (String::from_utf16(&entry.key).unwrap(), String::from_utf16(&entry.value).unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(decoded, [("keep".into(), "4".into()), ("PaTh".into(), "new".into())]);

        command.env_clear();
        let entries = merge_environment(inherited(), &command);
        let decoded = entries
            .iter()
            .map(|entry| String::from_utf16(&entry.key).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(decoded, ["=C:", "=c:", "keep", "PATH", "path"]);
    }

    #[test]
    #[cfg(not(miri))]
    fn environment_merge_uses_windows_ordinal_unicode_comparison() {
        let inherited = vec![EncodedEnvironmentEntry::new(OsStr::new("Ångström"), OsStr::new("old"))];
        let mut command = Command::new(crate::zygote::Launcher::for_test("fixture"));
        command.env_clear().env("ångström", "new").env("=C:", "drive").env_remove("=c:");
        let entries = merge_environment(inherited, &command);
        assert_eq!(entries.len(), 1);
        assert_eq!(String::from_utf16(&entries[0].key).unwrap(), "ångström");
        assert_eq!(String::from_utf16(&entries[0].value).unwrap(), "new");
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires process creation and Windows process APIs unavailable under Miri")]
    fn real_backend_transfers_failed_resume_cleanup_and_releases_handles() {
        resume_cleanup_service().unwrap();
        let (released, release_notification) = mpsc::channel();
        let _release_guard = CleanupReleaseGuard::install(released);
        let executable = std::env::current_exe().unwrap();
        let program = executable.to_str().unwrap();
        let command = Command::new(crate::zygote::Launcher::for_test(program));
        let _faults = BackendFaultGuard::install(BackendFaults {
            resume_error: Some(6),
            terminate_error: Some(5),
            cleanup_timeout: Some(Duration::from_millis(25)),
        });

        let error = spawn(&command, Stdio::null(), Stdio::null(), Stdio::null()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        let message = error.to_string();
        assert!(message.contains("ResumeThread failed"));
        assert!(message.contains("os error 6"));
        assert!(message.contains("os error 5"));
        assert!(message.contains("cleanup ownership transferred to the durable cleanup service"));

        let process_id = LAST_CREATED_PROCESS_ID.get();
        assert_ne!(process_id, 0);
        // SAFETY: the access mask requests only synchronization and query
        // rights, inheritance is disabled, and process_id came from CreateProcessW.
        let observed = unsafe { OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if observed.is_null() {
            assert_eq!(
                io::Error::last_os_error().raw_os_error(),
                Some(ERROR_INVALID_PARAMETER.cast_signed()),
                "failed to observe the launched child before or after cleanup"
            );
        } else {
            let observed = owned(observed).unwrap();
            assert!(
                wait_for_process(&observed, 5_000).unwrap(),
                "cleanup service did not terminate the child"
            );
        }
        release_notification
            .recv_timeout(Duration::from_secs(5))
            .expect("cleanup service did not release the process and primary-thread handles");
    }

    #[test]
    fn terminal_still_active_value_is_preserved() {
        assert_eq!(terminal_exit_status(259).code(), Some(259));
    }

    #[test]
    fn integrity_policy_requires_a_strict_reduction() {
        assert_eq!(
            effective_integrity_level(0x1000, IntegrityLevel::Low, false).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            effective_integrity_level(0x1000, IntegrityLevel::Medium, false).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            effective_integrity_level(0x2000, IntegrityLevel::Low, false).unwrap(),
            Some(IntegrityLevel::Low)
        );
        assert_eq!(effective_integrity_level(0x1000, IntegrityLevel::Low, true).unwrap(), None);
    }

    #[test]
    fn portable_reduction_cannot_be_weakened_by_native_token_settings() {
        let minimal = RestrictedTokenPolicy::builder()
            .remove_privileges(vec![Privilege { low_part: 1, high_part: 0 }])
            .build()
            .unwrap();
        assert_eq!(portable_integrity(minimal.integrity, true), Some(IntegrityLevel::Low));
        let medium = RestrictedTokenPolicy::builder()
            .integrity(Some(IntegrityLevel::Medium))
            .build()
            .unwrap();
        assert_eq!(portable_integrity(medium.integrity, true), Some(IntegrityLevel::Low));
        assert_eq!(
            portable_integrity(Some(IntegrityLevel::Untrusted), true),
            Some(IntegrityLevel::Untrusted)
        );
        assert_eq!(restricted_token_flags(true), DISABLE_MAX_PRIVILEGE);
    }

    #[test]
    fn restricted_token_sid_lists_use_required_attributes() {
        let sid = Sid(Box::from([1, 2, 0, 0, 0, 0, 0, 15, 2, 0, 0, 0, 1, 0, 0, 0]));

        assert_eq!(
            disabled_sid_attributes(std::slice::from_ref(&sid))[0].Attributes,
            SE_GROUP_USE_FOR_DENY_ONLY.cast_unsigned()
        );
        assert_eq!(restricting_sid_attributes(&[sid])[0].Attributes, 0);
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires native Windows sandbox and handle APIs unavailable under Miri")]
    fn sandbox_native_seams_accept_supported_controls() {
        JobPolicy::builder()
            .active_process_limit(Some(1))
            .kill_on_close(true)
            .build()
            .unwrap()
            .probe()
            .unwrap();
        MitigationPolicy::default().dep().probe().unwrap();

        let token = RestrictedTokenPolicy::builder()
            .integrity(Some(IntegrityLevel::Low))
            .build()
            .unwrap();
        drop(create_restricted_token(&token, true).unwrap());

        let appcontainer_sid = Sid::from_bytes(&[1, 2, 0, 0, 0, 0, 0, 15, 2, 0, 0, 0, 1, 0, 0, 0]).unwrap();
        let policy = AppContainerPolicy {
            appcontainer_sid,
            capabilities: Vec::new(),
        };
        let mut capabilities = SecurityCapabilities::new(&policy);
        let mut attributes = AttributeList::new(1).unwrap();
        attributes
            .update(
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                (&raw mut capabilities.raw).cast(),
                size_of::<SECURITY_CAPABILITIES>(),
            )
            .unwrap();

        let prepared = PreparedStdio::new(Stdio::null(), Stdio::null(), Stdio::null()).unwrap();
        let mut handles = prepared.child_handles();
        let mut attributes = AttributeList::new(1).unwrap();
        attributes
            .update(
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_mut_ptr().cast(),
                handles.len() * size_of::<HANDLE>(),
            )
            .unwrap();
    }
}
