// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Linux accelerated-launch and sandbox configuration.
//!
//! Import [`CommandExt`] to attach Linux-only controls to a
//! [`Command`]. Caller-opened cgroup, Landlock, and namespace
//! descriptors are validated before transfer and remain capabilities selected
//! by the caller. The runtime applies every requested control before reporting
//! that the child started; unsupported or rejected controls fail the launch.
//!
//! ```
//! use zygote_control::Zygote;
//! use zygote_control::linux::{CommandExt as _, PrivilegePolicy};
//!
//! # fn configure() -> std::io::Result<()> {
//! let zygote = Zygote::builder("/path/to/integrated-target").spawn()?;
//! let policy = PrivilegePolicy::builder()
//!     .no_new_privileges(true)
//!     .disable_dumping(true)
//!     .build()?;
//! zygote.command().privilege_policy(policy);
//! # Ok(())
//! # }
//! ```

#![expect(
    clippy::borrow_as_ptr,
    clippy::multiple_unsafe_ops_per_block,
    reason = "socket ancillary-data operations mirror the libc C layout and are documented at each boundary"
)]
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "wire and syscall sizes are checked against fixed protocol limits before conversion"
)]

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::Path;
use std::process::{self, ExitStatus};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use std::{fmt, iter, mem};

use prost::Message as _;
use zygote_rt::protocol::packet::Body;
use zygote_rt::protocol::{
    CONTROL_FD, CONTROL_FD_ENV, DescriptorRole, Launch, LinuxSandbox, MAX_FD_COUNT, MAX_ITEM_COUNT, MAX_PACKET_LEN, NONCE_ENV,
    NamespaceKind as ProtoNamespaceKind, Packet, PoolControl, PrivilegeReduction, ProtocolError, Rlimit,
    SeccompInstruction as ProtoSeccompInstruction, SeccompPolicy as ProtoSeccompPolicy, Shutdown, SupplementaryGroups, UnixOptions, decode,
    packet,
};
#[cfg(test)]
use zygote_rt::protocol::{ErrorMessage, Exited, PoolState, Ready, Started};

use crate::PrivilegeIntent;
use crate::child::{Child, ChildInner, ChildStderr, ChildStdin, ChildStdout, ReadPipe};
use crate::command::Command;
use crate::stdio::{Stdio, StdioInner};
use crate::zygote::{HealthState, Launcher, LauncherInner, PreforkPoolConfig, WorkerRecoveryPolicy, Zygote};

/// Linux audit architecture accepted by a seccomp filter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditArchitecture {
    /// x86-64 Linux syscall ABI.
    X86_64,
    /// `AArch64` Linux syscall ABI.
    Aarch64,
}

/// Linux sandbox facilities available to the running process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Support {
    /// Highest Landlock ABI, or `None` when Landlock is unavailable.
    pub landlock_abi: Option<u32>,
    /// Whether the seccomp filter operation is recognized by the kernel.
    pub seccomp_filter: bool,
    /// Whether descriptor-based namespace entry is available.
    pub setns: bool,
    /// Whether cgroup v2 is mounted at the conventional unified hierarchy.
    pub cgroup_v2: bool,
}

impl Support {
    /// Probes kernel support without changing process state.
    #[must_use]
    pub fn probe() -> Self {
        let landlock_abi = LandlockRuleset::abi_version().ok();
        let seccomp_filter = Path::new("/proc/sys/kernel/seccomp/actions_avail").exists();
        let setns = Path::new("/proc/self/ns").is_dir();
        // SAFETY: zero is a valid initial representation for statfs output.
        let mut metadata = unsafe { mem::zeroed::<libc::statfs>() };
        // SAFETY: the path is NUL terminated and metadata is writable.
        let cgroup_v2 = unsafe { libc::statfs(c"/sys/fs/cgroup".as_ptr(), &raw mut metadata) } == 0 && metadata.f_type == 0x6367_7270;
        Self {
            landlock_abi,
            seccomp_filter,
            setns,
            cgroup_v2,
        }
    }
}

impl AuditArchitecture {
    const fn raw(self) -> u32 {
        match self {
            Self::X86_64 => 0xc000_003e,
            Self::Aarch64 => 0xc000_00b7,
        }
    }

    /// Returns the architecture of the current target.
    #[must_use]
    #[expect(clippy::unnecessary_wraps, reason = "unsupported Linux architectures return None")]
    pub const fn current() -> Option<Self> {
        #[cfg(target_arch = "x86_64")]
        {
            Some(Self::X86_64)
        }
        #[cfg(target_arch = "aarch64")]
        {
            Some(Self::Aarch64)
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            None
        }
    }
}

/// One classic-BPF seccomp instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SeccompInstruction {
    /// Classic-BPF opcode.
    pub code: u16,
    /// Forward offset when a conditional jump is true.
    pub jump_true: u8,
    /// Forward offset when a conditional jump is false.
    pub jump_false: u8,
    /// Instruction operand.
    pub value: u32,
}

/// A validated seccomp classic-BPF program.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeccompPolicy {
    architecture: AuditArchitecture,
    instructions: Vec<SeccompInstruction>,
}

impl SeccompPolicy {
    /// Validates a bounded classic-BPF filter for the current architecture.
    ///
    /// The first three instructions must load `seccomp_data.arch`, compare it
    /// with `architecture`, and return `SECCOMP_RET_KILL_PROCESS` on mismatch.
    /// `SECCOMP_RET_USER_NOTIF` is not supported.
    ///
    /// The filter must allow `write(3)` and `close(3)` so the runtime can
    /// acknowledge successful installation and remove its private channel
    /// before application entry.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty, oversized, malformed, architecture-
    /// mismatched, or user-notification program.
    pub fn new(architecture: AuditArchitecture, instructions: Vec<SeccompInstruction>) -> io::Result<Self> {
        if AuditArchitecture::current() != Some(architecture) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "seccomp architecture does not match this target",
            ));
        }
        let proto = ProtoSeccompPolicy {
            audit_architecture: architecture.raw(),
            instructions: instructions
                .iter()
                .map(|instruction| ProtoSeccompInstruction {
                    code: u32::from(instruction.code),
                    jump_true: u32::from(instruction.jump_true),
                    jump_false: u32::from(instruction.jump_false),
                    value: instruction.value,
                })
                .collect(),
        };
        let sandbox = LinuxSandbox {
            privileges: Some(PrivilegePolicy::secure_default().into_proto()),
            seccomp: Some(proto),
            landlock: false,
            cgroup: false,
            namespaces: Vec::new(),
            landlock_abi: 0,
            landlock_handled_access: 0,
        };
        zygote_rt::protocol::validate(&packet(
            1,
            Body::Launch(Launch {
                argv: vec![b"x".to_vec()],
                environment: Vec::new(),
                cwd: None,
                unix: None,
                rlimits: Vec::new(),
                sandbox: Some(sandbox),
            }),
            vec![DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr],
        ))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        Ok(Self {
            architecture,
            instructions,
        })
    }
}

/// Reduction-only Linux privilege controls.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each field records an independent validated kernel guarantee"
)]
pub struct PrivilegePolicy {
    no_new_privileges: bool,
    disable_dumping: bool,
    clear_capabilities: bool,
    drop_capability_bounding_set: bool,
    lock_securebits: bool,
}

/// Builder for a validated [`PrivilegePolicy`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[expect(clippy::struct_excessive_bools, reason = "each field requests an independent kernel guarantee")]
pub struct PrivilegePolicyBuilder {
    no_new_privileges: bool,
    disable_dumping: bool,
    clear_capabilities: bool,
    drop_capability_bounding_set: bool,
    lock_securebits: bool,
}

impl PrivilegePolicy {
    /// Starts building a privilege-reduction policy.
    #[must_use]
    pub const fn builder() -> PrivilegePolicyBuilder {
        PrivilegePolicyBuilder::new()
    }

    /// Returns the recommended irreversible reduction policy.
    #[must_use]
    pub const fn secure_default() -> Self {
        Self {
            no_new_privileges: true,
            disable_dumping: true,
            clear_capabilities: true,
            drop_capability_bounding_set: true,
            lock_securebits: true,
        }
    }

    fn into_proto(self) -> PrivilegeReduction {
        PrivilegeReduction {
            no_new_privs: self.no_new_privileges,
            disable_dumping: self.disable_dumping,
            clear_capabilities: self.clear_capabilities,
            drop_capability_bounding_set: self.drop_capability_bounding_set,
            lock_securebits: self.lock_securebits,
        }
    }

    const fn reduces_privileges(self) -> bool {
        self.no_new_privileges
            || self.disable_dumping
            || self.clear_capabilities
            || self.drop_capability_bounding_set
            || self.lock_securebits
    }
}

impl PrivilegePolicyBuilder {
    /// Creates an empty policy builder.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            no_new_privileges: false,
            disable_dumping: false,
            clear_capabilities: false,
            drop_capability_bounding_set: false,
            lock_securebits: false,
        }
    }

    /// Configures prevention of privilege gain through `execve`.
    #[must_use]
    pub const fn no_new_privileges(mut self, enabled: bool) -> Self {
        self.no_new_privileges = enabled;
        self
    }

    /// Configures making the child non-dumpable.
    #[must_use]
    pub const fn disable_dumping(mut self, enabled: bool) -> Self {
        self.disable_dumping = enabled;
        self
    }

    /// Configures clearing all capability sets.
    #[must_use]
    pub const fn clear_capabilities(mut self, enabled: bool) -> Self {
        self.clear_capabilities = enabled;
        self
    }

    /// Configures removal of every capability from the bounding set.
    #[must_use]
    pub const fn drop_capability_bounding_set(mut self, enabled: bool) -> Self {
        self.drop_capability_bounding_set = enabled;
        self
    }

    /// Configures locking securebits against capability regain.
    #[must_use]
    pub const fn lock_securebits(mut self, enabled: bool) -> Self {
        self.lock_securebits = enabled;
        self
    }

    /// Validates and creates the policy.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] when no privilege-reducing
    /// control is enabled.
    pub fn build(self) -> io::Result<PrivilegePolicy> {
        let policy = PrivilegePolicy {
            no_new_privileges: self.no_new_privileges,
            disable_dumping: self.disable_dumping,
            clear_capabilities: self.clear_capabilities,
            drop_capability_bounding_set: self.drop_capability_bounding_set,
            lock_securebits: self.lock_securebits,
        };
        if !policy.reduces_privileges() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "privilege policy must enable at least one reduction",
            ));
        }
        Ok(policy)
    }
}

/// A caller-created Landlock ruleset capability.
#[derive(Debug)]
pub struct LandlockRuleset {
    descriptor: OwnedFd,
    abi: u32,
    handled_access: u64,
}

bitflags::bitflags! {
    /// Filesystem access rights handled by a Landlock ruleset.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct LandlockAccess: u64 {
        /// Execute a file.
        const EXECUTE = 1 << 0;
        /// Open a file for writing.
        const WRITE_FILE = 1 << 1;
        /// Open a file for reading.
        const READ_FILE = 1 << 2;
        /// Read a directory.
        const READ_DIR = 1 << 3;
        /// Remove a directory.
        const REMOVE_DIR = 1 << 4;
        /// Remove a file.
        const REMOVE_FILE = 1 << 5;
        /// Create a character device.
        const MAKE_CHAR = 1 << 6;
        /// Create a directory.
        const MAKE_DIR = 1 << 7;
        /// Create a regular file.
        const MAKE_REGULAR = 1 << 8;
        /// Create a Unix-domain socket.
        const MAKE_SOCKET = 1 << 9;
        /// Create a FIFO.
        const MAKE_FIFO = 1 << 10;
        /// Create a block device.
        const MAKE_BLOCK = 1 << 11;
        /// Create a symbolic link.
        const MAKE_SYMLINK = 1 << 12;
        /// Refer to an object across directories.
        const REFER = 1 << 13;
        /// Truncate a file.
        const TRUNCATE = 1 << 14;
    }
}

impl LandlockRuleset {
    /// Creates a Landlock ruleset handling exactly `handled_access`.
    ///
    /// The caller opens each allowed path, adds it to the ruleset, and then
    /// transfers the completed ruleset to one command:
    ///
    /// ```no_run
    /// use std::fs::File;
    /// use std::io;
    /// use std::os::fd::AsFd as _;
    ///
    /// use zygote_control::Zygote;
    /// use zygote_control::linux::{
    ///     CommandExt as _, LandlockAccess, LandlockRuleset, PrivilegePolicy,
    /// };
    ///
    /// fn main() -> io::Result<()> {
    ///     let allowed = File::open("/srv/application-data")?;
    ///     let rights = LandlockAccess::READ_FILE | LandlockAccess::READ_DIR;
    ///     let mut ruleset = LandlockRuleset::create(rights)?;
    ///     ruleset.add_path_beneath(allowed.as_fd(), rights)?;
    ///
    ///     let zygote = Zygote::builder("/path/to/integrated-target").spawn()?;
    ///     let mut command = zygote.command();
    ///     command
    ///         .privilege_policy(PrivilegePolicy::builder().no_new_privileges(true).build()?)
    ///         .landlock(ruleset);
    ///     let status = command.status()?;
    ///     assert!(status.success());
    ///     Ok(())
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error rather than removing rights unsupported by the
    /// running kernel.
    pub fn create(handled_access: LandlockAccess) -> io::Result<Self> {
        #[repr(C)]
        struct RulesetAttr {
            handled_access_fs: u64,
        }
        let handled_access = handled_access.bits();
        if handled_access == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Landlock handled rights must be nonzero",
            ));
        }
        let abi = Self::abi_version()?;
        let attributes = RulesetAttr {
            handled_access_fs: handled_access,
        };
        // SAFETY: attributes matches the kernel Landlock ABI and its size is exact.
        let descriptor = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &raw const attributes,
                mem::size_of::<RulesetAttr>(),
                0u32,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a successful syscall returns a newly owned descriptor.
        let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor as RawFd) };
        Ok(Self {
            descriptor,
            abi,
            handled_access,
        })
    }

    /// Adds one path-beneath rule using a caller-opened directory or file.
    ///
    /// # Errors
    ///
    /// Returns an error when `allowed_access` is empty, exceeds the handled
    /// set, or the kernel rejects the rule.
    pub fn add_path_beneath(&mut self, parent: BorrowedFd<'_>, allowed_access: LandlockAccess) -> io::Result<&mut Self> {
        #[repr(C)]
        struct PathBeneathAttr {
            allowed_access: u64,
            parent_fd: i32,
            reserved: u32,
        }
        let allowed_access = allowed_access.bits();
        if allowed_access == 0 || allowed_access & !self.handled_access != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Landlock rule exceeds handled rights"));
        }
        let attributes = PathBeneathAttr {
            allowed_access,
            parent_fd: parent.as_raw_fd(),
            reserved: 0,
        };
        // SAFETY: both descriptors are live and attributes has the documented layout.
        let result = unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                self.descriptor.as_raw_fd(),
                LANDLOCK_RULE_PATH_BENEATH,
                &raw const attributes,
                0u32,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(self)
    }

    /// Queries the running kernel's highest Landlock ABI.
    ///
    /// # Errors
    ///
    /// Returns the kernel error when Landlock is unavailable.
    pub fn abi_version() -> io::Result<u32> {
        const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
        // SAFETY: a null attribute with VERSION is the documented ABI query.
        let result = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<u8>(),
                0usize,
                LANDLOCK_CREATE_RULESET_VERSION,
            )
        };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            u32::try_from(result).map_err(|_overflow| io::Error::other("invalid Landlock ABI returned by kernel"))
        }
    }
}

/// Race-free cgroup v2 membership capability.
#[derive(Debug)]
pub struct CgroupMembership {
    descriptor: OwnedFd,
}

impl CgroupMembership {
    /// Opens `cgroup.procs` relative to a caller-selected cgroup directory.
    ///
    /// # Errors
    ///
    /// Returns an error if `directory` is not a cgroup v2 directory or its
    /// `cgroup.procs` child cannot be opened for writing without following a
    /// symbolic link.
    pub fn new(directory: impl AsFd) -> io::Result<Self> {
        // SAFETY: zero is a valid initial representation for stat output.
        let mut status = unsafe { mem::zeroed::<libc::stat>() };
        // SAFETY: directory is live and status is writable.
        if unsafe { libc::fstat(directory.as_fd().as_raw_fd(), &raw mut status) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if status.st_mode & libc::S_IFMT != libc::S_IFDIR {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cgroup membership requires a directory descriptor",
            ));
        }
        // SAFETY: zero is a valid initial representation for statfs output.
        let mut metadata = unsafe { mem::zeroed::<libc::statfs>() };
        // SAFETY: directory is live and metadata is writable.
        if unsafe { libc::fstatfs(directory.as_fd().as_raw_fd(), &raw mut metadata) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if metadata.f_type != CGROUP2_SUPER_MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "directory is not on cgroup v2"));
        }
        // SAFETY: directory is a live directory descriptor, the child name is
        // fixed and NUL terminated, and O_NOFOLLOW prevents substituting a
        // symbolic-link target for the kernel-provided control file.
        let descriptor = unsafe {
            libc::openat(
                directory.as_fd().as_raw_fd(),
                c"cgroup.procs".as_ptr(),
                libc::O_WRONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a new descriptor owned by this function.
        let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
        Ok(Self { descriptor })
    }
}

/// A supported Linux namespace.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum NamespaceKind {
    /// Cgroup namespace.
    Cgroup,
    /// IPC namespace.
    Ipc,
    /// UTS namespace.
    Uts,
    /// Network namespace.
    Network,
    /// Time namespace.
    Time,
    /// Mount namespace.
    Mount,
}

impl NamespaceKind {
    const fn proto(self) -> ProtoNamespaceKind {
        match self {
            Self::Cgroup => ProtoNamespaceKind::Cgroup,
            Self::Ipc => ProtoNamespaceKind::Ipc,
            Self::Uts => ProtoNamespaceKind::Uts,
            Self::Network => ProtoNamespaceKind::Network,
            Self::Time => ProtoNamespaceKind::Time,
            Self::Mount => ProtoNamespaceKind::Mount,
        }
    }
}

/// An open namespace descriptor.
#[derive(Debug)]
pub struct Namespace {
    kind: NamespaceKind,
    descriptor: OwnedFd,
}

impl Namespace {
    /// Creates a typed namespace capability.
    ///
    /// # Errors
    ///
    /// Returns an error if the descriptor is not the requested namespace
    /// type.
    pub fn new(kind: NamespaceKind, descriptor: OwnedFd) -> io::Result<Self> {
        const NS_GET_NSTYPE: libc::c_ulong = 0xb703;
        // SAFETY: NS_GET_NSTYPE reads metadata from the live descriptor.
        let namespace_type = unsafe { libc::ioctl(descriptor.as_raw_fd(), NS_GET_NSTYPE) };
        let expected = match kind {
            NamespaceKind::Cgroup => libc::CLONE_NEWCGROUP,
            NamespaceKind::Ipc => libc::CLONE_NEWIPC,
            NamespaceKind::Uts => libc::CLONE_NEWUTS,
            NamespaceKind::Network => libc::CLONE_NEWNET,
            NamespaceKind::Time => libc::CLONE_NEWTIME,
            NamespaceKind::Mount => libc::CLONE_NEWNS,
        };
        if namespace_type < 0 {
            return Err(io::Error::last_os_error());
        }
        if namespace_type != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "namespace descriptor has the wrong type",
            ));
        }
        Ok(Self { kind, descriptor })
    }

    /// Rejects user and PID namespace flags, which require different
    /// lifecycle and credential semantics.
    ///
    /// # Errors
    ///
    /// Returns `Unsupported` for user/PID namespaces and `InvalidInput` for
    /// unknown or mismatched namespace kinds.
    pub fn from_clone_flag(flag: i32, descriptor: OwnedFd) -> io::Result<Self> {
        let kind = match flag {
            libc::CLONE_NEWCGROUP => NamespaceKind::Cgroup,
            libc::CLONE_NEWIPC => NamespaceKind::Ipc,
            libc::CLONE_NEWUTS => NamespaceKind::Uts,
            libc::CLONE_NEWNET => NamespaceKind::Network,
            libc::CLONE_NEWTIME => NamespaceKind::Time,
            libc::CLONE_NEWNS => NamespaceKind::Mount,
            libc::CLONE_NEWUSER | libc::CLONE_NEWPID => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "user and PID namespace entry is not supported",
                ));
            }
            _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "unknown namespace flag")),
        };
        Self::new(kind, descriptor)
    }
}

/// Linux-specific sandbox options for one command.
#[derive(Debug, Default)]
pub(super) struct SandboxOptions {
    privileges: Option<PrivilegePolicy>,
    seccomp: Option<SeccompPolicy>,
    landlock: Option<LandlockRuleset>,
    cgroup: Option<CgroupMembership>,
    namespaces: Vec<Namespace>,
}

/// Linux-only sandbox extensions for [`Command`].
///
/// This trait is sealed and cannot be implemented outside this crate.
///
/// ```compile_fail
/// struct ForeignCommand;
/// impl zygote_control::linux::CommandExt for ForeignCommand {}
/// ```
pub trait CommandExt: crate::sealed::Sealed {
    /// Sets irreversible privilege reduction.
    fn privilege_policy(&mut self, policy: PrivilegePolicy) -> &mut Self;
    /// Installs a validated seccomp filter.
    fn seccomp(&mut self, policy: SeccompPolicy) -> &mut Self;
    /// Restricts filesystem access with a caller-created Landlock ruleset.
    fn landlock(&mut self, ruleset: LandlockRuleset) -> &mut Self;
    /// Joins a caller-selected cgroup v2 before application entry.
    fn cgroup(&mut self, membership: CgroupMembership) -> &mut Self;
    /// Enters a caller-opened namespace.
    ///
    /// # Errors
    ///
    /// Returns an error when the same namespace kind is configured twice.
    fn namespace(&mut self, namespace: Namespace) -> io::Result<&mut Self>;
}

impl CommandExt for Command {
    fn privilege_policy(&mut self, policy: PrivilegePolicy) -> &mut Self {
        self.linux_sandbox.privileges = Some(policy);
        self
    }

    fn seccomp(&mut self, policy: SeccompPolicy) -> &mut Self {
        self.linux_sandbox.seccomp = Some(policy);
        self
    }

    fn landlock(&mut self, ruleset: LandlockRuleset) -> &mut Self {
        self.linux_sandbox.landlock = Some(ruleset);
        self
    }

    fn cgroup(&mut self, membership: CgroupMembership) -> &mut Self {
        self.linux_sandbox.cgroup = Some(membership);
        self
    }

    fn namespace(&mut self, namespace: Namespace) -> io::Result<&mut Self> {
        if self.linux_sandbox.namespaces.iter().any(|existing| existing.kind == namespace.kind) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "namespace kind configured more than once",
            ));
        }
        self.linux_sandbox.namespaces.push(namespace);
        self.linux_sandbox.namespaces.sort_by_key(|namespace| namespace.kind);
        Ok(self)
    }
}

// Bounds bootstrap hangs without imposing a steady-state launch timeout. Keep
// this above normal dynamic-linker startup latency on supported systems.
const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PENDING_REQUESTS: usize = 64;
const COMMON_EVENT_PACKET_LEN: usize = 4 * 1024;
const CGROUP2_SUPER_MAGIC: libc::c_long = 0x6367_7270;
const LANDLOCK_RULE_PATH_BENEATH: u32 = 1;
#[cfg(test)]
static FAIL_EVENT_READER_START: AtomicBool = AtomicBool::new(false);

pub(super) struct LinuxShared {
    lifecycle: Mutex<Lifecycle>,
    pending: Mutex<HashMap<u64, mpsc::Sender<Event>>>,
    started_requests: Mutex<HashSet<u64>>,
    next_request_id: AtomicU64,
    stdio_placeholder: OwnedFd,
    template: Mutex<Option<std::process::Child>>,
    reader: Mutex<Option<JoinHandle<()>>>,
    owner: Weak<WorkerSlot>,
    health: Arc<HealthState>,
    reported_prefork_refills: AtomicU64,
    reported_prefork_refill_nanoseconds: AtomicU64,
    reported_idle_prefork_workers: AtomicUsize,
}

pub(super) struct LinuxPool {
    workers: Box<[Arc<WorkerSlot>]>,
    next: AtomicUsize,
    health: Arc<HealthState>,
}

impl LinuxPool {
    pub(super) fn spawn(&self, command: &Command, stdin: Stdio, stdout: Stdio, stderr: Stdio) -> io::Result<Child> {
        let first = self.next.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        let mut saturated = false;
        for offset in 0..self.workers.len() {
            let worker = &self.workers[(first + offset) % self.workers.len()];
            if let Some(shared) = worker.current()? {
                let Some(reservation) = reserve_launch(&shared)? else {
                    saturated = true;
                    continue;
                };
                return spawn_reserved(command, &shared, reservation, stdin, stdout, stderr);
            }
        }
        if saturated {
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "all healthy zygote workers have too many pending launch requests",
            ))
        } else {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "no healthy zygote worker is currently available",
            ))
        }
    }

    pub(super) fn shutdown(&self) -> io::Result<()> {
        self.health.shutting_down.store(true, Ordering::Relaxed);
        let mut first_error = None;
        for worker in &self.workers {
            worker.stopping.store(true, Ordering::Release);
            let Some(worker) = worker.current()? else {
                continue;
            };
            if let Err(error) = worker.shutdown()
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        self.health.ready_workers.store(0, Ordering::Relaxed);
        first_error.map_or(Ok(()), Err)
    }

    pub(super) fn trim_prefork_pool(&self, idle_workers: usize) -> io::Result<()> {
        if self.workers.iter().any(|worker| idle_workers > worker.prefork_pool.max_idle) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "prefork trim target exceeds the configured maximum",
            ));
        }
        let idle_workers = u32::try_from(idle_workers)
            .map_err(|_too_large| io::Error::new(io::ErrorKind::InvalidInput, "prefork trim target is too large"))?;
        for worker in &self.workers {
            let Some(shared) = worker.current()? else {
                continue;
            };
            let lifecycle = shared.lifecycle.lock().map_err(poisoned_lock)?;
            if lifecycle.shutdown {
                continue;
            }
            send_packet(
                lifecycle.socket.as_raw_fd(),
                &packet(0, Body::PoolControl(PoolControl { idle_workers }), Vec::new()),
                &[],
            )?;
            shared.reported_idle_prefork_workers.store(idle_workers as usize, Ordering::Relaxed);
        }
        self.health
            .idle_prefork_workers
            .store(idle_workers as usize * self.workers.len(), Ordering::Relaxed);
        Ok(())
    }
}

struct WorkerSlot {
    program: OsString,
    prefork_pool: PreforkPoolConfig,
    recovery: WorkerRecoveryPolicy,
    current: RwLock<Option<Arc<LinuxShared>>>,
    stopping: AtomicBool,
    recovering: AtomicBool,
    attempts: AtomicUsize,
    health: Arc<HealthState>,
}

impl WorkerSlot {
    fn current(&self) -> io::Result<Option<Arc<LinuxShared>>> {
        Ok(self.current.read().map_err(poisoned_lock)?.clone())
    }

    fn install(&self, worker: Arc<LinuxShared>, replacement: bool) -> io::Result<()> {
        *self.current.write().map_err(poisoned_lock)? = Some(worker);
        self.health.ready_workers.fetch_add(1, Ordering::Relaxed);
        self.health
            .idle_prefork_workers
            .fetch_add(self.prefork_pool.min_idle, Ordering::Relaxed);
        if replacement {
            self.health.worker_restarts.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    fn worker_failed(self: &Arc<Self>, failed: &Arc<LinuxShared>) {
        if self.stopping.load(Ordering::Acquire) {
            return;
        }
        let removed = match self.current.write() {
            Ok(mut current) if current.as_ref().is_some_and(|worker| Arc::ptr_eq(worker, failed)) => {
                current.take();
                true
            }
            _ => false,
        };
        if !removed {
            return;
        }
        self.health.ready_workers.fetch_sub(1, Ordering::Relaxed);
        let failed_idle = failed.reported_idle_prefork_workers.load(Ordering::Relaxed);
        let _ = self
            .health
            .idle_prefork_workers
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                Some(count.saturating_sub(failed_idle))
            });
        if self.recovery.max_restarts == 0 || self.recovering.swap(true, Ordering::AcqRel) {
            return;
        }
        let slot = Arc::clone(self);
        let failed = Arc::clone(failed);
        let spawn_result = thread::Builder::new()
            .name("zygote-recovery".to_owned())
            .spawn(move || slot.recover(&failed));
        if spawn_result.is_err() {
            self.recovering.store(false, Ordering::Release);
            self.health.worker_restart_failures.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn recover(self: Arc<Self>, failed: &LinuxShared) {
        let _ = failed.shutdown();
        while !self.stopping.load(Ordering::Acquire) {
            let attempt = self.attempts.fetch_add(1, Ordering::Relaxed);
            if attempt >= self.recovery.max_restarts {
                break;
            }
            let exponent = u32::try_from(attempt.min(31)).unwrap_or(31);
            let delay = self
                .recovery
                .initial_backoff
                .saturating_mul(1u32 << exponent)
                .min(self.recovery.maximum_backoff);
            if !delay.is_zero() {
                thread::sleep(delay);
            }
            match start_worker(&self.program, self.prefork_pool, Arc::downgrade(&self), Arc::clone(&self.health)) {
                Ok(worker) => {
                    if self.stopping.load(Ordering::Acquire) {
                        let _ = worker.shutdown();
                        break;
                    }
                    if self.install(worker, true).is_ok() {
                        self.recovering.store(false, Ordering::Release);
                        return;
                    }
                }
                Err(_) => {
                    self.health.worker_restart_failures.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        self.recovering.store(false, Ordering::Release);
    }
}

struct Lifecycle {
    socket: OwnedFd,
    shutdown: bool,
}

impl fmt::Debug for LinuxShared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinuxShared").finish_non_exhaustive()
    }
}

enum Event {
    Started { pid: u32, pidfd: OwnedFd },
    Exited(ExitStatus),
    Error(io::Error),
}

pub(super) struct LinuxChild {
    pid: u32,
    pidfd: OwnedFd,
    events: mpsc::Receiver<Event>,
    status: Option<ExitStatus>,
}

impl LinuxChild {
    pub(super) const fn id(&self) -> u32 {
        self.pid
    }

    pub(super) fn kill(&mut self) -> io::Result<()> {
        let result = unsafe {
            // SAFETY: pidfd is owned by this Child, SIGKILL is a valid signal,
            // and the remaining arguments are required to be null/zero.
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.pidfd.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0u32,
            )
        };
        if result == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    pub(super) fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        match self.events.recv() {
            Ok(Event::Exited(status)) => {
                self.status = Some(status);
                Ok(status)
            }
            Ok(Event::Error(error)) => Err(error),
            Ok(Event::Started { .. }) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "received duplicate zygote child-start event",
            )),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "zygote exited before reporting child status",
            )),
        }
    }

    pub(super) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.try_wait_inner(false)
    }

    pub(super) fn try_wait_for_output(&mut self) -> io::Result<Option<ExitStatus>> {
        self.try_wait_inner(true)
    }

    fn try_wait_inner(&mut self, detached_is_error: bool) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            return Ok(Some(status));
        }
        match self.events.try_recv() {
            Ok(Event::Exited(status)) => {
                self.status = Some(status);
                Ok(Some(status))
            }
            Ok(Event::Error(error)) => Err(error),
            Ok(Event::Started { .. }) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "received duplicate zygote child-start event",
            )),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                if detached_is_error || self.pidfd_has_exited()? {
                    Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "zygote exited before reporting child status",
                    ))
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn pidfd_has_exited(&self) -> io::Result<bool> {
        let mut descriptor = libc::pollfd {
            fd: self.pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            let result = unsafe {
                // SAFETY: descriptor points to one initialized pollfd and a
                // zero timeout only observes the retained pidfd.
                libc::poll(&mut descriptor, 1, 0)
            };
            if result == 0 {
                return Ok(false);
            }
            if result > 0 && descriptor.revents & libc::POLLIN != 0 {
                return Ok(true);
            }
            let error = io::Error::last_os_error();
            if result < 0 && error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return if result < 0 {
                Err(error)
            } else {
                Err(io::Error::other("pidfd reported an unexpected poll state"))
            };
        }
    }
}

impl AsFd for LinuxChild {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.pidfd.as_fd()
    }
}

pub(super) fn start(
    program: &OsStr,
    worker_count: NonZeroUsize,
    prefork_pool: PreforkPoolConfig,
    worker_recovery: WorkerRecoveryPolicy,
) -> io::Result<Zygote> {
    crate::zygote::ZygoteBuilder::validate_worker_count(worker_count.get())?;
    let health = Arc::new(HealthState::default());
    let mut workers = Vec::with_capacity(worker_count.get());
    for _ in 0..worker_count.get() {
        let slot = Arc::new(WorkerSlot {
            program: program.to_owned(),
            prefork_pool,
            recovery: worker_recovery,
            current: RwLock::new(None),
            stopping: AtomicBool::new(false),
            recovering: AtomicBool::new(false),
            attempts: AtomicUsize::new(0),
            health: Arc::clone(&health),
        });
        match start_worker(program, prefork_pool, Arc::downgrade(&slot), Arc::clone(&health)) {
            Ok(worker) => {
                slot.install(worker, false)?;
                workers.push(slot);
            }
            Err(error) => {
                for worker in &workers {
                    if let Ok(Some(worker)) = worker.current() {
                        let _ = worker.shutdown();
                    }
                }
                return Err(error);
            }
        }
    }

    let launcher = Launcher {
        program: Arc::new(program.to_owned()),
        inner: Arc::new(LauncherInner::Linux(LinuxPool {
            workers: workers.into_boxed_slice(),
            next: AtomicUsize::new(0),
            health: Arc::clone(&health),
        })),
        closed: Arc::new(std::sync::RwLock::new(false)),
        health,
    };
    Ok(Zygote { launcher })
}

fn start_worker(
    program: &OsStr,
    prefork_pool: PreforkPoolConfig,
    owner: Weak<WorkerSlot>,
    health: Arc<HealthState>,
) -> io::Result<Arc<LinuxShared>> {
    let closed_stdio = closed_standard_descriptors()?;
    let (controller_socket, target_socket) = socket_pair()?;
    let nonce = bootstrap_nonce()?;
    let target_fd = target_socket.as_raw_fd();
    let descriptor_limit = descriptor_limit()?;
    let mut command = process::Command::new(program);
    command
        .env_clear()
        .env(CONTROL_FD_ENV, CONTROL_FD.to_string())
        .env(NONCE_ENV, &nonce)
        .env("ZYGOTE_RT_PREFORK_MIN_IDLE", prefork_pool.min_idle.to_string())
        .env("ZYGOTE_RT_PREFORK_MAX_IDLE", prefork_pool.max_idle.to_string())
        .env("ZYGOTE_RT_PREFORK_REFILL_THRESHOLD", prefork_pool.refill_threshold.to_string())
        .env(
            "ZYGOTE_RT_PREFORK_REFILL_DELAY_MS",
            prefork_pool.refill_delay.as_millis().min(u128::from(u64::MAX)).to_string(),
        );

    unsafe {
        // SAFETY: this closure performs only direct syscalls and dup2/fcntl
        // operations documented as async-signal-safe between fork and exec.
        command.pre_exec(move || {
            mark_descriptors_close_on_exec(descriptor_limit)?;
            if target_fd != CONTROL_FD && libc::dup2(target_fd, CONTROL_FD) < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::fcntl(CONTROL_FD, libc::F_SETFD, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            for (descriptor, closed) in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO]
                .into_iter()
                .zip(closed_stdio)
            {
                if closed {
                    libc::close(descriptor);
                }
            }
            Ok(())
        });
    }

    let stdio_placeholder = null_stream(libc::STDIN_FILENO)?;
    let mut template = command.spawn()?;
    drop(target_socket);
    if let Err(error) = receive_ready(&controller_socket, nonce.as_bytes()) {
        abort_template(&mut template);
        return Err(error);
    }

    let reader_socket = match duplicate_fd(&controller_socket) {
        Ok(socket) => socket,
        Err(error) => {
            abort_template(&mut template);
            return Err(error);
        }
    };
    let shared = Arc::new(LinuxShared {
        lifecycle: Mutex::new(Lifecycle {
            socket: controller_socket,
            shutdown: false,
        }),
        pending: Mutex::new(HashMap::with_capacity(MAX_PENDING_REQUESTS)),
        started_requests: Mutex::new(HashSet::with_capacity(MAX_PENDING_REQUESTS)),
        next_request_id: AtomicU64::new(1),
        stdio_placeholder,
        template: Mutex::new(None),
        reader: Mutex::new(None),
        owner,
        health,
        reported_prefork_refills: AtomicU64::new(0),
        reported_prefork_refill_nanoseconds: AtomicU64::new(0),
        reported_idle_prefork_workers: AtomicUsize::new(prefork_pool.min_idle),
    });
    let reader = match spawn_event_reader(Arc::clone(&shared), reader_socket) {
        Ok(reader) => reader,
        Err(error) => {
            abort_template(&mut template);
            return Err(error);
        }
    };
    *shared.template.lock().map_err(poisoned_lock)? = Some(template);
    *shared.reader.lock().map_err(poisoned_lock)? = Some(reader);

    Ok(shared)
}

fn closed_standard_descriptors() -> io::Result<[bool; 3]> {
    let mut closed = [false; 3];
    for (index, descriptor) in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO]
        .into_iter()
        .enumerate()
    {
        // SAFETY: fcntl only observes whether this descriptor is open.
        if unsafe { libc::fcntl(descriptor, libc::F_GETFD) } >= 0 {
            continue;
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EBADF) {
            return Err(error);
        }
        closed[index] = true;
    }
    Ok(closed)
}

fn descriptor_limit() -> io::Result<libc::c_int> {
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: limit points to writable storage for the requested resource limit.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(libc::c_int::try_from(limit.rlim_cur.min(1_048_576)).unwrap_or(1_048_576))
}

fn mark_descriptors_close_on_exec(descriptor_limit: libc::c_int) -> io::Result<()> {
    // SAFETY: close_range receives scalar arguments and does not dereference pointers.
    let result = unsafe { libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, libc::CLOSE_RANGE_CLOEXEC) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(libc::ENOSYS) {
        return Err(error);
    }
    for descriptor in 3..descriptor_limit {
        // SAFETY: fcntl receives a descriptor number and scalar flag argument.
        let result = unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EBADF) {
                return Err(error);
            }
        }
    }
    Ok(())
}

fn spawn_event_reader(shared: Arc<LinuxShared>, socket: OwnedFd) -> io::Result<JoinHandle<()>> {
    #[cfg(test)]
    if FAIL_EVENT_READER_START.swap(false, Ordering::Relaxed) {
        return Err(io::Error::other("injected event reader start failure"));
    }
    thread::Builder::new()
        .name("zygote-control".to_owned())
        .spawn(move || reader_loop(shared, socket))
}

#[cfg(test)]
pub(super) fn spawn(command: &Command, shared: &Arc<LinuxShared>, stdin: Stdio, stdout: Stdio, stderr: Stdio) -> io::Result<Child> {
    let Some(reservation) = reserve_launch(shared)? else {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "zygote worker has too many pending launch requests",
        ));
    };
    spawn_reserved(command, shared, reservation, stdin, stdout, stderr)
}

struct ReservedLaunch {
    request_id: u64,
    events: mpsc::Receiver<Event>,
}

fn reserve_launch(shared: &LinuxShared) -> io::Result<Option<ReservedLaunch>> {
    let mut pending = shared.pending.lock().map_err(poisoned_lock)?;
    if pending.len() >= MAX_PENDING_REQUESTS {
        return Ok(None);
    }
    let request_id = allocate_request_id(&shared.next_request_id)?;
    let (sender, events) = mpsc::channel();
    pending.insert(request_id, sender);
    Ok(Some(ReservedLaunch { request_id, events }))
}

fn spawn_reserved(
    command: &Command,
    shared: &Arc<LinuxShared>,
    reservation: ReservedLaunch,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
) -> io::Result<Child> {
    let started_at = Instant::now();
    let ReservedLaunch { request_id, events } = reservation;
    let (mut launch_packet, sandbox_fds) = match encode_launch(command, request_id) {
        Ok(encoded) => encoded,
        Err(error) => {
            shared.pending.lock().map_err(poisoned_lock)?.remove(&request_id);
            return Err(error);
        }
    };

    let prepared = match prepare_stdio(stdin, stdout, stderr, &shared.stdio_placeholder) {
        Ok(prepared) => prepared,
        Err(error) => {
            shared.pending.lock().map_err(poisoned_lock)?.remove(&request_id);
            return Err(error);
        }
    };
    for (role, prepared_role) in launch_packet.descriptor_roles[..3].iter_mut().zip(prepared.roles) {
        *role = i32::from(prepared_role);
    }
    let mut descriptors = Vec::with_capacity(launch_packet.descriptor_roles.len());
    descriptors.extend(prepared.child_fds.iter().map(AsRawFd::as_raw_fd));
    descriptors.extend(sandbox_fds);
    let lifecycle = match shared.lifecycle.lock() {
        Ok(lifecycle) => lifecycle,
        Err(error) => {
            let error = poisoned_lock(error);
            shared.pending.lock().map_err(poisoned_lock)?.remove(&request_id);
            return Err(error);
        }
    };
    if lifecycle.shutdown {
        drop(lifecycle);
        shared.pending.lock().map_err(poisoned_lock)?.remove(&request_id);
        return Err(io::Error::new(io::ErrorKind::BrokenPipe, "zygote is shut down"));
    }
    let send_result = send_packet(lifecycle.socket.as_raw_fd(), &launch_packet, &descriptors);
    drop(lifecycle);
    if let Err(error) = send_result {
        shared.pending.lock().map_err(poisoned_lock)?.remove(&request_id);
        return Err(error);
    }
    drop(prepared.child_fds);

    let (pid, pidfd) = match events.recv() {
        Ok(Event::Started { pid, pidfd }) => (pid, pidfd),
        Ok(Event::Error(error)) => return Err(error),
        Ok(Event::Exited(_)) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "zygote reported child exit before child start",
            ));
        }
        Err(_) => {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "zygote exited while launching child"));
        }
    };
    let latency = u64::try_from(started_at.elapsed().as_nanos()).unwrap_or(u64::MAX);
    shared.health.request_to_entry_nanoseconds.fetch_add(latency, Ordering::Relaxed);
    shared
        .health
        .maximum_request_to_entry_nanoseconds
        .fetch_max(latency, Ordering::Relaxed);

    Ok(Child {
        inner: ChildInner::Linux(LinuxChild {
            pid,
            pidfd,
            events,
            status: None,
        }),
        stdin: prepared.stdin,
        stdout: prepared.stdout,
        stderr: prepared.stderr,
        output_limits: command.output_limits,
    })
}

impl LinuxShared {
    fn shutdown(&self) -> io::Result<()> {
        let mut first_error = None;
        {
            let mut lifecycle = self.lifecycle.lock().map_err(poisoned_lock)?;
            if !lifecycle.shutdown {
                lifecycle.shutdown = true;
                let send_result = {
                    send_packet(
                        lifecycle.socket.as_raw_fd(),
                        &packet(0, Body::Shutdown(Shutdown {}), Vec::new()),
                        &[],
                    )
                };
                if let Err(error) = send_result {
                    first_error = Some(error);
                }
                let result = unsafe {
                    // SAFETY: socket is an owned Unix-domain socket descriptor.
                    libc::shutdown(lifecycle.socket.as_raw_fd(), libc::SHUT_RDWR)
                };
                if result != 0 && first_error.is_none() {
                    first_error = Some(io::Error::last_os_error());
                }
            }
        }

        let template = self.template.lock().map_err(poisoned_lock)?.take();
        if let Some(mut template) = template
            && let Err(error) = template.wait()
            && first_error.is_none()
        {
            first_error = Some(error);
        }

        if let Some(reader) = self.reader.lock().map_err(poisoned_lock)?.take()
            && reader.thread().id() != thread::current().id()
            && reader.join().is_err()
            && first_error.is_none()
        {
            first_error = Some(io::Error::other("zygote event reader panicked"));
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

fn abort_template(template: &mut std::process::Child) {
    let _ = template.kill();
    let _ = template.wait();
}

fn allocate_request_id(next: &AtomicU64) -> io::Result<u64> {
    next.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        (current != 0).then(|| current.wrapping_add(1))
    })
    .map_err(|_exhausted| io::Error::other("zygote request identifier space exhausted"))
}

struct PreparedStdio {
    child_fds: [OwnedFd; 3],
    roles: [DescriptorRole; 3],
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
}

struct PreparedStream {
    child: OwnedFd,
    parent: Option<Box<dyn IoPipe>>,
    role: DescriptorRole,
}

type WritePipe = Box<dyn Write + Send>;
type BoxedReadPipe = Box<dyn ReadPipe>;

fn null_stream(stream: RawFd) -> io::Result<OwnedFd> {
    let mut options = OpenOptions::new();
    if stream == 0 {
        options.read(true);
    } else {
        options.write(true);
    }
    relocate_above_stdio(options.open("/dev/null")?.into())
}

fn prepare_stream(
    configuration: Stdio,
    stream: RawFd,
    inherited_role: DescriptorRole,
    closed_role: DescriptorRole,
    placeholder: &impl AsFd,
) -> io::Result<PreparedStream> {
    let (child, parent, role) = match configuration.inner {
        StdioInner::Inherit => match duplicate_raw_fd(stream) {
            Ok(descriptor) => (descriptor, None, inherited_role),
            Err(error) if error.raw_os_error() == Some(libc::EBADF) => (duplicate_fd(placeholder)?, None, closed_role),
            Err(error) => return Err(error),
        },
        StdioInner::Null => (null_stream(stream)?, None, inherited_role),
        StdioInner::File(file) => (duplicate_fd(&file)?, None, inherited_role),
        StdioInner::Piped => {
            let (read, write) = pipe()?;
            if stream == 0 {
                (read, Some(Box::new(WriteIo(File::from(write))) as Box<dyn IoPipe>), inherited_role)
            } else {
                (write, Some(Box::new(ReadIo(File::from(read))) as Box<dyn IoPipe>), inherited_role)
            }
        }
    };
    Ok(PreparedStream { child, parent, role })
}

trait IoPipe: Send {
    fn into_write(self: Box<Self>) -> Option<WritePipe>;
    fn into_read(self: Box<Self>) -> Option<BoxedReadPipe>;
}

struct WriteIo(File);
struct ReadIo(File);

impl IoPipe for WriteIo {
    fn into_write(self: Box<Self>) -> Option<WritePipe> {
        Some(Box::new(self.0))
    }

    fn into_read(self: Box<Self>) -> Option<BoxedReadPipe> {
        None
    }
}

impl IoPipe for ReadIo {
    fn into_write(self: Box<Self>) -> Option<WritePipe> {
        None
    }

    fn into_read(self: Box<Self>) -> Option<BoxedReadPipe> {
        Some(Box::new(self.0))
    }
}

fn prepare_stdio(stdin: Stdio, stdout: Stdio, stderr: Stdio, placeholder: &impl AsFd) -> io::Result<PreparedStdio> {
    let stdin = prepare_stream(stdin, 0, DescriptorRole::Stdin, DescriptorRole::ClosedStdin, placeholder)?;
    let stdout = prepare_stream(stdout, 1, DescriptorRole::Stdout, DescriptorRole::ClosedStdout, placeholder)?;
    let stderr = prepare_stream(stderr, 2, DescriptorRole::Stderr, DescriptorRole::ClosedStderr, placeholder)?;
    Ok(PreparedStdio {
        child_fds: [stdin.child, stdout.child, stderr.child],
        roles: [stdin.role, stdout.role, stderr.role],
        stdin: stdin.parent.and_then(IoPipe::into_write).map(|inner| ChildStdin { inner }),
        stdout: stdout.parent.and_then(IoPipe::into_read).map(|inner| ChildStdout { inner }),
        stderr: stderr.parent.and_then(IoPipe::into_read).map(|inner| ChildStderr { inner }),
    })
}

#[expect(clippy::too_many_lines, reason = "validation and canonical wire construction stay adjacent")]
fn encode_launch(command: &Command, request_id: u64) -> io::Result<(Packet, Vec<RawFd>)> {
    let argument_count = command
        .args
        .len()
        .checked_add(1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "too many arguments"))?;
    if argument_count > MAX_ITEM_COUNT {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "too many arguments"));
    }
    let arg0 = command.unix.arg0.as_deref().unwrap_or(command.launcher.program.as_ref());
    let environment = command.resolved_environment();
    if environment.len() > MAX_ITEM_COUNT {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "too many environment variables"));
    }

    let directory = match &command.current_dir {
        Some(directory) if directory.is_absolute() => directory.clone(),
        Some(directory) => std::env::current_dir()?.join(directory),
        None => std::env::current_dir()?,
    };
    if command.unix.new_session && command.unix.process_group.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "new_session and process_group cannot be combined",
        ));
    }
    let mut launch_arguments = Vec::with_capacity(argument_count);
    for value in iter::once(arg0).chain(command.args.iter().map(OsString::as_os_str)) {
        launch_arguments.push(checked_unix_bytes(value)?);
    }
    let mut environment_entries = Vec::with_capacity(environment.len());
    for (key, value) in environment.iter() {
        validate_environment_key(key)?;
        if value.as_bytes().contains(&0) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "launch value contains a NUL byte"));
        }
        let mut entry = Vec::with_capacity(key.as_bytes().len() + 1 + value.as_bytes().len());
        entry.extend_from_slice(key.as_bytes());
        entry.push(b'=');
        entry.extend_from_slice(value.as_bytes());
        environment_entries.push(entry);
    }
    if let Some(groups) = &command.unix.groups
        && groups.len() > MAX_ITEM_COUNT
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "too many supplementary groups"));
    }
    if command.unix.resource_limits.len() > MAX_ITEM_COUNT {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "too many resource limits"));
    }
    let unix = UnixOptions {
        uid: command.unix.uid,
        gid: command.unix.gid,
        groups: command
            .unix
            .groups
            .as_ref()
            .map(|groups| SupplementaryGroups { gids: groups.clone() }),
        process_group: command.unix.process_group,
        new_session: command.unix.new_session,
        umask: command.unix.umask,
    };
    let privilege = if command.sandbox.privilege_intent() == PrivilegeIntent::Reduce {
        Some(PrivilegePolicy::secure_default())
    } else {
        command.linux_sandbox.privileges
    };
    let requires_no_new_privs = command.linux_sandbox.seccomp.is_some() || command.linux_sandbox.landlock.is_some();
    if requires_no_new_privs && !privilege.is_some_and(|policy| policy.no_new_privileges) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "seccomp and Landlock require no_new_privileges",
        ));
    }
    let sandbox_requested = privilege.is_some()
        || command.linux_sandbox.seccomp.is_some()
        || command.linux_sandbox.landlock.is_some()
        || command.linux_sandbox.cgroup.is_some()
        || !command.linux_sandbox.namespaces.is_empty();
    let sandbox = sandbox_requested.then(|| LinuxSandbox {
        privileges: privilege.map(PrivilegePolicy::into_proto),
        seccomp: command.linux_sandbox.seccomp.as_ref().map(|policy| ProtoSeccompPolicy {
            audit_architecture: policy.architecture.raw(),
            instructions: policy
                .instructions
                .iter()
                .map(|instruction| ProtoSeccompInstruction {
                    code: u32::from(instruction.code),
                    jump_true: u32::from(instruction.jump_true),
                    jump_false: u32::from(instruction.jump_false),
                    value: instruction.value,
                })
                .collect(),
        }),
        landlock: command.linux_sandbox.landlock.is_some(),
        cgroup: command.linux_sandbox.cgroup.is_some(),
        namespaces: command
            .linux_sandbox
            .namespaces
            .iter()
            .map(|namespace| i32::from(namespace.kind.proto()))
            .collect(),
        landlock_abi: command.linux_sandbox.landlock.as_ref().map_or(0, |ruleset| ruleset.abi),
        landlock_handled_access: command.linux_sandbox.landlock.as_ref().map_or(0, |ruleset| ruleset.handled_access),
    });
    let launch = Launch {
        argv: launch_arguments,
        environment: environment_entries,
        cwd: Some(checked_unix_bytes(directory.as_os_str())?),
        unix: Some(unix),
        rlimits: command
            .unix
            .resource_limits
            .iter()
            .map(|limit| Rlimit {
                resource: limit.resource.raw(),
                soft: limit.soft,
                hard: limit.hard,
            })
            .collect(),
        sandbox,
    };
    let mut roles = vec![DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr];
    let mut descriptors = Vec::new();
    if let Some(cgroup) = &command.linux_sandbox.cgroup {
        roles.push(DescriptorRole::CgroupProcs);
        descriptors.push(cgroup.descriptor.as_raw_fd());
    }
    if let Some(landlock) = &command.linux_sandbox.landlock {
        roles.push(DescriptorRole::LandlockRuleset);
        descriptors.push(landlock.descriptor.as_raw_fd());
    }
    for namespace in &command.linux_sandbox.namespaces {
        roles.push(match namespace.kind {
            NamespaceKind::Cgroup => DescriptorRole::NamespaceCgroup,
            NamespaceKind::Ipc => DescriptorRole::NamespaceIpc,
            NamespaceKind::Uts => DescriptorRole::NamespaceUts,
            NamespaceKind::Network => DescriptorRole::NamespaceNetwork,
            NamespaceKind::Time => DescriptorRole::NamespaceTime,
            NamespaceKind::Mount => DescriptorRole::NamespaceMount,
        });
        descriptors.push(namespace.descriptor.as_raw_fd());
    }
    let launch_packet = packet(request_id, Body::Launch(launch), roles);
    zygote_rt::protocol::validate(&launch_packet).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    // Reject oversized requests before preparing stdio; send_packet validates the final roles and encodes once.
    if launch_packet.encoded_len() > MAX_PACKET_LEN {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, ProtocolError::PacketTooLarge));
    }
    Ok((launch_packet, descriptors))
}

fn validate_environment_key(key: &OsStr) -> io::Result<()> {
    let bytes = key.as_bytes();
    if bytes.is_empty() || bytes.contains(&b'=') || bytes.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "environment variable name contains an invalid byte",
        ));
    }
    Ok(())
}

fn checked_unix_bytes(value: &OsStr) -> io::Result<Vec<u8>> {
    let bytes = value.as_bytes();
    if bytes.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "launch value contains a NUL byte"));
    }
    Ok(bytes.to_vec())
}

fn socket_pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut descriptors = [-1; 2];
    let result = unsafe {
        // SAFETY: descriptors points to space for two fds, and all constants
        // describe a valid local sequenced-packet socket pair.
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            descriptors.as_mut_ptr(),
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    let first = unsafe {
        // SAFETY: socketpair initialized this descriptor on success.
        OwnedFd::from_raw_fd(descriptors[0])
    };
    let second = unsafe {
        // SAFETY: socketpair initialized this descriptor on success.
        OwnedFd::from_raw_fd(descriptors[1])
    };
    Ok((relocate_above_stdio(first)?, relocate_above_stdio(second)?))
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut descriptors = [-1; 2];
    let result = unsafe {
        // SAFETY: descriptors points to space for two fds.
        libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC)
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    let read = unsafe {
        // SAFETY: pipe2 initialized this descriptor on success.
        OwnedFd::from_raw_fd(descriptors[0])
    };
    let write = unsafe {
        // SAFETY: pipe2 initialized this descriptor on success.
        OwnedFd::from_raw_fd(descriptors[1])
    };
    Ok((relocate_above_stdio(read)?, relocate_above_stdio(write)?))
}

fn duplicate_fd(fd: &impl AsFd) -> io::Result<OwnedFd> {
    duplicate_raw_fd(fd.as_fd().as_raw_fd())
}

fn duplicate_raw_fd(fd: RawFd) -> io::Result<OwnedFd> {
    let duplicated = unsafe {
        // SAFETY: fcntl does not take ownership of fd and returns a new fd.
        libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3)
    };
    if duplicated < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe {
        // SAFETY: F_DUPFD_CLOEXEC returned a new owned descriptor.
        OwnedFd::from_raw_fd(duplicated)
    })
}

fn relocate_above_stdio(fd: OwnedFd) -> io::Result<OwnedFd> {
    if fd.as_raw_fd() > libc::STDERR_FILENO {
        return Ok(fd);
    }
    duplicate_fd(&fd)
}

fn bootstrap_nonce() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    let mut offset = 0;
    while offset < bytes.len() {
        let result = unsafe {
            // SAFETY: the remaining slice is writable for the supplied length.
            libc::syscall(libc::SYS_getrandom, bytes[offset..].as_mut_ptr(), bytes.len() - offset, 0u32)
        };
        if result > 0 {
            offset += usize::try_from(result).expect("positive getrandom result fits usize");
            continue;
        }
        let error = io::Error::last_os_error();
        if result < 0 && error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return if result < 0 {
            Err(error)
        } else {
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "getrandom returned no data"))
        };
    }
    let mut nonce = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut nonce, "{byte:02x}").map_err(io::Error::other)?;
    }
    Ok(nonce)
}

fn receive_ready(socket: &impl AsFd, expected_nonce: &[u8]) -> io::Result<()> {
    let mut descriptor = libc::pollfd {
        fd: socket.as_fd().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let deadline = Instant::now() + BOOTSTRAP_TIMEOUT;
    let result = loop {
        let timeout = i32::try_from(deadline.saturating_duration_since(Instant::now()).as_millis()).unwrap_or(i32::MAX);
        let result = unsafe {
            // SAFETY: descriptor points to one initialized pollfd.
            libc::poll(&mut descriptor, 1, timeout)
        };
        if result < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        break result;
    };
    if result == 0 {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out waiting for zygote readiness"));
    }
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buffer = ReceiveBuffer::new();
    let packet = receive_packet(socket.as_fd().as_raw_fd(), &mut buffer)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "zygote exited before readiness"))?;
    let body = packet
        .message
        .body
        .as_ref()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "zygote readiness packet has no body"))?;
    let ready = match body {
        Body::Ready(ready) => ready,
        Body::Error(error) => {
            let source = io::Error::from_raw_os_error(error.error_number as i32);
            return Err(io::Error::new(
                source.kind(),
                format!("zygote startup failed at stage {}: {source}", error.stage),
            ));
        }
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid zygote readiness handshake")),
    };
    if packet.message.request_id != 0 || !packet.fds.is_empty() || ready.nonce != expected_nonce {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid zygote readiness handshake"));
    }
    Ok(())
}

struct ReceiveBuffer {
    packet: Box<[u8]>,
    control: Box<[usize]>,
}

impl ReceiveBuffer {
    fn new() -> Self {
        let control_len = unsafe {
            // SAFETY: MAX_FD_COUNT is a bounded compile-time constant.
            libc::CMSG_SPACE((MAX_FD_COUNT * mem::size_of::<RawFd>()) as u32) as usize
        };
        Self {
            packet: vec![0; COMMON_EVENT_PACKET_LEN].into_boxed_slice(),
            control: vec![0; control_len.div_ceil(mem::size_of::<usize>())].into_boxed_slice(),
        }
    }
}

struct ReceivedPacket {
    message: Packet,
    fds: ReceivedDescriptors,
}

struct ReceivedDescriptors {
    descriptors: [RawFd; MAX_FD_COUNT],
    len: usize,
}

impl ReceivedDescriptors {
    const fn new() -> Self {
        Self {
            descriptors: [-1; MAX_FD_COUNT],
            len: 0,
        }
    }

    const fn len(&self) -> usize {
        self.len
    }

    const fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn push(&mut self, descriptor: RawFd) -> io::Result<()> {
        let Some(slot) = self.descriptors.get_mut(self.len) else {
            unsafe {
                // SAFETY: recvmsg transferred ownership of this descriptor,
                // but the fixed table has no slot in which to retain it.
                libc::close(descriptor);
            }
            return Err(io::Error::new(io::ErrorKind::InvalidData, "too many received descriptors"));
        };
        *slot = descriptor;
        self.len += 1;
        Ok(())
    }

    fn take(&mut self, index: usize) -> Option<OwnedFd> {
        let descriptor = self.descriptors.get_mut(index)?;
        if *descriptor < 0 {
            return None;
        }
        let owned = unsafe {
            // SAFETY: recvmsg transferred this descriptor to the process, and
            // replacing the slot prevents Drop from closing it a second time.
            OwnedFd::from_raw_fd(*descriptor)
        };
        *descriptor = -1;
        Some(owned)
    }
}

impl Drop for ReceivedDescriptors {
    fn drop(&mut self) {
        for descriptor in &mut self.descriptors[..self.len] {
            if *descriptor >= 0 {
                unsafe {
                    // SAFETY: every nonnegative slot is an owned descriptor
                    // received through SCM_RIGHTS and has not been taken.
                    libc::close(*descriptor);
                }
                *descriptor = -1;
            }
        }
    }
}

fn send_packet(fd: RawFd, packet: &Packet, fds: &[RawFd]) -> io::Result<()> {
    if fds.len() != packet.descriptor_roles.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "zygote descriptor roles do not match its contents",
        ));
    }
    let encoded = zygote_rt::protocol::encode(packet).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let descriptor_bytes = mem::size_of_val(fds);
    let descriptor_bytes =
        u32::try_from(descriptor_bytes).map_err(|_overflow| io::Error::new(io::ErrorKind::InvalidInput, "too many descriptors"))?;
    let mut vector = libc::iovec {
        iov_base: encoded.as_ptr().cast_mut().cast(),
        iov_len: encoded.len(),
    };
    let control_len = if fds.is_empty() {
        0
    } else {
        unsafe {
            // SAFETY: the descriptor count is bounded by MAX_FD_COUNT.
            libc::CMSG_SPACE(descriptor_bytes) as usize
        }
    };
    let mut control = vec![0usize; control_len.div_ceil(mem::size_of::<usize>())];
    let mut message: libc::msghdr = unsafe {
        // SAFETY: zero is a valid initial state for msghdr.
        mem::zeroed()
    };
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    if !fds.is_empty() {
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = control_len;
        let control_header = unsafe {
            // SAFETY: control has CMSG_SPACE bytes with usize alignment.
            libc::CMSG_FIRSTHDR(&message)
        };
        if control_header.is_null() {
            return Err(io::Error::other("failed to allocate descriptor control message"));
        }
        unsafe {
            // SAFETY: control_header points inside control with enough room for
            // all bounded descriptors.
            (*control_header).cmsg_level = libc::SOL_SOCKET;
            (*control_header).cmsg_type = libc::SCM_RIGHTS;
            (*control_header).cmsg_len = libc::CMSG_LEN(descriptor_bytes) as usize;
            std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(control_header).cast(), fds.len());
        }
    }

    loop {
        let sent = unsafe {
            // SAFETY: message references live header, payload, and control
            // buffers for the duration of sendmsg.
            libc::sendmsg(fd, &message, libc::MSG_NOSIGNAL)
        };
        if sent == encoded.len() as isize {
            return Ok(());
        }
        if sent < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(if sent < 0 {
            io::Error::last_os_error()
        } else {
            io::Error::new(io::ErrorKind::WriteZero, "partial zygote packet write")
        });
    }
}

fn receive_packet(fd: RawFd, buffer: &mut ReceiveBuffer) -> io::Result<Option<ReceivedPacket>> {
    let packet_len = loop {
        let mut byte = 0u8;
        let received = unsafe {
            // SAFETY: byte is writable and MSG_PEEK leaves the single-reader
            // sequenced packet queued for the following recvmsg.
            libc::recv(fd, (&raw mut byte).cast(), 1, libc::MSG_PEEK | libc::MSG_TRUNC)
        };
        if received < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if received < 0 {
            return Err(io::Error::last_os_error());
        }
        break usize::try_from(received).map_err(|_overflow| io::Error::new(io::ErrorKind::InvalidData, "invalid zygote packet length"))?;
    };
    if packet_len == 0 {
        return Ok(None);
    }
    if packet_len > MAX_PACKET_LEN {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "zygote packet exceeds protocol limit"));
    }
    let mut oversized = (packet_len > buffer.packet.len()).then(|| vec![0u8; packet_len]);
    let packet_storage = oversized.as_deref_mut().unwrap_or(&mut buffer.packet);
    let mut vector = libc::iovec {
        iov_base: packet_storage.as_mut_ptr().cast(),
        iov_len: packet_storage.len(),
    };
    let mut message: libc::msghdr = unsafe {
        // SAFETY: zero is a valid initial state for msghdr.
        mem::zeroed()
    };
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    message.msg_control = buffer.control.as_mut_ptr().cast();
    let control_capacity = buffer.control.len() * mem::size_of::<usize>();
    message.msg_controllen = control_capacity;

    let received = loop {
        let received = unsafe {
            // SAFETY: message references writable packet and control buffers.
            libc::recvmsg(fd, &mut message, libc::MSG_CMSG_CLOEXEC)
        };
        if received < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        break received;
    };
    if received < 0 {
        return Err(io::Error::last_os_error());
    }
    let fds = receive_descriptors(&message, control_capacity)?;
    if received == 0 {
        return Ok(None);
    }
    let received =
        usize::try_from(received).map_err(|_overflow| io::Error::new(io::ErrorKind::InvalidData, "invalid zygote packet length"))?;
    if received > packet_storage.len() || received != packet_len {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "truncated zygote packet"));
    }
    let bytes = &packet_storage[..received];
    let message = decode(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if fds.len() != message.descriptor_roles.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "zygote descriptor count does not match its declared roles",
        ));
    }
    Ok(Some(ReceivedPacket { message, fds }))
}

fn receive_descriptors(message: &libc::msghdr, control_capacity: usize) -> io::Result<ReceivedDescriptors> {
    if message.msg_controllen > control_capacity {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid zygote descriptor message length",
        ));
    }
    let control_start = message.msg_control as usize;
    let control_end = control_start
        .checked_add(message.msg_controllen)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid zygote descriptor message bounds"))?;

    let mut fds = ReceivedDescriptors::new();
    let mut control_header = unsafe {
        // SAFETY: message was initialized by recvmsg.
        libc::CMSG_FIRSTHDR(message)
    };
    while !control_header.is_null() {
        unsafe {
            // SAFETY: pointer bounds are checked before the header or payload
            // is read; read_unaligned avoids assuming payload alignment.
            let header_start = control_header as usize;
            let header_end = header_start
                .checked_add(mem::size_of::<libc::cmsghdr>())
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid descriptor header bounds"))?;
            if header_start < control_start || header_end > control_end {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid descriptor header bounds"));
            }
            let control_message_len = (*control_header).cmsg_len;
            let base_len = libc::CMSG_LEN(0) as usize;
            let message_end = header_start
                .checked_add(control_message_len)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid descriptor message bounds"))?;
            if control_message_len < base_len || message_end > control_end {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid descriptor message bounds"));
            }
            if (*control_header).cmsg_level == libc::SOL_SOCKET && (*control_header).cmsg_type == libc::SCM_RIGHTS {
                let data_len = control_message_len - base_len;
                if !data_len.is_multiple_of(mem::size_of::<RawFd>()) {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "misaligned descriptor message"));
                }
                let count = data_len / mem::size_of::<RawFd>();
                let descriptor_bytes = libc::CMSG_DATA(control_header);
                let data_start = descriptor_bytes as usize;
                let data_end = data_start
                    .checked_add(data_len)
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid descriptor payload bounds"))?;
                if data_start < header_start || data_end > message_end || data_end > control_end {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid descriptor payload bounds"));
                }
                if count > MAX_FD_COUNT - fds.len() {
                    for index in 0..count {
                        let descriptor = descriptor_bytes
                            .add(index * mem::size_of::<RawFd>())
                            .cast::<RawFd>()
                            .read_unaligned();
                        libc::close(descriptor);
                    }
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "too many received descriptors"));
                }
                for index in 0..count {
                    let descriptor = descriptor_bytes
                        .add(index * mem::size_of::<RawFd>())
                        .cast::<RawFd>()
                        .read_unaligned();
                    fds.push(descriptor)?;
                }
            }
            control_header = libc::CMSG_NXTHDR(message, control_header);
        }
    }
    if message.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "truncated zygote packet"));
    }
    Ok(fds)
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the reader thread owns both values for its entire lifetime"
)]
#[expect(clippy::too_many_lines, reason = "the packet-routing state machine is kept together")]
fn reader_loop(shared: Arc<LinuxShared>, socket: OwnedFd) {
    let mut buffer = ReceiveBuffer::new();
    while let Ok(Some(packet)) = receive_packet(socket.as_raw_fd(), &mut buffer) {
        let request_id = packet.message.request_id;
        match packet.message.body {
            Some(Body::Started(started)) => {
                let mut fds = packet.fds;
                let Some(pidfd) = fds.take(0) else {
                    break;
                };
                let sender = match shared.pending.lock() {
                    Ok(pending) => pending.get(&request_id).cloned(),
                    Err(_) => break,
                };
                let Some(sender) = sender else {
                    break;
                };
                if let Ok(mut started_requests) = shared.started_requests.lock() {
                    if !started_requests.insert(request_id) {
                        break;
                    }
                    shared.health.active_children.fetch_add(1, Ordering::Relaxed);
                } else {
                    break;
                }
                if started.prefork_hit {
                    shared.health.prefork_hits.fetch_add(1, Ordering::Relaxed);
                } else {
                    shared.health.prefork_misses.fetch_add(1, Ordering::Relaxed);
                }
                apply_pool_state(
                    &shared,
                    started.idle_workers,
                    started.prefork_refills,
                    started.prefork_refill_nanoseconds,
                );
                let _ = sender.send(Event::Started { pid: started.pid, pidfd });
            }
            Some(Body::PoolState(state)) => {
                apply_pool_state(&shared, state.idle_workers, state.prefork_refills, state.prefork_refill_nanoseconds);
            }
            Some(Body::Exited(exited)) => {
                let raw = exited.wait_status as i32;
                let status = ExitStatus::from_raw(raw);
                let sender = match shared.pending.lock() {
                    Ok(mut pending) => pending.remove(&request_id),
                    Err(_) => break,
                };
                let Some(sender) = sender else {
                    break;
                };
                let was_started = match shared.started_requests.lock() {
                    Ok(mut started_requests) => started_requests.remove(&request_id),
                    Err(_) => break,
                };
                if !was_started {
                    let _ = sender.send(Event::Error(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "zygote reported child exit before child start",
                    )));
                    break;
                }
                decrement_active_children(&shared.health, 1);
                let _ = sender.send(Event::Exited(status));
            }
            Some(Body::Error(error_message)) => {
                let stage = error_message.stage;
                let error_number = error_message.error_number as i32;
                let source = io::Error::from_raw_os_error(error_number);
                let error = io::Error::new(source.kind(), format!("zygote launch failed at stage {stage}: {source}"));
                let sender = match shared.pending.lock() {
                    Ok(mut pending) => pending.remove(&request_id),
                    Err(_) => break,
                };
                let Some(sender) = sender else {
                    break;
                };
                if let Ok(mut started_requests) = shared.started_requests.lock()
                    && started_requests.remove(&request_id)
                {
                    decrement_active_children(&shared.health, 1);
                }
                let _ = sender.send(Event::Error(error));
            }
            _ => break,
        }
    }
    let mut expected_shutdown = true;
    if let Ok(mut lifecycle) = shared.lifecycle.lock() {
        expected_shutdown = lifecycle.shutdown;
        lifecycle.shutdown = true;
        unsafe {
            // SAFETY: socket is an owned Unix-domain socket descriptor. This
            // also shuts down its duplicate held by the exiting reader.
            libc::shutdown(lifecycle.socket.as_raw_fd(), libc::SHUT_RDWR);
        }
    }
    if let Ok(mut pending) = shared.pending.lock() {
        let started_requests = shared.started_requests.lock().ok();
        let template_status = shared
            .template
            .lock()
            .ok()
            .and_then(|mut template| template.as_mut().and_then(|template| template.try_wait().ok()).flatten());
        let started_count = started_requests.as_ref().map_or(0, |started| started.len());
        decrement_active_children(&shared.health, started_count);
        for (request_id, sender) in pending.drain() {
            let started = started_requests.as_ref().is_some_and(|started| started.contains(&request_id));
            let message = if started {
                "zygote worker disconnected; child exit status is unavailable".to_owned()
            } else if expected_shutdown {
                "zygote worker shut down before the launch completed".to_owned()
            } else {
                format!(
                    "zygote worker failed{}; launch outcome is indeterminate and was not replayed",
                    template_status.map_or_else(String::new, |status| format!(" with status {status}"))
                )
            };
            let _ = sender.send(Event::Error(io::Error::new(io::ErrorKind::BrokenPipe, message)));
        }
    }
    if let Ok(mut started_requests) = shared.started_requests.lock() {
        started_requests.clear();
    }

    if !expected_shutdown && let Some(owner) = shared.owner.upgrade() {
        owner.worker_failed(&shared);
    }
}

fn decrement_active_children(health: &HealthState, count: usize) {
    let _ = health
        .active_children
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |active| Some(active.saturating_sub(count)));
}

fn apply_pool_state(shared: &LinuxShared, idle_workers: u32, prefork_refills: u64, prefork_refill_nanoseconds: u64) {
    let idle_workers = idle_workers as usize;
    let previous_idle = shared.reported_idle_prefork_workers.swap(idle_workers, Ordering::Relaxed);
    if idle_workers >= previous_idle {
        shared
            .health
            .idle_prefork_workers
            .fetch_add(idle_workers - previous_idle, Ordering::Relaxed);
    } else {
        saturating_sub_counter(&shared.health.idle_prefork_workers, previous_idle - idle_workers);
    }
    let previous = shared.reported_prefork_refills.swap(prefork_refills, Ordering::Relaxed);
    shared
        .health
        .prefork_refills
        .fetch_add(prefork_refills.saturating_sub(previous), Ordering::Relaxed);
    let previous = shared
        .reported_prefork_refill_nanoseconds
        .swap(prefork_refill_nanoseconds, Ordering::Relaxed);
    shared
        .health
        .prefork_refill_nanoseconds
        .fetch_add(prefork_refill_nanoseconds.saturating_sub(previous), Ordering::Relaxed);
}

fn saturating_sub_counter(counter: &AtomicUsize, count: usize) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| Some(value.saturating_sub(count)));
}

fn poisoned_lock<T>(_error: PoisonError<T>) -> io::Error {
    io::Error::other("zygote controller lock was poisoned")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn privilege_builder_rejects_an_empty_policy() {
        assert_eq!(PrivilegePolicy::builder().build().unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert!(
            PrivilegePolicy::builder()
                .no_new_privileges(true)
                .build()
                .unwrap()
                .reduces_privileges()
        );
    }

    #[test]
    fn portable_privilege_reduction_cannot_be_weakened_by_a_native_override() {
        use crate::linux::CommandExt as _;

        let mut command = Command::new(Launcher::for_test("fixture"));
        command
            .current_dir("/")
            .sandbox(crate::SandboxPolicy::new().reduce_privileges())
            .privilege_policy(PrivilegePolicy::builder().disable_dumping(true).build().unwrap());
        let (launch_packet, _) = encode_launch(&command, 1).unwrap();
        let Some(Body::Launch(launch)) = launch_packet.body else {
            panic!("expected launch packet")
        };
        let privileges = launch.sandbox.unwrap().privileges.unwrap();

        assert!(privileges.no_new_privs);
        assert!(privileges.disable_dumping);
        assert!(privileges.clear_capabilities);
        assert!(privileges.drop_capability_bounding_set);
        assert!(privileges.lock_securebits);
    }

    #[test]
    fn launch_packet_size_check_matches_final_wire_encoding() {
        let mut command = Command::new(Launcher::for_test("fixture"));
        command.env_clear().current_dir("/").arg("a".repeat(MAX_PACKET_LEN / 2));
        let (initial, _) = encode_launch(&command, 128).unwrap();
        let remaining = MAX_PACKET_LEN - initial.encoded_len();
        command.args[0].push("a".repeat(remaining));

        let (mut at_limit, _) = encode_launch(&command, 128).unwrap();
        assert_eq!(at_limit.encoded_len(), MAX_PACKET_LEN);
        assert_eq!(zygote_rt::protocol::encode(&at_limit).unwrap().len(), MAX_PACKET_LEN);
        at_limit.descriptor_roles[..3].copy_from_slice(&[
            i32::from(DescriptorRole::ClosedStdin),
            i32::from(DescriptorRole::ClosedStdout),
            i32::from(DescriptorRole::ClosedStderr),
        ]);
        assert_eq!(zygote_rt::protocol::encode(&at_limit).unwrap().len(), MAX_PACKET_LEN);

        command.args[0].push("a");
        let error = encode_launch(&command, 128).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(error.to_string(), ProtocolError::PacketTooLarge.to_string());
    }

    fn shared_with_reader() -> (Arc<LinuxShared>, OwnedFd) {
        let (controller, peer) = socket_pair().unwrap();
        let reader_socket = duplicate_fd(&controller).unwrap();
        let health = Arc::new(HealthState::default());
        let shared = Arc::new(LinuxShared {
            lifecycle: Mutex::new(Lifecycle {
                socket: controller,
                shutdown: false,
            }),
            pending: Mutex::new(HashMap::new()),
            started_requests: Mutex::new(HashSet::new()),
            next_request_id: AtomicU64::new(1),
            stdio_placeholder: null_stream(libc::STDIN_FILENO).unwrap(),
            template: Mutex::new(None),
            reader: Mutex::new(None),
            owner: Weak::new(),
            health,
            reported_prefork_refills: AtomicU64::new(0),
            reported_prefork_refill_nanoseconds: AtomicU64::new(0),
            reported_idle_prefork_workers: AtomicUsize::new(0),
        });
        let reader_shared = Arc::clone(&shared);
        let reader = thread::spawn(move || reader_loop(reader_shared, reader_socket));
        *shared.reader.lock().unwrap() = Some(reader);
        (shared, peer)
    }

    fn finish_reader(shared: &LinuxShared, peer: OwnedFd) {
        drop(peer);
        shared.reader.lock().unwrap().take().unwrap().join().unwrap();
    }

    fn started_event(event: Event) -> (u32, OwnedFd) {
        match event {
            Event::Started { pid, pidfd } => (pid, pidfd),
            Event::Exited(_) => panic!("expected started event, got exit"),
            Event::Error(error) => panic!("expected started event, got {error}"),
        }
    }

    fn started_packet(request_id: u64, pid: u32) -> Packet {
        packet(
            request_id,
            Body::Started(Started {
                pid,
                prefork_hit: false,
                idle_workers: 0,
                prefork_refills: 0,
                prefork_refill_nanoseconds: 0,
            }),
            vec![DescriptorRole::Pidfd],
        )
    }

    fn exited_packet(request_id: u64) -> Packet {
        packet(request_id, Body::Exited(Exited { wait_status: 0 }), Vec::new())
    }

    fn pool_state_packet(idle_workers: u32, prefork_refills: u64, prefork_refill_nanoseconds: u64) -> Packet {
        packet(
            0,
            Body::PoolState(PoolState {
                idle_workers,
                prefork_refills,
                prefork_refill_nanoseconds,
            }),
            Vec::new(),
        )
    }

    fn assert_broken_pipe_event(event: Event, message: &str) {
        let Event::Error(error) = event else {
            panic!("expected explicit disconnect error")
        };
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert!(error.to_string().contains(message), "{error}");
    }

    #[test]
    fn request_identifier_exhaustion_is_terminal() {
        let next = AtomicU64::new(u64::MAX);

        assert_eq!(allocate_request_id(&next).unwrap(), u64::MAX);
        assert_eq!(
            allocate_request_id(&next).unwrap_err().to_string(),
            "zygote request identifier space exhausted"
        );
        assert_eq!(
            allocate_request_id(&next).unwrap_err().to_string(),
            "zygote request identifier space exhausted"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn output_wait_reports_detachment_while_public_try_wait_stays_nonblocking() {
        let (pidfd, writer) = pipe().unwrap();
        let (sender, events) = mpsc::channel();
        drop(sender);
        let mut child = LinuxChild {
            pid: 1,
            pidfd,
            events,
            status: None,
        };

        assert_eq!(child.try_wait().unwrap(), None);
        assert_eq!(child.try_wait_for_output().unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        drop(writer);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn try_wait_reports_explicit_post_start_errors() {
        let (pidfd, writer) = pipe().unwrap();
        let (sender, events) = mpsc::channel();
        sender
            .send(Event::Error(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "zygote disconnected after child start",
            )))
            .unwrap();
        drop(sender);
        let mut child = LinuxChild {
            pid: 1,
            pidfd,
            events,
            status: None,
        };

        let error = child.try_wait().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(error.to_string(), "zygote disconnected after child start");
        assert_eq!(child.wait().unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        drop(writer);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn pending_launch_requests_are_bounded_per_worker() {
        let (shared, peer) = shared_with_reader();
        let mut receivers = Vec::with_capacity(MAX_PENDING_REQUESTS);
        for request_id in 0..MAX_PENDING_REQUESTS as u64 {
            let (sender, receiver) = mpsc::channel();
            shared.pending.lock().unwrap().insert(request_id, sender);
            receivers.push(receiver);
        }

        let error = spawn(
            &Command::new(Launcher::for_test("fixture")),
            &shared,
            Stdio::null(),
            Stdio::null(),
            Stdio::null(),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(shared.pending.lock().unwrap().len(), MAX_PENDING_REQUESTS);

        shared.pending.lock().unwrap().clear();
        drop(receivers);
        finish_reader(&shared, peer);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn saturated_worker_does_not_block_dispatch_to_the_next_worker() {
        let (saturated, saturated_peer) = shared_with_reader();
        let mut saturated_receivers = Vec::with_capacity(MAX_PENDING_REQUESTS);
        for request_id in 0..MAX_PENDING_REQUESTS as u64 {
            let (sender, receiver) = mpsc::channel();
            saturated.pending.lock().unwrap().insert(request_id, sender);
            saturated_receivers.push(receiver);
        }
        let (available, available_peer) = shared_with_reader();
        let health = Arc::new(HealthState::default());
        let workers = [Arc::clone(&saturated), Arc::clone(&available)].map(|shared| {
            Arc::new(WorkerSlot {
                program: "/unused-test-target".into(),
                prefork_pool: PreforkPoolConfig::default(),
                recovery: WorkerRecoveryPolicy::default(),
                current: RwLock::new(Some(shared)),
                stopping: AtomicBool::new(false),
                recovering: AtomicBool::new(false),
                attempts: AtomicUsize::new(0),
                health: Arc::clone(&health),
            })
        });
        let pool = Arc::new(LinuxPool {
            workers: Box::new(workers),
            next: AtomicUsize::new(0),
            health,
        });
        let launch = {
            let pool = Arc::clone(&pool);
            thread::spawn(move || {
                pool.spawn(
                    &Command::new(Launcher::for_test("fixture")),
                    Stdio::null(),
                    Stdio::null(),
                    Stdio::null(),
                )
            })
        };

        let mut buffer = ReceiveBuffer::new();
        let packet = receive_packet(available_peer.as_raw_fd(), &mut buffer).unwrap().unwrap();
        assert!(matches!(packet.message.body, Some(Body::Launch(_))));
        let request_id = packet.message.request_id;
        let pidfd = File::open("/dev/null").unwrap();
        send_packet(available_peer.as_raw_fd(), &started_packet(request_id, 123), &[pidfd.as_raw_fd()]).unwrap();
        let mut child = launch.join().unwrap().unwrap();
        send_packet(available_peer.as_raw_fd(), &exited_packet(request_id), &[]).unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(saturated.pending.lock().unwrap().len(), MAX_PENDING_REQUESTS);
        assert_eq!(saturated.next_request_id.load(Ordering::Relaxed), 1);
        assert!(available.pending.lock().unwrap().is_empty());
        assert_eq!(available.next_request_id.load(Ordering::Relaxed), 2);

        saturated.pending.lock().unwrap().clear();
        drop(saturated_receivers);
        finish_reader(&saturated, saturated_peer);
        finish_reader(&available, available_peer);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn reader_routes_out_of_order_events_without_releasing_started_requests() {
        let (shared, peer) = shared_with_reader();
        let (first_sender, first_events) = mpsc::channel();
        let (second_sender, second_events) = mpsc::channel();
        shared.pending.lock().unwrap().insert(11, first_sender);
        shared.pending.lock().unwrap().insert(22, second_sender);

        let first_pidfd = File::open("/dev/null").unwrap();
        let second_pidfd = File::open("/dev/null").unwrap();
        send_packet(peer.as_raw_fd(), &started_packet(22, 222), &[second_pidfd.as_raw_fd()]).unwrap();
        send_packet(peer.as_raw_fd(), &started_packet(11, 111), &[first_pidfd.as_raw_fd()]).unwrap();

        assert_eq!(started_event(second_events.recv().unwrap()).0, 222);
        assert_eq!(started_event(first_events.recv().unwrap()).0, 111);
        assert_eq!(shared.pending.lock().unwrap().len(), 2);
        assert_eq!(shared.health.active_children.load(Ordering::Relaxed), 2);

        send_packet(peer.as_raw_fd(), &pool_state_packet(3, 4, 5), &[]).unwrap();

        for request_id in [22, 11] {
            send_packet(peer.as_raw_fd(), &exited_packet(request_id), &[]).unwrap();
        }
        assert!(matches!(second_events.recv().unwrap(), Event::Exited(status) if status.success()));
        assert!(matches!(first_events.recv().unwrap(), Event::Exited(status) if status.success()));
        assert!(shared.pending.lock().unwrap().is_empty());
        assert_eq!(shared.health.active_children.load(Ordering::Relaxed), 0);
        assert_eq!(shared.health.idle_prefork_workers.load(Ordering::Relaxed), 3);
        assert_eq!(shared.health.prefork_refills.load(Ordering::Relaxed), 4);
        assert_eq!(shared.health.prefork_refill_nanoseconds.load(Ordering::Relaxed), 5);
        finish_reader(&shared, peer);
        assert!(shared.lifecycle.lock().unwrap().shutdown);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn reader_routes_every_valid_two_request_interleaving() {
        const START_FIRST: u8 = 0;
        const EXIT_FIRST: u8 = 1;
        const START_SECOND: u8 = 2;
        const EXIT_SECOND: u8 = 3;
        let interleavings = [
            [START_FIRST, EXIT_FIRST, START_SECOND, EXIT_SECOND],
            [START_FIRST, START_SECOND, EXIT_FIRST, EXIT_SECOND],
            [START_FIRST, START_SECOND, EXIT_SECOND, EXIT_FIRST],
            [START_SECOND, EXIT_SECOND, START_FIRST, EXIT_FIRST],
            [START_SECOND, START_FIRST, EXIT_SECOND, EXIT_FIRST],
            [START_SECOND, START_FIRST, EXIT_FIRST, EXIT_SECOND],
        ];

        for interleaving in interleavings {
            let (shared, peer) = shared_with_reader();
            let (first_sender, first_events) = mpsc::channel();
            let (second_sender, second_events) = mpsc::channel();
            shared.pending.lock().unwrap().insert(11, first_sender);
            shared.pending.lock().unwrap().insert(22, second_sender);

            for event in interleaving {
                let (message, pidfd) = match event {
                    START_FIRST => (started_packet(11, 111), Some(File::open("/dev/null").unwrap())),
                    EXIT_FIRST => (exited_packet(11), None),
                    START_SECOND => (started_packet(22, 222), Some(File::open("/dev/null").unwrap())),
                    EXIT_SECOND => (exited_packet(22), None),
                    _ => unreachable!(),
                };
                let descriptors = pidfd.as_ref().map_or_else(Vec::new, |descriptor| vec![descriptor.as_raw_fd()]);
                send_packet(peer.as_raw_fd(), &message, &descriptors).unwrap();
            }

            assert_eq!(started_event(first_events.recv().unwrap()).0, 111);
            assert!(matches!(first_events.recv().unwrap(), Event::Exited(status) if status.success()));
            assert_eq!(started_event(second_events.recv().unwrap()).0, 222);
            assert!(matches!(second_events.recv().unwrap(), Event::Exited(status) if status.success()));
            assert!(shared.pending.lock().unwrap().is_empty());
            assert_eq!(shared.health.active_children.load(Ordering::Relaxed), 0);
            finish_reader(&shared, peer);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn reader_reports_launch_errors_and_disconnects_pending_requests() {
        let (shared, peer) = shared_with_reader();
        let (error_sender, error_events) = mpsc::channel();
        shared.pending.lock().unwrap().insert(7, error_sender);
        send_packet(
            peer.as_raw_fd(),
            &packet(
                7,
                Body::Error(ErrorMessage {
                    stage: 6,
                    error_number: libc::ESRCH as u32,
                }),
                Vec::new(),
            ),
            &[],
        )
        .unwrap();
        let Event::Error(error) = error_events.recv().unwrap() else {
            panic!("launch failure was not routed as an error")
        };
        assert!(error.to_string().contains("stage 6"));
        assert!(error.to_string().contains("os error 3"));

        let (pending_sender, pending_events) = mpsc::channel();
        shared.pending.lock().unwrap().insert(8, pending_sender);
        finish_reader(&shared, peer);
        assert_broken_pipe_event(pending_events.recv().unwrap(), "launch outcome is indeterminate");
        assert!(shared.pending.lock().unwrap().is_empty());
        assert!(shared.lifecycle.lock().unwrap().shutdown);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn reader_disconnect_reports_started_child_and_clears_active_count() {
        let (shared, peer) = shared_with_reader();
        let (sender, events) = mpsc::channel();
        shared.pending.lock().unwrap().insert(7, sender);
        let pidfd = File::open("/dev/null").unwrap();
        send_packet(peer.as_raw_fd(), &started_packet(7, 77), &[pidfd.as_raw_fd()]).unwrap();

        assert_eq!(started_event(events.recv().unwrap()).0, 77);
        assert_eq!(shared.health.active_children.load(Ordering::Relaxed), 1);
        finish_reader(&shared, peer);
        assert_broken_pipe_event(events.recv().unwrap(), "child exit status is unavailable");
        assert_eq!(shared.health.active_children.load(Ordering::Relaxed), 0);
        assert!(shared.pending.lock().unwrap().is_empty());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn reader_disconnects_pending_requests_for_unknown_identifiers() {
        let (shared, peer) = shared_with_reader();
        let (sender, events) = mpsc::channel();
        shared.pending.lock().unwrap().insert(7, sender);
        send_packet(peer.as_raw_fd(), &exited_packet(99), &[]).unwrap();

        shared.reader.lock().unwrap().take().unwrap().join().unwrap();
        assert_broken_pipe_event(events.recv().unwrap(), "launch outcome is indeterminate");
        assert!(shared.pending.lock().unwrap().is_empty());
        assert!(shared.lifecycle.lock().unwrap().shutdown);
        drop(peer);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn malformed_event_disconnects_every_pending_request() {
        let (shared, peer) = shared_with_reader();
        let (sender, events) = mpsc::channel();
        shared.pending.lock().unwrap().insert(9, sender);
        let malformed = zygote_rt::protocol::encode(&started_packet(9, 99)).unwrap();
        // SAFETY: peer is a live socket and malformed remains allocated for
        // the complete send.
        let sent = unsafe { libc::send(peer.as_raw_fd(), malformed.as_ptr().cast(), malformed.len(), libc::MSG_NOSIGNAL) };
        assert_eq!(sent, malformed.len() as isize);

        shared.reader.lock().unwrap().take().unwrap().join().unwrap();
        assert_broken_pipe_event(events.recv().unwrap(), "launch outcome is indeterminate");
        assert!(shared.pending.lock().unwrap().is_empty());
        assert!(shared.lifecycle.lock().unwrap().shutdown);
        drop(peer);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn readiness_requires_the_expected_nonce_and_message_shape() {
        let (controller, peer) = socket_pair().unwrap();
        send_packet(
            peer.as_raw_fd(),
            &packet(0, Body::Ready(Ready { nonce: vec![b'x'; 32] }), Vec::new()),
            &[],
        )
        .unwrap();
        assert_eq!(
            receive_ready(&controller, b"expected").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn readiness_reports_startup_errors() {
        let (controller, peer) = socket_pair().unwrap();
        send_packet(
            peer.as_raw_fd(),
            &packet(
                0,
                Body::Error(ErrorMessage {
                    stage: 102,
                    error_number: libc::EBUSY as u32,
                }),
                Vec::new(),
            ),
            &[],
        )
        .unwrap();
        let error = receive_ready(&controller, b"expected").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ResourceBusy);
        assert!(error.to_string().contains("stage 102"));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn shutdown_waits_for_an_admitted_launch_and_rejects_the_next_one() {
        let (shared, peer) = shared_with_reader();
        let health = Arc::clone(&shared.health);
        let worker = Arc::new(WorkerSlot {
            program: "/unused-test-target".into(),
            prefork_pool: PreforkPoolConfig::default(),
            recovery: WorkerRecoveryPolicy::default(),
            current: RwLock::new(Some(Arc::clone(&shared))),
            stopping: AtomicBool::new(false),
            recovering: AtomicBool::new(false),
            attempts: AtomicUsize::new(0),
            health: Arc::clone(&health),
        });
        let launcher = Launcher {
            program: Arc::new("/unused-test-target".into()),
            inner: Arc::new(LauncherInner::Linux(LinuxPool {
                workers: Box::new([worker]),
                next: AtomicUsize::new(0),
                health: Arc::clone(&health),
            })),
            closed: Arc::new(std::sync::RwLock::new(false)),
            health,
        };
        let launch_handle = {
            let launcher = launcher.clone();
            thread::spawn(move || {
                launcher
                    .command()
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
            })
        };

        let mut buffer = ReceiveBuffer::new();
        let packet = receive_packet(peer.as_raw_fd(), &mut buffer).unwrap().unwrap();
        assert!(matches!(packet.message.body, Some(Body::Launch(_))));
        assert_eq!(packet.message.descriptor_roles.len(), 3);
        let request_id = packet.message.request_id;
        drop(packet);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match shared.lifecycle.try_lock() {
                Ok(lifecycle) => {
                    drop(lifecycle);
                    break;
                }
                Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => thread::yield_now(),
                Err(error) => panic!("lifecycle mutex did not become available: {error:?}"),
            }
        }
        launcher.closed.try_write().unwrap_err();

        let shutdown_handle = {
            let shared = Arc::clone(&shared);
            let closed = Arc::clone(&launcher.closed);
            thread::spawn(move || {
                *closed.write().unwrap() = true;
                shared.shutdown()
            })
        };
        let pidfd = File::open("/dev/null").unwrap();
        send_packet(peer.as_raw_fd(), &started_packet(request_id, 123), &[pidfd.as_raw_fd()]).unwrap();

        let child = launch_handle.join().unwrap().unwrap();
        assert_eq!(child.id(), 123);
        shutdown_handle.join().unwrap().unwrap();
        assert_eq!(launcher.command().spawn().unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        drop(peer);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn event_reader_start_failure_is_reported() {
        let (shared, peer) = shared_with_reader();
        finish_reader(&shared, peer);
        let (socket, peer) = socket_pair().unwrap();
        FAIL_EVENT_READER_START.store(true, Ordering::Relaxed);

        assert_eq!(
            spawn_event_reader(Arc::clone(&shared), socket).unwrap_err().to_string(),
            "injected event reader start failure"
        );
        drop(peer);
    }

    #[test]
    fn unavailable_worker_counts_one_launch_failure() {
        let health = Arc::new(HealthState::default());
        let worker = Arc::new(WorkerSlot {
            program: "/unused-test-target".into(),
            prefork_pool: PreforkPoolConfig::default(),
            recovery: WorkerRecoveryPolicy::default(),
            current: RwLock::new(None),
            stopping: AtomicBool::new(false),
            recovering: AtomicBool::new(false),
            attempts: AtomicUsize::new(0),
            health: Arc::clone(&health),
        });
        let launcher = Launcher {
            program: Arc::new("/unused-test-target".into()),
            inner: Arc::new(LauncherInner::Linux(LinuxPool {
                workers: Box::new([worker]),
                next: AtomicUsize::new(0),
                health: Arc::clone(&health),
            })),
            closed: Arc::new(RwLock::new(false)),
            health,
        };

        let error = launcher.command().spawn().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(launcher.health().launch_failures, 1);
    }

    #[test]
    fn linux_start_defensively_rejects_excess_workers() {
        let error = start(
            OsStr::new("/unused-test-target"),
            NonZeroUsize::new(crate::zygote::MAX_WORKERS + 1).unwrap(),
            PreforkPoolConfig::default(),
            WorkerRecoveryPolicy::default(),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
