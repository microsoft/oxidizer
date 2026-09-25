// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Protobuf messages shared by the controller and target runtime.
//!
//! [`SCHEMA`] is the checked-in authoritative wire
//! contract. Generated definitions are committed here so downstream builds do
//! not require `protoc`.

use std::collections::HashSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use prost::Message;

use self::packet::Body;

/// The authoritative protocol schema.
pub const SCHEMA: &str = include_str!("zygote.proto");

/// Current incompatible protocol major version.
pub const PROTOCOL_MAJOR: u32 = 1;

/// Baseline feature required by every packet.
pub const FEATURE_BASE: u32 = 1;
/// Linux launch-time sandbox specialization.
pub const FEATURE_LINUX_SANDBOX: u32 = 2;

/// Largest accepted `SOCK_SEQPACKET` datagram.
pub const MAX_PACKET_LEN: usize = 1024 * 1024;

/// Largest accepted repeated argv, environment, group, or rlimit collection.
pub const MAX_ITEM_COUNT: usize = 4096;
/// Maximum time allowed for Linux child specialization before the runtime
/// terminates and reaps the ambiguous child.
pub const SPECIALIZATION_TIMEOUT_MILLIS: i32 = 30_000;

/// Largest number of descriptors accepted with one packet.
pub const MAX_FD_COUNT: usize = 64;

/// Reserved descriptor used for initial control-channel inheritance.
pub const CONTROL_FD: i32 = 198;

/// Environment variable containing the inherited control descriptor.
pub const CONTROL_FD_ENV: &str = "ZYGOTE_RT_CONTROL_FD";

/// Environment variable containing the bootstrap nonce.
pub const NONCE_ENV: &str = "ZYGOTE_RT_NONCE";

/// One complete control-channel datagram.
#[derive(Clone, Eq, PartialEq, Message)]
pub struct Packet {
    /// Incompatible protocol major version.
    #[prost(uint32, tag = "1")]
    pub protocol_major: u32,
    /// Controller-assigned request identifier, or zero for lifecycle packets.
    #[prost(uint64, tag = "2")]
    pub request_id: u64,
    /// Features the receiver must understand before processing this packet.
    #[prost(uint32, repeated, packed = "false", tag = "3")]
    pub required_features: Vec<u32>,
    /// Ordered roles for descriptors carried out-of-band through `SCM_RIGHTS`.
    #[prost(enumeration = "DescriptorRole", repeated, packed = "false", tag = "4")]
    pub descriptor_roles: Vec<i32>,
    /// Packet body.
    #[prost(oneof = "packet::Body", tags = "10, 11, 12, 13, 14, 15, 16, 17")]
    pub body: Option<packet::Body>,
}

/// Nested types for [`Packet`].
pub mod packet {
    /// Packet body.
    #[derive(Clone, Eq, PartialEq, prost::Oneof)]
    #[expect(clippy::large_enum_variant, reason = "wire-compatible checked-in Prost representation")]
    pub enum Body {
        /// Target readiness handshake.
        #[prost(message, tag = "10")]
        Ready(super::Ready),
        /// Child launch request.
        #[prost(message, tag = "11")]
        Launch(super::Launch),
        /// Successful child start.
        #[prost(message, tag = "12")]
        Started(super::Started),
        /// Child exit notification.
        #[prost(message, tag = "13")]
        Exited(super::Exited),
        /// Request failure.
        #[prost(message, tag = "14")]
        Error(super::ErrorMessage),
        /// Graceful template shutdown.
        #[prost(message, tag = "15")]
        Shutdown(super::Shutdown),
        /// Adjusts the number of dormant prefork workers.
        #[prost(message, tag = "16")]
        PoolControl(super::PoolControl),
        /// Reports completed prefork-pool state changes.
        #[prost(message, tag = "17")]
        PoolState(super::PoolState),
    }
}

/// Ordered meaning of an out-of-band descriptor.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, prost::Enumeration)]
#[repr(i32)]
pub enum DescriptorRole {
    /// Standard input.
    Stdin = 1,
    /// Standard output.
    Stdout = 2,
    /// Standard error.
    Stderr = 3,
    /// Linux process descriptor.
    Pidfd = 4,
    /// Writable cgroup v2 `cgroup.procs` capability.
    CgroupProcs = 5,
    /// Landlock ruleset capability.
    LandlockRuleset = 6,
    /// Cgroup namespace.
    NamespaceCgroup = 7,
    /// IPC namespace.
    NamespaceIpc = 8,
    /// UTS namespace.
    NamespaceUts = 9,
    /// Network namespace.
    NamespaceNetwork = 10,
    /// Time namespace.
    NamespaceTime = 11,
    /// Mount namespace.
    NamespaceMount = 12,
    /// Closed standard input placeholder.
    ClosedStdin = 13,
    /// Closed standard output placeholder.
    ClosedStdout = 14,
    /// Closed standard error placeholder.
    ClosedStderr = 15,
}

/// Target readiness handshake.
#[derive(Clone, Eq, PartialEq, Message)]
pub struct Ready {
    /// Bootstrap nonce copied from the authenticated inherited environment.
    #[prost(bytes = "vec", tag = "1")]
    pub nonce: Vec<u8>,
}

/// Child launch request.
#[derive(Clone, Eq, PartialEq, Message)]
pub struct Launch {
    /// Argument vector, including logical `argv[0]`.
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub argv: Vec<Vec<u8>>,
    /// Environment entries encoded as `name=value`.
    #[prost(bytes = "vec", repeated, tag = "2")]
    pub environment: Vec<Vec<u8>>,
    /// Child working directory.
    #[prost(bytes = "vec", optional, tag = "3")]
    pub cwd: Option<Vec<u8>>,
    /// Linux child specialization.
    #[prost(message, optional, tag = "4")]
    pub unix: Option<UnixOptions>,
    /// Resource limits to install in the child.
    #[prost(message, repeated, tag = "5")]
    pub rlimits: Vec<Rlimit>,
    /// Linux sandbox specialization.
    #[prost(message, optional, tag = "6")]
    pub sandbox: Option<LinuxSandbox>,
}

/// Linux child specialization options.
#[derive(Clone, Eq, PartialEq, Message)]
pub struct UnixOptions {
    /// Real/effective/saved user identifier.
    #[prost(uint32, optional, tag = "1")]
    pub uid: Option<u32>,
    /// Real/effective/saved group identifier.
    #[prost(uint32, optional, tag = "2")]
    pub gid: Option<u32>,
    /// Optional supplementary group list; an empty list explicitly clears it.
    #[prost(message, optional, tag = "3")]
    pub groups: Option<SupplementaryGroups>,
    /// Process group identifier; zero creates a group led by the child.
    #[prost(sint32, optional, tag = "4")]
    pub process_group: Option<i32>,
    /// Whether to create a new session.
    #[prost(bool, tag = "5")]
    pub new_session: bool,
    /// File-creation mask.
    #[prost(uint32, optional, tag = "6")]
    pub umask: Option<u32>,
}

/// Explicit supplementary group list.
#[derive(Clone, Eq, PartialEq, Message)]
pub struct SupplementaryGroups {
    /// Group identifiers.
    #[prost(uint32, repeated, packed = "false", tag = "1")]
    pub gids: Vec<u32>,
}

/// One resource limit.
#[derive(Clone, Copy, Eq, PartialEq, Message)]
pub struct Rlimit {
    /// Platform `RLIMIT_*` resource identifier.
    #[prost(uint32, tag = "1")]
    pub resource: u32,
    /// Soft limit.
    #[prost(uint64, tag = "2")]
    pub soft: u64,
    /// Hard limit.
    #[prost(uint64, tag = "3")]
    pub hard: u64,
}

/// Linux-only launch-time sandbox state.
#[derive(Clone, Eq, PartialEq, Message)]
pub struct LinuxSandbox {
    /// Irreversible privilege reduction.
    #[prost(message, optional, tag = "1")]
    pub privileges: Option<PrivilegeReduction>,
    /// Classic-BPF seccomp filter.
    #[prost(message, optional, tag = "2")]
    pub seccomp: Option<SeccompPolicy>,
    /// Whether a Landlock ruleset descriptor is present.
    #[prost(bool, tag = "3")]
    pub landlock: bool,
    /// Whether a cgroup.procs descriptor is present.
    #[prost(bool, tag = "4")]
    pub cgroup: bool,
    /// Namespace descriptors in deterministic application order.
    #[prost(enumeration = "NamespaceKind", repeated, packed = "false", tag = "5")]
    pub namespaces: Vec<i32>,
    /// Landlock ABI used to create the supplied ruleset.
    #[prost(uint32, tag = "6")]
    pub landlock_abi: u32,
    /// Exact filesystem rights handled by the supplied ruleset.
    #[prost(uint64, tag = "7")]
    pub landlock_handled_access: u64,
}

/// Reduction-only Linux privilege controls.
#[derive(Clone, Copy, Eq, PartialEq, Message)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each boolean is an independent wire-level security guarantee"
)]
pub struct PrivilegeReduction {
    /// Set `PR_SET_NO_NEW_PRIVS`.
    #[prost(bool, tag = "1")]
    pub no_new_privs: bool,
    /// Set `PR_SET_DUMPABLE` to zero.
    #[prost(bool, tag = "2")]
    pub disable_dumping: bool,
    /// Clear permitted, effective, inheritable, and ambient capabilities.
    #[prost(bool, tag = "3")]
    pub clear_capabilities: bool,
    /// Drop every capability from the bounding set.
    #[prost(bool, tag = "4")]
    pub drop_capability_bounding_set: bool,
    /// Lock securebits against regaining capabilities.
    #[prost(bool, tag = "5")]
    pub lock_securebits: bool,
}

/// A bounded classic-BPF seccomp filter.
#[derive(Clone, Eq, PartialEq, Message)]
pub struct SeccompPolicy {
    /// Linux audit architecture expected by the filter.
    #[prost(uint32, tag = "1")]
    pub audit_architecture: u32,
    /// Validated classic-BPF instructions.
    #[prost(message, repeated, tag = "2")]
    pub instructions: Vec<SeccompInstruction>,
}

/// One classic-BPF instruction.
#[derive(Clone, Copy, Eq, PartialEq, Message)]
pub struct SeccompInstruction {
    /// BPF opcode.
    #[prost(uint32, tag = "1")]
    pub code: u32,
    /// True-branch forward offset.
    #[prost(uint32, tag = "2")]
    pub jump_true: u32,
    /// False-branch forward offset.
    #[prost(uint32, tag = "3")]
    pub jump_false: u32,
    /// Instruction operand.
    #[prost(uint32, tag = "4")]
    pub value: u32,
}

/// Linux namespace type.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, prost::Enumeration)]
#[repr(i32)]
pub enum NamespaceKind {
    /// Cgroup namespace.
    Cgroup = 1,
    /// IPC namespace.
    Ipc = 2,
    /// UTS namespace.
    Uts = 3,
    /// Network namespace.
    Network = 4,
    /// Time namespace.
    Time = 5,
    /// Mount namespace.
    Mount = 6,
}

/// Successful child start.
#[derive(Clone, Copy, Eq, PartialEq, Message)]
pub struct Started {
    /// Child process identifier.
    #[prost(uint32, tag = "1")]
    pub pid: u32,
    /// Whether the launch consumed an already-dormant worker.
    #[prost(bool, tag = "2")]
    pub prefork_hit: bool,
    /// Dormant workers remaining after assignment.
    #[prost(uint32, tag = "3")]
    pub idle_workers: u32,
    /// Cumulative successful dormant-worker refills in this template.
    #[prost(uint64, tag = "4")]
    pub prefork_refills: u64,
    /// Cumulative refill time in nanoseconds in this template.
    #[prost(uint64, tag = "5")]
    pub prefork_refill_nanoseconds: u64,
}

/// Child exit notification.
#[derive(Clone, Copy, Eq, PartialEq, Message)]
pub struct Exited {
    /// Raw `waitpid` status.
    #[prost(uint32, tag = "1")]
    pub wait_status: u32,
}

/// Launch failure.
#[derive(Clone, Copy, Eq, PartialEq, Message)]
pub struct ErrorMessage {
    /// Stable launch-stage identifier.
    #[prost(uint32, tag = "1")]
    pub stage: u32,
    /// Platform error number.
    #[prost(uint32, tag = "2")]
    pub error_number: u32,
}

/// Graceful template shutdown.
#[derive(Clone, Copy, Eq, PartialEq, Message)]
#[expect(clippy::empty_structs_with_brackets, reason = "matches the checked-in Protobuf message shape")]
pub struct Shutdown {}

/// Runtime prefork-pool control.
#[derive(Clone, Copy, Eq, PartialEq, Message)]
pub struct PoolControl {
    /// Dormant workers to retain immediately.
    #[prost(uint32, tag = "1")]
    pub idle_workers: u32,
}

/// Completed runtime prefork-pool state.
#[derive(Clone, Copy, Eq, PartialEq, Message)]
pub struct PoolState {
    /// Dormant workers currently ready for assignment.
    #[prost(uint32, tag = "1")]
    pub idle_workers: u32,
    /// Cumulative successful dormant-worker refills in this template.
    #[prost(uint64, tag = "2")]
    pub prefork_refills: u64,
    /// Cumulative refill time in nanoseconds in this template.
    #[prost(uint64, tag = "3")]
    pub prefork_refill_nanoseconds: u64,
}

/// Constructs a packet with the baseline required feature.
#[must_use]
pub fn packet(request_id: u64, body: packet::Body, descriptor_roles: Vec<DescriptorRole>) -> Packet {
    let mut required_features = vec![FEATURE_BASE];
    if matches!(&body, packet::Body::Launch(launch) if launch.sandbox.is_some()) {
        required_features.push(FEATURE_LINUX_SANDBOX);
    }
    Packet {
        protocol_major: PROTOCOL_MAJOR,
        request_id,
        required_features,
        descriptor_roles: descriptor_roles.into_iter().map(i32::from).collect(),
        body: Some(body),
    }
}

/// Encodes and validates one protobuf datagram.
pub fn encode(packet: &Packet) -> Result<Vec<u8>, ProtocolError> {
    validate(packet)?;
    let encoded = packet.encode_to_vec();
    if encoded.len() > MAX_PACKET_LEN {
        return Err(ProtocolError::PacketTooLarge);
    }
    Ok(encoded)
}

/// Decodes and validates one protobuf datagram.
pub fn decode(bytes: &[u8]) -> Result<Packet, ProtocolError> {
    if bytes.len() > MAX_PACKET_LEN {
        return Err(ProtocolError::PacketTooLarge);
    }
    validate_wire(bytes)?;
    let packet = Packet::decode(bytes).map_err(|_decode_error| ProtocolError::MalformedProtobuf)?;
    validate(&packet)?;
    Ok(packet)
}

/// Validates protocol-level and semantic invariants.
pub fn validate(packet: &Packet) -> Result<(), ProtocolError> {
    if packet.protocol_major != PROTOCOL_MAJOR {
        return Err(ProtocolError::UnsupportedVersion(packet.protocol_major));
    }
    if packet.required_features.is_empty() || !packet.required_features.contains(&FEATURE_BASE) {
        return Err(ProtocolError::MissingRequiredFeature);
    }
    let mut previous = 0;
    let mut has_sandbox_feature = false;
    for &feature in &packet.required_features {
        if !matches!(feature, FEATURE_BASE | FEATURE_LINUX_SANDBOX) {
            return Err(ProtocolError::UnknownRequiredFeature(feature));
        }
        has_sandbox_feature |= feature == FEATURE_LINUX_SANDBOX;
        if feature <= previous {
            return Err(ProtocolError::DuplicateOrUnorderedFeature);
        }
        previous = feature;
    }
    if packet.descriptor_roles.len() > MAX_FD_COUNT {
        return Err(ProtocolError::TooManyDescriptors);
    }
    for &role in &packet.descriptor_roles {
        DescriptorRole::try_from(role).map_err(|_unknown_role| ProtocolError::UnknownDescriptorRole(role))?;
    }

    match packet.body.as_ref().ok_or(ProtocolError::MissingBody)? {
        Body::Ready(ready) => {
            if has_sandbox_feature {
                return Err(ProtocolError::IncompatibleOptions);
            }
            require_lifecycle(packet, 0, &[])?;
            if ready.nonce.len() != 32 {
                return Err(ProtocolError::InvalidNonce);
            }
        }
        Body::Launch(launch) => {
            if packet.request_id == 0 {
                return Err(ProtocolError::InvalidRequestId);
            }
            let expected = launch_descriptor_roles(launch)?;
            require_launch_roles(packet, &expected)?;
            if launch.sandbox.is_some() != has_sandbox_feature {
                return Err(ProtocolError::MissingRequiredFeature);
            }
            validate_launch(launch)?;
        }
        Body::Started(started) => {
            if has_sandbox_feature {
                return Err(ProtocolError::IncompatibleOptions);
            }
            if packet.request_id == 0 || started.pid == 0 {
                return Err(ProtocolError::InvalidRequestId);
            }
            require_roles(packet, &[DescriptorRole::Pidfd])?;
        }
        Body::Exited(_) => {
            if has_sandbox_feature {
                return Err(ProtocolError::IncompatibleOptions);
            }
            if packet.request_id == 0 {
                return Err(ProtocolError::InvalidRequestId);
            }
            require_roles(packet, &[])?;
        }
        Body::Error(_) => {
            if has_sandbox_feature {
                return Err(ProtocolError::IncompatibleOptions);
            }
            require_roles(packet, &[])?;
        }
        Body::Shutdown(_) => {
            if has_sandbox_feature {
                return Err(ProtocolError::IncompatibleOptions);
            }
            require_lifecycle(packet, 0, &[])?;
        }
        Body::PoolControl(control) => {
            if has_sandbox_feature || control.idle_workers > 64 {
                return Err(ProtocolError::IncompatibleOptions);
            }
            require_lifecycle(packet, 0, &[])?;
        }
        Body::PoolState(state) => {
            if has_sandbox_feature || state.idle_workers > 64 {
                return Err(ProtocolError::IncompatibleOptions);
            }
            require_lifecycle(packet, 0, &[])?;
        }
    }
    Ok(())
}

fn require_lifecycle(packet: &Packet, request_id: u64, roles: &[DescriptorRole]) -> Result<(), ProtocolError> {
    if packet.request_id != request_id {
        return Err(ProtocolError::InvalidRequestId);
    }
    require_roles(packet, roles)
}

fn require_roles(packet: &Packet, roles: &[DescriptorRole]) -> Result<(), ProtocolError> {
    if packet.descriptor_roles.len() != roles.len()
        || packet
            .descriptor_roles
            .iter()
            .copied()
            .zip(roles.iter().copied().map(i32::from))
            .any(|(actual, expected)| actual != expected)
    {
        return Err(ProtocolError::InvalidDescriptorRoles);
    }
    Ok(())
}

fn require_launch_roles(packet: &Packet, expected_roles: &[DescriptorRole]) -> Result<(), ProtocolError> {
    if packet.descriptor_roles.len() != expected_roles.len() {
        return Err(ProtocolError::InvalidDescriptorRoles);
    }
    let standard_roles_valid = matches!(
        packet.descriptor_roles[0],
        value if value == DescriptorRole::Stdin as i32 || value == DescriptorRole::ClosedStdin as i32
    ) && matches!(
        packet.descriptor_roles[1],
        value if value == DescriptorRole::Stdout as i32 || value == DescriptorRole::ClosedStdout as i32
    ) && matches!(
        packet.descriptor_roles[2],
        value if value == DescriptorRole::Stderr as i32 || value == DescriptorRole::ClosedStderr as i32
    );
    if !standard_roles_valid
        || packet.descriptor_roles[3..]
            .iter()
            .copied()
            .zip(expected_roles[3..].iter().copied().map(i32::from))
            .any(|(actual, expected)| actual != expected)
    {
        return Err(ProtocolError::InvalidDescriptorRoles);
    }
    Ok(())
}

fn validate_launch(launch: &Launch) -> Result<(), ProtocolError> {
    if launch.argv.is_empty() || launch.argv.len() > MAX_ITEM_COUNT {
        return Err(ProtocolError::InvalidItemCount);
    }
    if launch.environment.len() > MAX_ITEM_COUNT || launch.rlimits.len() > MAX_ITEM_COUNT {
        return Err(ProtocolError::InvalidItemCount);
    }
    for value in launch.argv.iter().chain(launch.cwd.iter()) {
        if value.contains(&0) {
            return Err(ProtocolError::EmbeddedNul);
        }
    }
    let mut environment_names = HashSet::with_capacity(launch.environment.len());
    for entry in &launch.environment {
        let equals = entry.iter().position(|byte| *byte == b'=');
        if entry.contains(&0) || equals.is_none() || equals == Some(0) {
            return Err(ProtocolError::InvalidEnvironment);
        }
        if !environment_names.insert(&entry[..equals.unwrap_or_default()]) {
            return Err(ProtocolError::DuplicateEnvironment);
        }
    }
    if let Some(unix) = &launch.unix {
        if unix.new_session && unix.process_group.is_some() {
            return Err(ProtocolError::IncompatibleOptions);
        }
        if unix.umask.is_some_and(|mask| mask > 0o777) {
            return Err(ProtocolError::InvalidUmask);
        }
        if unix.groups.as_ref().is_some_and(|groups| groups.gids.len() > MAX_ITEM_COUNT) {
            return Err(ProtocolError::InvalidItemCount);
        }
    }
    let mut resources = HashSet::with_capacity(launch.rlimits.len());
    for limit in &launch.rlimits {
        if !matches!(limit.resource, 0 | 1 | 2 | 3 | 7 | 9) || limit.soft > limit.hard {
            return Err(ProtocolError::InvalidResourceLimit);
        }
        if !resources.insert(limit.resource) {
            return Err(ProtocolError::DuplicateResourceLimit);
        }
    }
    if let Some(sandbox) = &launch.sandbox {
        validate_linux_sandbox(sandbox)?;
    }
    Ok(())
}

fn launch_descriptor_roles(launch: &Launch) -> Result<Vec<DescriptorRole>, ProtocolError> {
    let mut roles = vec![DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr];
    let Some(sandbox) = &launch.sandbox else {
        return Ok(roles);
    };
    if sandbox.cgroup {
        roles.push(DescriptorRole::CgroupProcs);
    }
    if sandbox.landlock {
        roles.push(DescriptorRole::LandlockRuleset);
    }
    for &kind in &sandbox.namespaces {
        roles.push(
            match NamespaceKind::try_from(kind).map_err(|_unknown_kind| ProtocolError::InvalidNamespace)? {
                NamespaceKind::Cgroup => DescriptorRole::NamespaceCgroup,
                NamespaceKind::Ipc => DescriptorRole::NamespaceIpc,
                NamespaceKind::Uts => DescriptorRole::NamespaceUts,
                NamespaceKind::Network => DescriptorRole::NamespaceNetwork,
                NamespaceKind::Time => DescriptorRole::NamespaceTime,
                NamespaceKind::Mount => DescriptorRole::NamespaceMount,
            },
        );
    }
    Ok(roles)
}

fn validate_linux_sandbox(sandbox: &LinuxSandbox) -> Result<(), ProtocolError> {
    if sandbox.privileges.as_ref().is_some_and(|policy| {
        !policy.no_new_privs
            && !policy.disable_dumping
            && !policy.clear_capabilities
            && !policy.drop_capability_bounding_set
            && !policy.lock_securebits
    }) {
        return Err(ProtocolError::InvalidPrivilegeReduction);
    }
    if sandbox.landlock && !sandbox.privileges.as_ref().is_some_and(|policy| policy.no_new_privs) {
        return Err(ProtocolError::IncompatibleOptions);
    }
    if sandbox.landlock != (sandbox.landlock_abi != 0) || sandbox.landlock != (sandbox.landlock_handled_access != 0) {
        return Err(ProtocolError::InvalidLandlock);
    }
    if let Some(seccomp) = &sandbox.seccomp {
        if !sandbox.privileges.as_ref().is_some_and(|policy| policy.no_new_privs) {
            return Err(ProtocolError::IncompatibleOptions);
        }
        validate_seccomp(seccomp)?;
    }
    let mut previous = 0;
    for &kind in &sandbox.namespaces {
        NamespaceKind::try_from(kind).map_err(|_unknown_kind| ProtocolError::InvalidNamespace)?;
        if kind <= previous {
            return Err(ProtocolError::DuplicateOrUnorderedNamespace);
        }
        previous = kind;
    }
    Ok(())
}

fn validate_seccomp(policy: &SeccompPolicy) -> Result<(), ProtocolError> {
    const BPF_CLASS_MASK: u32 = 0x07;
    const BPF_JMP: u32 = 0x05;
    const BPF_JA: u32 = 0x05;
    const BPF_RET: u32 = 0x06;
    const SECCOMP_RET_ACTION_FULL: u32 = 0xffff_0000;
    const SECCOMP_RET_USER_NOTIF: u32 = 0x7fc0_0000;
    if policy.audit_architecture == 0 || policy.instructions.len() < 4 || policy.instructions.len() > MAX_ITEM_COUNT {
        return Err(ProtocolError::InvalidSeccomp);
    }
    let [load_arch, compare_arch, reject_arch, ..] = policy.instructions.as_slice() else {
        return Err(ProtocolError::InvalidSeccomp);
    };
    if (load_arch.code, load_arch.jump_true, load_arch.jump_false, load_arch.value) != (0x20, 0, 0, 4)
        || (
            compare_arch.code,
            compare_arch.jump_true,
            compare_arch.jump_false,
            compare_arch.value,
        ) != (0x15, 1, 0, policy.audit_architecture)
        || (reject_arch.code, reject_arch.jump_true, reject_arch.jump_false, reject_arch.value) != (0x06, 0, 0, 0x8000_0000)
    {
        return Err(ProtocolError::InvalidSeccomp);
    }
    for (index, instruction) in policy.instructions.iter().enumerate() {
        if instruction.code > u32::from(u16::MAX)
            || instruction.jump_true > u32::from(u8::MAX)
            || instruction.jump_false > u32::from(u8::MAX)
        {
            return Err(ProtocolError::InvalidSeccomp);
        }
        if instruction.code & BPF_CLASS_MASK == BPF_JMP {
            let remaining = policy.instructions.len() - index - 1;
            if instruction.code == BPF_JA {
                if usize::try_from(instruction.value).map_or(true, |offset| offset >= remaining) {
                    return Err(ProtocolError::InvalidSeccomp);
                }
            } else if instruction.jump_true as usize >= remaining || instruction.jump_false as usize >= remaining {
                return Err(ProtocolError::InvalidSeccomp);
            }
        }
        if instruction.code & BPF_CLASS_MASK == BPF_RET
            && (instruction.code != BPF_RET || instruction.value & SECCOMP_RET_ACTION_FULL == SECCOMP_RET_USER_NOTIF)
        {
            return Err(ProtocolError::InvalidSeccomp);
        }
    }
    if policy.instructions.last().is_none_or(|instruction| instruction.code != BPF_RET) {
        return Err(ProtocolError::InvalidSeccomp);
    }
    if !seccomp_allows_setup_operations(policy) {
        return Err(ProtocolError::InvalidSeccomp);
    }
    Ok(())
}

fn seccomp_allows_setup_operations(policy: &SeccompPolicy) -> bool {
    let (write, close) = match policy.audit_architecture {
        0xc000_003e => (1, 3),
        0xc000_00b7 => (64, 57),
        _ => return false,
    };
    seccomp_allows_exact_input(policy, write, [Some(3), None, Some(8), None, None, None])
        && seccomp_allows_exact_input(policy, close, [Some(3), None, None, None, None, None])
}

fn seccomp_allows_exact_input(policy: &SeccompPolicy, syscall: u32, arguments: [Option<u64>; 6]) -> bool {
    const BPF_RET_K: u32 = 0x06;
    const BPF_RET_A: u32 = 0x16;
    const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
    let mut data = [None; 16];
    data[0] = Some(syscall);
    data[1] = Some(policy.audit_architecture);
    for (index, argument) in arguments.into_iter().enumerate() {
        if let Some(argument) = argument {
            data[4 + index * 2] =
                Some(u32::try_from(argument & u64::from(u32::MAX)).expect("mask guarantees the syscall argument's low word fits u32"));
            data[5 + index * 2] = Some(u32::try_from(argument >> u32::BITS).expect("a u64 syscall argument has exactly two u32 words"));
        }
    }
    let mut accumulator = 0u32;
    let mut index = 0usize;
    while let Some(instruction) = policy.instructions.get(index) {
        let jump_true = usize::try_from(instruction.jump_true).expect("seccomp jump width was validated above");
        let jump_false = usize::try_from(instruction.jump_false).expect("seccomp jump width was validated above");
        match instruction.code {
            0x00 => accumulator = instruction.value,
            0x20 => {
                let Ok(word) = usize::try_from(instruction.value / 4) else {
                    return false;
                };
                if instruction.value % 4 != 0 {
                    return false;
                }
                let Some(Some(value)) = data.get(word) else {
                    return false;
                };
                accumulator = *value;
            }
            0x05 => {
                let Ok(offset) = usize::try_from(instruction.value) else {
                    return false;
                };
                index = match index.checked_add(offset + 1) {
                    Some(next) => next,
                    None => return false,
                };
                continue;
            }
            0x15 => {
                index += if accumulator == instruction.value {
                    jump_true + 1
                } else {
                    jump_false + 1
                };
                continue;
            }
            0x25 => {
                index += if accumulator > instruction.value {
                    jump_true + 1
                } else {
                    jump_false + 1
                };
                continue;
            }
            0x35 => {
                index += if accumulator >= instruction.value {
                    jump_true + 1
                } else {
                    jump_false + 1
                };
                continue;
            }
            0x45 => {
                index += if accumulator & instruction.value != 0 {
                    jump_true + 1
                } else {
                    jump_false + 1
                };
                continue;
            }
            BPF_RET_K => return instruction.value == SECCOMP_RET_ALLOW,
            BPF_RET_A => return accumulator == SECCOMP_RET_ALLOW,
            _ => return false,
        }
        index += 1;
    }
    false
}

fn validate_wire(bytes: &[u8]) -> Result<(), ProtocolError> {
    let mut cursor = WireCursor::new(bytes);
    let mut singular = 0u32;
    let mut body_seen = false;
    let mut repeated_counts = [0usize; 5];
    while !cursor.is_empty() {
        let (field, wire) = cursor.key()?;
        match field {
            1 | 2 => {
                reject_duplicate(&mut singular, field)?;
                require_wire(wire, 0)?;
                let value = cursor.varint()?;
                if field == 1 && value > u64::from(u32::MAX) {
                    return Err(ProtocolError::MalformedProtobuf);
                }
            }
            3 | 4 => {
                let index = usize::try_from(field).map_err(|_overflow| ProtocolError::MalformedProtobuf)?;
                let limit = if field == 4 { MAX_FD_COUNT } else { MAX_ITEM_COUNT };
                count_repeated(&mut repeated_counts[index], limit, field == 4)?;
                require_wire(wire, 0)?;
                if cursor.varint()? > u64::from(u32::MAX) {
                    return Err(ProtocolError::MalformedProtobuf);
                }
            }
            10..=17 => {
                if body_seen {
                    return Err(ProtocolError::DuplicateField(field));
                }
                body_seen = true;
                require_wire(wire, 2)?;
                validate_nested(field, cursor.bytes()?)?;
            }
            _ => cursor.skip(wire)?,
        }
    }
    Ok(())
}

fn validate_nested(parent: u32, bytes: &[u8]) -> Result<(), ProtocolError> {
    let mut cursor = WireCursor::new(bytes);
    let mut singular = 0u32;
    let mut repeated_counts = [0usize; 7];
    while !cursor.is_empty() {
        let (field, wire) = cursor.key()?;
        let repeated = matches!((parent, field), (11, 1 | 2 | 5) | (103, 1) | (106, 5) | (202, 2));
        if repeated {
            let index = usize::try_from(field).map_err(|_overflow| ProtocolError::MalformedProtobuf)?;
            count_repeated(&mut repeated_counts[index], MAX_ITEM_COUNT, false)?;
            let expected_wire = if matches!((parent, field), (103, 1) | (106, 5)) { 0 } else { 2 };
            require_wire(wire, expected_wire)?;
        } else if is_known_nested_field(parent, field) {
            reject_duplicate(&mut singular, field)?;
        }
        match wire {
            0 => {
                let value = cursor.varint()?;
                if is_32_bit_field(parent, field) && value > u64::from(u32::MAX) {
                    return Err(ProtocolError::MalformedProtobuf);
                }
                if is_boolean_field(parent, field) && value > 1 {
                    return Err(ProtocolError::MalformedProtobuf);
                }
                if parent == 203
                    && match field {
                        1 => value > u64::from(u16::MAX),
                        2 | 3 => value > u64::from(u8::MAX),
                        4 => value > u64::from(u32::MAX),
                        _ => false,
                    }
                {
                    return Err(ProtocolError::InvalidSeccomp);
                }
            }
            2 => {
                let nested = cursor.bytes()?;
                let child = match (parent, field) {
                    (11, 4) => Some(104),
                    (11, 5) => Some(105),
                    (11, 6) => Some(106),
                    (104, 3) => Some(103),
                    (106, 1) => Some(201),
                    (106, 2) => Some(202),
                    (202, 2) => Some(203),
                    _ => None,
                };
                if let Some(child) = child {
                    validate_nested(child, nested)?;
                }
            }
            _ => cursor.skip(wire)?,
        }
    }
    Ok(())
}

const fn is_known_nested_field(parent: u32, field: u32) -> bool {
    match parent {
        10 | 12 | 13 | 16 | 103 => field == 1,
        11 | 104 => matches!(field, 1..=6),
        14 | 202 => matches!(field, 1 | 2),
        17 | 105 => matches!(field, 1..=3),
        106 => matches!(field, 1..=7),
        201 => matches!(field, 1..=5),
        203 => matches!(field, 1..=4),
        _ => false,
    }
}

fn count_repeated(count: &mut usize, limit: usize, descriptors: bool) -> Result<(), ProtocolError> {
    *count = count.checked_add(1).ok_or(ProtocolError::InvalidItemCount)?;
    if *count <= limit {
        Ok(())
    } else if descriptors {
        Err(ProtocolError::TooManyDescriptors)
    } else {
        Err(ProtocolError::InvalidItemCount)
    }
}

const fn is_32_bit_field(parent: u32, field: u32) -> bool {
    matches!(
        (parent, field),
        (12 | 13 | 16 | 17 | 103 | 105 | 202, 1) | (14, 1 | 2) | (104, 1 | 2 | 4 | 6) | (106, 5 | 6) | (203, 1..=4)
    )
}

const fn is_boolean_field(parent: u32, field: u32) -> bool {
    matches!((parent, field), (104, 5) | (106, 3 | 4) | (201, 1..=5))
}

fn reject_duplicate(seen: &mut u32, field: u32) -> Result<(), ProtocolError> {
    if field >= 32 {
        return Ok(());
    }
    let bit = 1u32 << field;
    if *seen & bit != 0 {
        return Err(ProtocolError::DuplicateField(field));
    }
    *seen |= bit;
    Ok(())
}

fn require_wire(actual: u8, expected: u8) -> Result<(), ProtocolError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ProtocolError::MalformedProtobuf)
    }
}

struct WireCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> WireCursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn is_empty(&self) -> bool {
        self.offset == self.bytes.len()
    }

    fn key(&mut self) -> Result<(u32, u8), ProtocolError> {
        let key = self.varint()?;
        let field = u32::try_from(key >> 3).map_err(|_overflow| ProtocolError::MalformedProtobuf)?;
        if field == 0 {
            return Err(ProtocolError::MalformedProtobuf);
        }
        Ok((field, (key & 7) as u8))
    }

    fn varint(&mut self) -> Result<u64, ProtocolError> {
        let mut value = 0u64;
        for shift in (0..70).step_by(7) {
            let byte = *self.bytes.get(self.offset).ok_or(ProtocolError::MalformedProtobuf)?;
            self.offset += 1;
            if shift == 63 && byte > 1 {
                return Err(ProtocolError::MalformedProtobuf);
            }
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(ProtocolError::MalformedProtobuf)
    }

    fn bytes(&mut self) -> Result<&'a [u8], ProtocolError> {
        let length = usize::try_from(self.varint()?).map_err(|_overflow| ProtocolError::MalformedProtobuf)?;
        let end = self.offset.checked_add(length).ok_or(ProtocolError::MalformedProtobuf)?;
        let value = self.bytes.get(self.offset..end).ok_or(ProtocolError::MalformedProtobuf)?;
        self.offset = end;
        Ok(value)
    }

    fn skip(&mut self, wire: u8) -> Result<(), ProtocolError> {
        match wire {
            0 => {
                self.varint()?;
            }
            1 => self.offset = self.offset.checked_add(8).ok_or(ProtocolError::MalformedProtobuf)?,
            2 => {
                self.bytes()?;
            }
            5 => self.offset = self.offset.checked_add(4).ok_or(ProtocolError::MalformedProtobuf)?,
            _ => return Err(ProtocolError::MalformedProtobuf),
        }
        if self.offset > self.bytes.len() {
            return Err(ProtocolError::MalformedProtobuf);
        }
        Ok(())
    }
}

/// A malformed, incompatible, or semantically ambiguous packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    /// The datagram is larger than the protocol limit.
    PacketTooLarge,
    /// The bytes are not a valid supported Protobuf encoding.
    MalformedProtobuf,
    /// A singular field or oneof body appears more than once.
    DuplicateField(u32),
    /// The peer uses an unsupported major version.
    UnsupportedVersion(u32),
    /// The baseline feature identifier is absent.
    MissingRequiredFeature,
    /// The receiver does not understand a required feature.
    UnknownRequiredFeature(u32),
    /// Required features are duplicated or not in canonical order.
    DuplicateOrUnorderedFeature,
    /// A packet has no body.
    MissingBody,
    /// A lifecycle or request packet has an invalid request identifier.
    InvalidRequestId,
    /// A descriptor role is unknown.
    UnknownDescriptorRole(i32),
    /// Descriptor roles do not exactly match the body.
    InvalidDescriptorRoles,
    /// Too many descriptors were declared.
    TooManyDescriptors,
    /// A repeated launch collection violates its bound.
    InvalidItemCount,
    /// A Unix byte string contains NUL.
    EmbeddedNul,
    /// An environment entry is not an unambiguous `name=value`.
    InvalidEnvironment,
    /// An environment name appears more than once.
    DuplicateEnvironment,
    /// Mutually exclusive Unix options were combined.
    IncompatibleOptions,
    /// A privilege policy does not request any reduction.
    InvalidPrivilegeReduction,
    /// A file-creation mask contains non-permission bits.
    InvalidUmask,
    /// The same resource limit appears more than once.
    DuplicateResourceLimit,
    /// A resource limit identifier or soft/hard relationship is invalid.
    InvalidResourceLimit,
    /// A seccomp program is malformed, unbounded, or requests `USER_NOTIF`.
    InvalidSeccomp,
    /// Landlock metadata is absent or inconsistent with its descriptor.
    InvalidLandlock,
    /// A namespace kind is unknown.
    InvalidNamespace,
    /// Namespace descriptors are duplicated or out of deterministic order.
    DuplicateOrUnorderedNamespace,
    /// A readiness nonce has the wrong length.
    InvalidNonce,
}

impl Display for ProtocolError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::PacketTooLarge => f.write_str("zygote protobuf packet exceeds its size limit"),
            Self::MalformedProtobuf => f.write_str("malformed zygote protobuf packet"),
            Self::DuplicateField(field) => write!(f, "duplicate protobuf field {field}"),
            Self::UnsupportedVersion(version) => write!(f, "unsupported zygote protocol major version {version}"),
            Self::MissingRequiredFeature => f.write_str("zygote packet omits the baseline required feature"),
            Self::UnknownRequiredFeature(feature) => write!(f, "unknown required zygote feature {feature}"),
            Self::DuplicateOrUnorderedFeature => f.write_str("zygote required features are duplicated or unordered"),
            Self::MissingBody => f.write_str("zygote packet has no body"),
            Self::InvalidRequestId => f.write_str("zygote packet has an invalid request identifier"),
            Self::UnknownDescriptorRole(role) => write!(f, "unknown zygote descriptor role {role}"),
            Self::InvalidDescriptorRoles => f.write_str("zygote descriptor roles do not match the packet body"),
            Self::TooManyDescriptors => f.write_str("zygote packet carries too many descriptors"),
            Self::InvalidItemCount => f.write_str("zygote launch item count is invalid"),
            Self::EmbeddedNul => f.write_str("zygote launch value contains NUL"),
            Self::InvalidEnvironment => f.write_str("zygote environment entry is invalid"),
            Self::DuplicateEnvironment => f.write_str("zygote environment name is duplicated"),
            Self::IncompatibleOptions => f.write_str("zygote Unix options are incompatible"),
            Self::InvalidPrivilegeReduction => f.write_str("zygote privilege policy does not request any reduction"),
            Self::InvalidUmask => f.write_str("zygote file-creation mask is invalid"),
            Self::DuplicateResourceLimit => f.write_str("zygote resource limit is duplicated"),
            Self::InvalidResourceLimit => f.write_str("zygote resource limit is invalid"),
            Self::InvalidSeccomp => f.write_str("zygote seccomp policy is invalid"),
            Self::InvalidLandlock => f.write_str("zygote Landlock policy is invalid"),
            Self::InvalidNamespace => f.write_str("zygote namespace kind is invalid"),
            Self::DuplicateOrUnorderedNamespace => f.write_str("zygote namespace kinds are duplicated or unordered"),
            Self::InvalidNonce => f.write_str("zygote readiness nonce is invalid"),
        }
    }
}

impl Error for ProtocolError {}

#[cfg(any(test, feature = "private-test-util"))]
#[doc(hidden)]
pub mod test_support {
    #![allow(missing_docs, reason = "feature-gated private test support")]

    use prost::Message;

    use super::packet::Body;
    use super::{DescriptorRole, Launch, LinuxSandbox, MAX_FD_COUNT, MAX_ITEM_COUNT, Packet, PrivilegeReduction, Rlimit, decode, packet};

    /// Environment marker used only by the specialization-timeout fixture.
    pub const SPECIALIZATION_STALL_ENV: &str = "ZYGOTE_RT_TEST_STALL_SPECIALIZATION";

    /// One shared differential-decoder input and its expected result.
    #[derive(Debug)]
    pub struct DecoderCase {
        pub name: String,
        pub bytes: Vec<u8>,
        pub accepted: bool,
    }

    fn launch(environment: Vec<Vec<u8>>) -> Packet {
        packet(
            7,
            Body::Launch(Launch {
                argv: vec![b"target".to_vec(), b"argument".to_vec()],
                environment,
                cwd: Some(b"/work".to_vec()),
                unix: None,
                rlimits: Vec::new(),
                sandbox: None,
            }),
            vec![DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr],
        )
    }

    #[expect(clippy::needless_pass_by_value, reason = "call sites construct one-shot corpus packets inline")]
    fn case(name: impl Into<String>, packet: Packet, accepted: bool) -> DecoderCase {
        let bytes = packet.encode_to_vec();
        DecoderCase {
            name: name.into(),
            accepted,
            bytes,
        }
    }

    fn append_varint(bytes: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 {
            let chunk = u8::try_from(value & 0x7f).expect("mask limits the value to seven bits");
            bytes.push(chunk | 0x80);
            value >>= 7;
        }
        bytes.push(u8::try_from(value).expect("loop leaves a value below 128"));
    }

    fn append_varint_field(bytes: &mut Vec<u8>, field: u32, value: u64) {
        append_varint(bytes, u64::from(field) << 3);
        append_varint(bytes, value);
    }

    fn append_bytes_field(bytes: &mut Vec<u8>, field: u32, value: &[u8]) {
        append_varint(bytes, (u64::from(field) << 3) | 2);
        append_varint(bytes, value.len() as u64);
        bytes.extend_from_slice(value);
    }

    fn seccomp_value_overflow_packet() -> Vec<u8> {
        fn instruction(code: u64, jump_true: u64, value: u64) -> Vec<u8> {
            let mut bytes = Vec::new();
            append_varint_field(&mut bytes, 1, code);
            if jump_true != 0 {
                append_varint_field(&mut bytes, 2, jump_true);
            }
            append_varint_field(&mut bytes, 4, value);
            bytes
        }

        let architecture = 0xc000_003e;
        let mut seccomp = Vec::new();
        append_varint_field(&mut seccomp, 1, architecture);
        for instruction in [
            instruction(0x20, 0, 4),
            instruction(0x15, 1, architecture),
            instruction(0x06, 0, 0x8000_0000),
            instruction(0x06, 0, u64::from(u32::MAX) + 1),
        ] {
            append_bytes_field(&mut seccomp, 2, &instruction);
        }

        let mut privileges = Vec::new();
        append_varint_field(&mut privileges, 1, 1);
        let mut sandbox = Vec::new();
        append_bytes_field(&mut sandbox, 1, &privileges);
        append_bytes_field(&mut sandbox, 2, &seccomp);
        let mut launch = Vec::new();
        append_bytes_field(&mut launch, 1, b"target");
        append_bytes_field(&mut launch, 6, &sandbox);
        let mut packet = Vec::new();
        append_varint_field(&mut packet, 1, 1);
        append_varint_field(&mut packet, 2, 7);
        append_varint_field(&mut packet, 3, u64::from(super::FEATURE_BASE));
        append_varint_field(&mut packet, 3, u64::from(super::FEATURE_LINUX_SANDBOX));
        for role in [DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr] {
            append_varint_field(&mut packet, 4, u64::from(role as u32));
        }
        append_bytes_field(&mut packet, 11, &launch);
        packet
    }

    fn duplicate_seccomp_architecture_packet() -> Vec<u8> {
        fn instruction(code: u64, jump_true: u64, value: u64) -> Vec<u8> {
            let mut bytes = Vec::new();
            append_varint_field(&mut bytes, 1, code);
            if jump_true != 0 {
                append_varint_field(&mut bytes, 2, jump_true);
            }
            append_varint_field(&mut bytes, 4, value);
            bytes
        }

        let architecture = 0xc000_003e;
        let mut seccomp = Vec::new();
        append_varint_field(&mut seccomp, 1, architecture);
        append_varint_field(&mut seccomp, 1, architecture);
        for instruction in [
            instruction(0x20, 0, 4),
            instruction(0x15, 1, architecture),
            instruction(0x06, 0, 0x8000_0000),
            instruction(0x06, 0, 0x7fff_0000),
        ] {
            append_bytes_field(&mut seccomp, 2, &instruction);
        }

        let mut privileges = Vec::new();
        append_varint_field(&mut privileges, 1, 1);
        let mut sandbox = Vec::new();
        append_bytes_field(&mut sandbox, 1, &privileges);
        append_bytes_field(&mut sandbox, 2, &seccomp);
        let mut launch = Vec::new();
        append_bytes_field(&mut launch, 1, b"target");
        append_bytes_field(&mut launch, 6, &sandbox);
        let mut packet = Vec::new();
        append_varint_field(&mut packet, 1, 1);
        append_varint_field(&mut packet, 2, 7);
        append_varint_field(&mut packet, 3, u64::from(super::FEATURE_BASE));
        append_varint_field(&mut packet, 3, u64::from(super::FEATURE_LINUX_SANDBOX));
        for role in [DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr] {
            append_varint_field(&mut packet, 4, u64::from(role as u32));
        }
        append_bytes_field(&mut packet, 11, &launch);
        packet
    }

    fn started_pid_overflow_packet() -> Vec<u8> {
        let mut started = Vec::new();
        append_varint_field(&mut started, 1, u64::from(u32::MAX) + 1);
        let mut packet = Vec::new();
        append_varint_field(&mut packet, 1, 1);
        append_varint_field(&mut packet, 2, 7);
        append_varint_field(&mut packet, 3, u64::from(super::FEATURE_BASE));
        append_varint_field(&mut packet, 4, u64::from(DescriptorRole::Pidfd as u32));
        append_bytes_field(&mut packet, 12, &started);
        packet
    }

    fn non_boolean_unix_option_packet() -> Vec<u8> {
        let mut unix = Vec::new();
        append_varint_field(&mut unix, 5, 2);
        let mut launch = Vec::new();
        append_bytes_field(&mut launch, 1, b"target");
        append_bytes_field(&mut launch, 4, &unix);
        let mut packet = Vec::new();
        append_varint_field(&mut packet, 1, 1);
        append_varint_field(&mut packet, 2, 7);
        append_varint_field(&mut packet, 3, u64::from(super::FEATURE_BASE));
        for role in [DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr] {
            append_varint_field(&mut packet, 4, u64::from(role as u32));
        }
        append_bytes_field(&mut packet, 11, &launch);
        packet
    }

    fn unknown_nested_unix_field_packet() -> Vec<u8> {
        let mut unix = Vec::new();
        append_bytes_field(&mut unix, 99, b"future");
        append_bytes_field(&mut unix, 99, b"future-again");
        let mut launch = Vec::new();
        append_bytes_field(&mut launch, 1, b"target");
        append_bytes_field(&mut launch, 4, &unix);
        let mut packet = Vec::new();
        append_varint_field(&mut packet, 1, 1);
        append_varint_field(&mut packet, 2, 7);
        append_varint_field(&mut packet, 3, u64::from(super::FEATURE_BASE));
        for role in [DescriptorRole::Stdin, DescriptorRole::Stdout, DescriptorRole::Stderr] {
            append_varint_field(&mut packet, 4, u64::from(role as u32));
        }
        append_bytes_field(&mut packet, 11, &launch);
        packet
    }

    fn repeated_field_overflow_packet(field: u32, count: usize) -> Vec<u8> {
        let mut packet = Vec::new();
        append_varint_field(&mut packet, 1, 1);
        append_varint_field(&mut packet, 2, 7);
        for _ in 0..count {
            append_varint_field(&mut packet, field, 1);
        }
        packet
    }

    fn empty_argument_overflow_packet() -> Vec<u8> {
        let mut launch = Vec::new();
        for _ in 0..=MAX_ITEM_COUNT {
            append_bytes_field(&mut launch, 1, b"");
        }
        let mut packet = Vec::new();
        append_varint_field(&mut packet, 1, 1);
        append_varint_field(&mut packet, 2, 7);
        append_varint_field(&mut packet, 3, u64::from(super::FEATURE_BASE));
        append_bytes_field(&mut packet, 11, &launch);
        packet
    }

    /// Returns fixed cases with independently specified expected verdicts.
    #[must_use]
    #[expect(clippy::too_many_lines, reason = "the fixed malformed-packet corpus is kept in one catalog")]
    pub fn fixed_decoder_corpus() -> Vec<DecoderCase> {
        let valid = launch(vec![b"A=1".to_vec(), b"AB=2".to_vec()]);
        let valid_bytes = valid.encode_to_vec();
        let mut cases = vec![
            case("valid", valid, true),
            case("empty-value", launch(vec![b"A=".to_vec()]), true),
            case("bytewise-case-distinct", launch(vec![b"A=1".to_vec(), b"a=2".to_vec()]), true),
            case(
                "embedded-nul-argument",
                {
                    let mut packet = launch(Vec::new());
                    let Some(Body::Launch(launch)) = &mut packet.body else {
                        unreachable!()
                    };
                    launch.argv.push(b"bad\0argument".to_vec());
                    packet
                },
                false,
            ),
            case("embedded-nul-environment", launch(vec![b"A=bad\0value".to_vec()]), false),
            case("embedded-nul-name", launch(vec![b"A\0B=value".to_vec()]), false),
            case("missing-equals", launch(vec![b"NAME".to_vec()]), false),
            case("empty-name", launch(vec![b"=value".to_vec()]), false),
            case("duplicate-name", launch(vec![b"NAME=one".to_vec(), b"NAME=two".to_vec()]), false),
            case(
                "closed-standard-streams",
                {
                    let mut packet = launch(Vec::new());
                    packet.descriptor_roles = vec![
                        DescriptorRole::ClosedStdin as i32,
                        DescriptorRole::ClosedStdout as i32,
                        DescriptorRole::ClosedStderr as i32,
                    ];
                    packet
                },
                true,
            ),
            case(
                "duplicate-empty-value-name",
                launch(vec![b"NAME=".to_vec(), b"NAME=value".to_vec()]),
                false,
            ),
            case(
                "wrong-role-order",
                {
                    let mut packet = launch(Vec::new());
                    packet.descriptor_roles.swap(0, 1);
                    packet
                },
                false,
            ),
            case(
                "duplicate-role",
                {
                    let mut packet = launch(Vec::new());
                    packet.descriptor_roles[1] = packet.descriptor_roles[0];
                    packet
                },
                false,
            ),
            case(
                "missing-role",
                {
                    let mut packet = launch(Vec::new());
                    packet.descriptor_roles.pop();
                    packet
                },
                false,
            ),
            case(
                "extra-role-without-sandbox",
                {
                    let mut packet = launch(Vec::new());
                    packet.descriptor_roles.push(DescriptorRole::CgroupProcs as i32);
                    packet
                },
                false,
            ),
            case(
                "too-many-arguments",
                {
                    let mut packet = launch(Vec::new());
                    let Some(Body::Launch(launch)) = &mut packet.body else {
                        unreachable!()
                    };
                    launch.argv = vec![b"x".to_vec(); MAX_ITEM_COUNT + 1];
                    packet
                },
                false,
            ),
            case(
                "duplicate-resource",
                {
                    let mut packet = launch(Vec::new());
                    let Some(Body::Launch(launch)) = &mut packet.body else {
                        unreachable!()
                    };
                    let limit = Rlimit {
                        resource: 7,
                        soft: 1,
                        hard: 2,
                    };
                    launch.rlimits = vec![limit, limit];
                    packet
                },
                false,
            ),
            case(
                "empty-privilege-policy",
                {
                    let mut packet = launch(Vec::new());
                    packet.required_features.push(super::FEATURE_LINUX_SANDBOX);
                    let Some(Body::Launch(launch)) = &mut packet.body else {
                        unreachable!()
                    };
                    launch.sandbox = Some(LinuxSandbox {
                        privileges: Some(PrivilegeReduction::default()),
                        seccomp: None,
                        landlock: false,
                        cgroup: false,
                        namespaces: Vec::new(),
                        landlock_abi: 0,
                        landlock_handled_access: 0,
                    });
                    packet
                },
                false,
            ),
            case(
                "disabled-landlock-with-abi",
                {
                    let mut packet = launch(Vec::new());
                    packet.required_features.push(super::FEATURE_LINUX_SANDBOX);
                    let Some(Body::Launch(launch)) = &mut packet.body else {
                        unreachable!()
                    };
                    launch.sandbox = Some(LinuxSandbox {
                        landlock_abi: 1,
                        ..LinuxSandbox::default()
                    });
                    packet
                },
                false,
            ),
            case(
                "disabled-landlock-with-rights",
                {
                    let mut packet = launch(Vec::new());
                    packet.required_features.push(super::FEATURE_LINUX_SANDBOX);
                    let Some(Body::Launch(launch)) = &mut packet.body else {
                        unreachable!()
                    };
                    launch.sandbox = Some(LinuxSandbox {
                        landlock_handled_access: 1,
                        ..LinuxSandbox::default()
                    });
                    packet
                },
                false,
            ),
            DecoderCase {
                name: "seccomp-value-overflow".to_owned(),
                bytes: seccomp_value_overflow_packet(),
                accepted: false,
            },
            DecoderCase {
                name: "duplicate-seccomp-architecture".to_owned(),
                bytes: duplicate_seccomp_architecture_packet(),
                accepted: false,
            },
            DecoderCase {
                name: "started-pid-overflow".to_owned(),
                bytes: started_pid_overflow_packet(),
                accepted: false,
            },
            DecoderCase {
                name: "non-boolean-unix-option".to_owned(),
                bytes: non_boolean_unix_option_packet(),
                accepted: false,
            },
            DecoderCase {
                name: "repeated-unknown-nested-unix-field".to_owned(),
                bytes: unknown_nested_unix_field_packet(),
                accepted: true,
            },
            DecoderCase {
                name: "too-many-required-features".to_owned(),
                bytes: repeated_field_overflow_packet(3, MAX_ITEM_COUNT + 1),
                accepted: false,
            },
            DecoderCase {
                name: "too-many-descriptor-roles".to_owned(),
                bytes: repeated_field_overflow_packet(4, MAX_FD_COUNT + 1),
                accepted: false,
            },
            DecoderCase {
                name: "too-many-empty-arguments".to_owned(),
                bytes: empty_argument_overflow_packet(),
                accepted: false,
            },
        ];
        cases.extend((0..valid_bytes.len()).map(|length| DecoderCase {
            name: format!("truncated-valid-{length}"),
            bytes: valid_bytes[..length].to_vec(),
            accepted: false,
        }));
        cases
    }

    /// Returns bounded generated cases whose verdicts come from the Prost
    /// decoder oracle for differential comparison with the native decoder.
    #[must_use]
    pub fn generated_decoder_cases() -> Vec<DecoderCase> {
        let mut cases = Vec::with_capacity(128);
        let mut state = 0x4d59_5df4_d0f3_3173u64;
        for index in 0..128 {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let count = usize::try_from(state % 24).unwrap_or(0) + 1;
            let mut environment = Vec::with_capacity(count);
            for item in 0..count {
                state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                let name = format!("K{state:016x}_{item}");
                environment.push(format!("{name}=V{:x}", state.rotate_left(17)).into_bytes());
            }
            if index % 5 == 0 && environment.len() > 1 {
                let equals = environment[0].iter().position(|byte| *byte == b'=').unwrap_or(environment[0].len());
                let mut duplicate = environment[0][..=equals].to_vec();
                duplicate.extend_from_slice(b"different");
                environment.push(duplicate);
            } else if index % 7 == 0 {
                environment[0].insert(1, 0);
            }
            let packet = launch(environment);
            let bytes = packet.encode_to_vec();
            cases.push(DecoderCase {
                name: format!("generated-oracle-{index}"),
                accepted: decode(&bytes).is_ok(),
                bytes,
            });
        }
        cases
    }

    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    mod tests {
        use super::super::{ProtocolError, SeccompInstruction, SeccompPolicy, validate_seccomp};

        fn seccomp_with_jump(jump_true: u32) -> SeccompPolicy {
            SeccompPolicy {
                audit_architecture: 0xc000_003e,
                instructions: vec![
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
                        value: 0xc000_003e,
                    },
                    SeccompInstruction {
                        code: 0x06,
                        jump_true: 0,
                        jump_false: 0,
                        value: 0x8000_0000,
                    },
                    SeccompInstruction {
                        code: 0x15,
                        jump_true,
                        jump_false: 0,
                        value: u32::MAX,
                    },
                    SeccompInstruction {
                        code: 0x06,
                        jump_true: 0,
                        jump_false: 0,
                        value: 0x7fff_0000,
                    },
                ],
            }
        }

        fn seccomp_denying_syscall(syscall: u32) -> SeccompPolicy {
            let mut policy = seccomp_with_jump(0);
            policy.instructions[3] = SeccompInstruction {
                code: 0x20,
                jump_true: 0,
                jump_false: 0,
                value: 0,
            };
            policy.instructions.insert(
                4,
                SeccompInstruction {
                    code: 0x15,
                    jump_true: 0,
                    jump_false: 1,
                    value: syscall,
                },
            );
            policy.instructions.insert(
                5,
                SeccompInstruction {
                    code: 0x06,
                    jump_true: 0,
                    jump_false: 0,
                    value: 0x0005_0001,
                },
            );
            policy
        }

        #[test]
        fn seccomp_jump_offsets_are_relative_to_the_next_instruction() {
            assert_eq!(validate_seccomp(&seccomp_with_jump(0)), Ok(()));
            assert_eq!(validate_seccomp(&seccomp_with_jump(1)), Err(ProtocolError::InvalidSeccomp));
        }

        #[test]
        fn seccomp_must_allow_post_install_acknowledgement_and_cleanup() {
            assert_eq!(validate_seccomp(&seccomp_with_jump(0)), Ok(()));
            assert_eq!(validate_seccomp(&seccomp_denying_syscall(1)), Err(ProtocolError::InvalidSeccomp));
            assert_eq!(validate_seccomp(&seccomp_denying_syscall(3)), Err(ProtocolError::InvalidSeccomp));
        }
    }
}
