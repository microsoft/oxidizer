// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Unix-specific process configuration.
//!
//! Import [`CommandExt`] to configure the logical argument zero, credentials,
//! process group or session, file-creation mask, and resource limits for one
//! [`Command`]. These settings are applied after the child is
//! created and before application code starts. Invalid combinations and
//! operating-system failures are reported by [`Command::spawn`](crate::Command::spawn).
//!
//! ```
//! use zygote_control::Zygote;
//! use zygote_control::unix::{CommandExt as _, Resource, ResourceLimit};
//!
//! # fn configure() -> std::io::Result<()> {
//! let zygote = Zygote::builder("/path/to/integrated-target").spawn()?;
//! let mut command = zygote.command();
//! command
//!     .process_group(0)
//!     .umask(0o027)
//!     .resource_limit(ResourceLimit {
//!         resource: Resource::OpenFiles,
//!         soft: 256,
//!         hard: 256,
//!     });
//! # Ok(())
//! # }
//! ```

use std::ffi::{OsStr, OsString};
#[cfg(not(target_os = "linux"))]
use std::io;
#[cfg(not(target_os = "linux"))]
use std::process;

use crate::Command;

/// A resource controlled by a Unix `setrlimit` call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resource {
    /// Maximum virtual address-space size.
    AddressSpace,
    /// Maximum CPU time in seconds.
    Cpu,
    /// Maximum data-segment size.
    Data,
    /// Maximum created file size.
    FileSize,
    /// Maximum number of open descriptors.
    OpenFiles,
    /// Maximum stack size.
    Stack,
}

impl Resource {
    #[expect(
        trivial_numeric_casts,
        clippy::unnecessary_cast,
        reason = "libc resource constant types differ across Unix targets"
    )]
    pub(super) const fn raw(self) -> u32 {
        match self {
            Self::AddressSpace => libc::RLIMIT_AS as u32,
            Self::Cpu => libc::RLIMIT_CPU as u32,
            Self::Data => libc::RLIMIT_DATA as u32,
            Self::FileSize => libc::RLIMIT_FSIZE as u32,
            Self::OpenFiles => libc::RLIMIT_NOFILE as u32,
            Self::Stack => libc::RLIMIT_STACK as u32,
        }
    }
}

/// Soft and hard limits for one resource.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceLimit {
    /// Resource to limit.
    pub resource: Resource,
    /// Soft limit.
    pub soft: u64,
    /// Hard limit.
    pub hard: u64,
}

/// Unix-specific extensions for [`Command`].
///
/// This trait is sealed and cannot be implemented outside this crate.
///
/// ```compile_fail
/// struct ForeignCommand;
/// impl zygote_control::unix::CommandExt for ForeignCommand {}
/// ```
pub trait CommandExt: crate::sealed::Sealed {
    /// Sets logical `argv[0]`.
    fn arg0<S: AsRef<OsStr>>(&mut self, argument: S) -> &mut Self;

    /// Sets the child user ID.
    fn uid(&mut self, id: u32) -> &mut Self;

    /// Sets the child group ID.
    fn gid(&mut self, id: u32) -> &mut Self;

    /// Sets the child supplementary groups.
    fn groups(&mut self, groups: &[u32]) -> &mut Self;

    /// Sets the child process group.
    fn process_group(&mut self, group: i32) -> &mut Self;

    /// Requests a new session for the child.
    fn new_session(&mut self) -> &mut Self;

    /// Keeps the child in the inherited session.
    fn inherit_session(&mut self) -> &mut Self;

    /// Sets the child file creation mask.
    ///
    /// Values outside `0o000..=0o777` are rejected when the command launches.
    fn umask(&mut self, mask: u32) -> &mut Self;

    /// Sets one child resource limit.
    fn resource_limit(&mut self, limit: ResourceLimit) -> &mut Self;
}

#[derive(Clone, Debug, Default)]
pub(super) struct UnixOptions {
    pub(super) arg0: Option<OsString>,
    pub(super) uid: Option<u32>,
    pub(super) gid: Option<u32>,
    pub(super) groups: Option<Vec<u32>>,
    pub(super) process_group: Option<i32>,
    pub(super) new_session: bool,
    pub(super) umask: Option<u32>,
    pub(super) resource_limits: Vec<ResourceLimit>,
}

impl CommandExt for Command {
    fn arg0<S: AsRef<OsStr>>(&mut self, argument: S) -> &mut Self {
        self.unix.arg0 = Some(argument.as_ref().to_owned());
        self
    }

    fn uid(&mut self, id: u32) -> &mut Self {
        self.unix.uid = Some(id);
        self
    }

    fn gid(&mut self, id: u32) -> &mut Self {
        self.unix.gid = Some(id);
        self
    }

    fn groups(&mut self, groups: &[u32]) -> &mut Self {
        self.unix.groups = Some(groups.to_vec());
        self
    }

    fn process_group(&mut self, group: i32) -> &mut Self {
        self.unix.process_group = Some(group);
        self
    }

    fn new_session(&mut self) -> &mut Self {
        self.unix.new_session = true;
        self
    }

    fn inherit_session(&mut self) -> &mut Self {
        self.unix.new_session = false;
        self
    }

    fn umask(&mut self, mask: u32) -> &mut Self {
        self.unix.umask = Some(mask);
        self
    }

    fn resource_limit(&mut self, limit: ResourceLimit) -> &mut Self {
        if let Some(existing) = self
            .unix
            .resource_limits
            .iter_mut()
            .find(|existing| existing.resource == limit.resource)
        {
            *existing = limit;
        } else {
            self.unix.resource_limits.push(limit);
        }
        self
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) fn apply_native(command: &mut process::Command, options: &UnixOptions) -> io::Result<()> {
    use std::os::unix::process::CommandExt as _;

    if options.new_session && options.process_group.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "new_session and process_group cannot be combined",
        ));
    }
    if let Some(argument) = &options.arg0 {
        command.arg0(argument);
    }

    let options = options.clone();
    unsafe {
        // SAFETY: the closure performs only Unix process-setup calls and does
        // not allocate after the native launcher forks.
        command.pre_exec(move || apply_after_fork(&options));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn apply_after_fork(options: &UnixOptions) -> io::Result<()> {
    // SAFETY: called only in the single-threaded post-fork child; setsid has
    // no pointer arguments and mutates only this process.
    if options.new_session && unsafe { libc::setsid() } < 0 {
        return Err(io::Error::last_os_error());
    }
    if let Some(group) = options.process_group
        // SAFETY: zero names the current child and group was supplied by the caller.
        && unsafe { libc::setpgid(0, group) } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if let Some(mask) = options.umask {
        let mask = native_umask(mask)?;
        // SAFETY: umask accepts every mode_t and mutates only this child.
        unsafe {
            libc::umask(mask);
        }
    }
    for limit in &options.resource_limits {
        let native = libc::rlimit {
            rlim_cur: limit.soft,
            rlim_max: limit.hard,
        };
        // SAFETY: native is initialized and the resource selector is from the
        // closed Resource enum.
        if unsafe { libc::setrlimit(limit.resource.raw() as _, &raw const native) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    if let Some(groups) = &options.groups {
        let count = groups
            .len()
            .try_into()
            .map_err(|_overflow| io::Error::new(io::ErrorKind::InvalidInput, "too many supplementary groups"))?;
        // SAFETY: groups is readable for count entries and remains live.
        if unsafe { libc::setgroups(count, groups.as_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
    // SAFETY: count zero permits a null pointer and clears the current child's groups.
    } else if options.uid.is_some() && unsafe { libc::setgroups(0, std::ptr::null()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if let Some(group) = options.gid
        // SAFETY: setgid has no pointer arguments and mutates only this child.
        && unsafe { libc::setgid(group) } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if let Some(user) = options.uid
        // SAFETY: setuid has no pointer arguments and mutates only this child.
        && unsafe { libc::setuid(user) } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn native_umask(mask: u32) -> io::Result<libc::mode_t> {
    if mask > 0o777 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "umask must contain only file permission bits",
        ));
    }
    mask.try_into()
        .map_err(|_overflow| io::Error::new(io::ErrorKind::InvalidInput, "umask does not fit the platform mode type"))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn setters_replace_scalar_and_resource_state() {
        let mut command = Command::new(crate::zygote::Launcher::for_test("fixture"));
        command
            .arg0("logical")
            .uid(10)
            .uid(11)
            .gid(20)
            .groups(&[30, 31])
            .process_group(40)
            .new_session()
            .inherit_session()
            .umask(0o027)
            .resource_limit(ResourceLimit {
                resource: Resource::OpenFiles,
                soft: 100,
                hard: 200,
            })
            .resource_limit(ResourceLimit {
                resource: Resource::OpenFiles,
                soft: 300,
                hard: 400,
            });

        assert_eq!(command.unix.arg0.as_deref(), Some(OsStr::new("logical")));
        assert_eq!(command.unix.uid, Some(11));
        assert_eq!(command.unix.gid, Some(20));
        assert_eq!(command.unix.groups.as_deref(), Some([30, 31].as_slice()));
        assert_eq!(command.unix.process_group, Some(40));
        assert!(!command.unix.new_session);
        assert_eq!(command.unix.umask, Some(0o027));
        assert_eq!(
            command.unix.resource_limits,
            [ResourceLimit {
                resource: Resource::OpenFiles,
                soft: 300,
                hard: 400,
            }]
        );
    }
}
