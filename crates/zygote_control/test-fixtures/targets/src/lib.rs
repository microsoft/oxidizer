// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

#[cfg(test)]
zygote_rt::link!();

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read, Write};
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::time::Duration;

pub fn run(arguments: Vec<OsString>, prepared_by: Option<u32>) -> i32 {
    let mode = arguments.get(1).and_then(|value| value.to_str()).unwrap_or("report");
    match mode {
        "echo" => echo(),
        "exit" => arguments
            .get(2)
            .and_then(|value| value.to_str())
            .and_then(|value| value.parse().ok())
            .unwrap_or(2),
        "panic" => panic!("fixture panic"),
        "report" => report(&arguments, prepared_by),
        "sleep" => {
            std::thread::sleep(Duration::from_secs(60));
            0
        }
        #[cfg(target_os = "linux")]
        "stdin-state" => {
            // SAFETY: fcntl only observes whether the standard descriptor is open.
            let open = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_GETFD) } >= 0;
            println!("stdin={}", if open { "open" } else { "closed" });
            0
        }
        #[cfg(windows)]
        "stdin-state" => {
            use windows_sys::Win32::System::Console::{GetStdHandle, STD_INPUT_HANDLE};

            // SAFETY: the selector is the documented standard-input identifier.
            let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
            println!("stdin={}", if handle.is_null() { "unavailable" } else { "available" });
            0
        }
        #[cfg(windows)]
        "handle-inheritance-state" => {
            use std::mem::MaybeUninit;

            use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};

            let (Some(handle), Some(sentinel_path), Some(output)) = (arguments.get(2), arguments.get(3), arguments.get(4)) else {
                return 64;
            };
            let handle = handle.to_string_lossy().parse::<usize>().unwrap() as *mut std::ffi::c_void;
            let mut candidate = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
            // SAFETY: candidate is writable and the numeric value is observed
            // only as a candidate handle before this process opens the sentinel.
            let candidate_valid = unsafe { GetFileInformationByHandle(handle, candidate.as_mut_ptr()) } != 0;
            let inherited = candidate_valid && {
                let sentinel = fs::File::open(Path::new(sentinel_path)).unwrap();
                use std::os::windows::io::AsRawHandle as _;
                let mut expected = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
                // SAFETY: sentinel owns a live file handle and expected is writable.
                assert_ne!(
                    unsafe { GetFileInformationByHandle(sentinel.as_raw_handle().cast(), expected.as_mut_ptr()) },
                    0
                );
                // SAFETY: both successful calls initialized their output structures.
                let (candidate, expected) = unsafe { (candidate.assume_init(), expected.assume_init()) };
                candidate.dwVolumeSerialNumber == expected.dwVolumeSerialNumber
                    && candidate.nFileIndexHigh == expected.nFileIndexHigh
                    && candidate.nFileIndexLow == expected.nFileIndexLow
            };
            let state: &[u8] = if inherited { b"inherited" } else { b"unavailable" };
            fs::write(Path::new(output), state).unwrap();
            0
        }
        "write-forever" => loop {
            io::stdout().write_all(&[b'x'; 4096]).unwrap();
            io::stderr().write_all(&[b'y'; 4096]).unwrap();
        },
        "write-forever-survives-pipes" => {
            let Some(pid_path) = arguments.get(2) else {
                return 64;
            };
            fs::write(Path::new(pid_path), std::process::id().to_string()).unwrap();
            loop {
                let stdout_closed = io::stdout().write_all(&[b'x'; 4096]).is_err();
                let stderr_closed = io::stderr().write_all(&[b'y'; 4096]).is_err();
                if stdout_closed && stderr_closed {
                    std::thread::sleep(Duration::from_secs(60));
                }
            }
        }
        #[cfg(target_os = "linux")]
        "unshare-net-hold" => {
            // SAFETY: unshare applies the documented network-namespace flag to
            // this single-threaded fixture process.
            if unsafe { libc::unshare(libc::CLONE_NEWNET) } != 0 {
                println!("unshare_error={}", io::Error::last_os_error().raw_os_error().unwrap_or_default());
                return 1;
            }
            let metadata = fs::metadata("/proc/self/ns/net").unwrap();
            println!("netns={}:{}", metadata.dev(), metadata.ino());
            io::stdout().flush().unwrap();
            let mut release = [0_u8; 1];
            let _ = io::stdin().read(&mut release);
            0
        }
        "touch" => touch(arguments.get(2)),
        "touch-pair" => {
            let (Some(allowed), Some(denied)) = (arguments.get(2), arguments.get(3)) else {
                return 64;
            };
            println!(
                "allowed={:?}",
                fs::write(Path::new(allowed), b"created").map_err(|error| error.kind())
            );
            println!(
                "denied={:?}",
                fs::write(Path::new(denied), b"created").map_err(|error| error.kind())
            );
            0
        }
        #[cfg(target_os = "linux")]
        "getppid" => {
            // SAFETY: errno is thread-local; getppid has no preconditions and
            // fcntl only observes whether the internal setup slot is closed.
            unsafe {
                *libc::__errno_location() = 0;
                println!("result={}", libc::getppid());
                println!("errno={}", *libc::__errno_location());
                println!("fd3={}", if libc::fcntl(3, libc::F_GETFD) == -1 { "closed" } else { "open" });
            }
            0
        }
        _ => 64,
    }
}

fn echo() -> i32 {
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input).unwrap();
    io::stdout().write_all(b"stdout:").unwrap();
    io::stdout().write_all(&input).unwrap();
    io::stderr().write_all(b"stderr:").unwrap();
    io::stderr().write_all(&input).unwrap();
    0
}

fn report(arguments: &[OsString], prepared_by: Option<u32>) -> i32 {
    if let Some(pid) = prepared_by {
        println!("prepared_by={pid}");
    }
    println!("argv={}", join_hex(arguments.iter().map(OsString::as_os_str)));
    println!(
        "env={}",
        std::env::var_os("ZYGOTE_TEST_VALUE").map_or_else(|| "<unset>".to_owned(), |value| hex(value.as_os_str()))
    );
    println!("cwd={}", hex(std::env::current_dir().unwrap().as_os_str()));
    println!("pid={}", std::process::id());
    #[cfg(unix)]
    // SAFETY: these process-query and process-local configuration APIs receive
    // initialized outputs and valid constant selectors.
    unsafe {
        println!("uid={}", libc::getuid());
        println!("gid={}", libc::getgid());
        println!("pgrp={}", libc::getpgrp());
        println!("session={}", libc::getsid(0));
        let mask = libc::umask(0);
        libc::umask(mask);
        println!("umask={mask:o}");

        let mut limit = std::mem::zeroed::<libc::rlimit>();
        assert_eq!(libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit), 0);
        println!("nofile={}:{}", limit.rlim_cur, limit.rlim_max);

        #[cfg(target_os = "linux")]
        {
            let mut action: libc::sigaction = std::mem::zeroed();
            assert_eq!(libc::sigaction(libc::SIGUSR1, std::ptr::null(), &raw mut action), 0);
            let disposition = if action.sa_sigaction == libc::SIG_DFL {
                "default"
            } else if action.sa_sigaction == libc::SIG_IGN {
                "ignored"
            } else {
                "handler"
            };
            println!("sigusr1={disposition}");
            let mut alternate: libc::stack_t = std::mem::zeroed();
            assert_eq!(libc::sigaltstack(std::ptr::null(), &raw mut alternate), 0);
            println!(
                "altstack={}",
                if alternate.ss_flags & libc::SS_DISABLE != 0 {
                    "disabled"
                } else {
                    "enabled"
                }
            );
        }
    }
    #[cfg(target_os = "linux")]
    // SAFETY: prctl receives documented query operations; capget receives
    // initialized version storage and a writable two-element capability array.
    unsafe {
        println!("fds={}", open_descriptors());
        println!("no_new_privs={}", libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0));
        println!("dumpable={}", libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0));
        println!("securebits={}", libc::prctl(libc::PR_GET_SECUREBITS, 0, 0, 0, 0));
        println!("cgroup={}", fs::read_to_string("/proc/self/cgroup").unwrap().trim());
        let namespace = fs::metadata("/proc/self/ns/net").unwrap();
        println!("netns={}:{}", namespace.dev(), namespace.ino());
        let mut header = [0x2008_0522u32, 0];
        let mut data = [0u32; 6];
        assert_eq!(libc::syscall(libc::SYS_capget, header.as_mut_ptr(), data.as_mut_ptr()), 0);
        println!("cap_effective={:08x}{:08x}", data[3], data[0]);
        println!("cap_permitted={:08x}{:08x}", data[4], data[1]);
        println!("cap_inheritable={:08x}{:08x}", data[5], data[2]);
    }
    #[cfg(windows)]
    // SAFETY: each query receives the current process or its live token and
    // initialized writable storage of the documented size.
    unsafe {
        use std::mem::size_of;
        use std::ptr::null_mut;

        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::Security::{
            GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenIntegrityLevel,
            TokenIsAppContainer,
        };
        use windows_sys::Win32::System::JobObjects::IsProcessInJob;
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, GetProcessMitigationPolicy, OpenProcessToken, ProcessDEPPolicy, ProcessDynamicCodePolicy,
        };

        let process = GetCurrentProcess();
        let mut in_job = 0;
        assert_ne!(IsProcessInJob(process, null_mut(), &raw mut in_job), 0);
        println!("job={in_job}");

        let mut token = null_mut();
        assert_ne!(OpenProcessToken(process, TOKEN_QUERY, &raw mut token), 0);
        let mut token_len = 0;
        GetTokenInformation(token, TokenIntegrityLevel, null_mut(), 0, &raw mut token_len);
        assert!(token_len >= size_of::<TOKEN_MANDATORY_LABEL>() as u32);
        let mut token_info = vec![0u8; token_len as usize];
        assert_ne!(
            GetTokenInformation(
                token,
                TokenIntegrityLevel,
                token_info.as_mut_ptr().cast(),
                token_len,
                &raw mut token_len,
            ),
            0
        );
        let mut is_appcontainer = 0u32;
        let mut length = 0;
        assert_ne!(
            GetTokenInformation(
                token,
                TokenIsAppContainer,
                (&raw mut is_appcontainer).cast(),
                size_of::<u32>() as u32,
                &raw mut length,
            ),
            0
        );
        println!("appcontainer={is_appcontainer}");
        assert_ne!(CloseHandle(token), 0);
        let label = token_info.as_ptr().cast::<TOKEN_MANDATORY_LABEL>().read_unaligned();
        let authority_count = *GetSidSubAuthorityCount(label.Label.Sid);
        assert!(authority_count > 0);
        let integrity = *GetSidSubAuthority(label.Label.Sid, u32::from(authority_count - 1));
        println!("integrity={integrity}");

        let mut dep_policy = [0u32; 2];
        assert_ne!(
            GetProcessMitigationPolicy(process, ProcessDEPPolicy, dep_policy.as_mut_ptr().cast(), size_of::<[u32; 2]>(),),
            0
        );
        println!("dep={}", dep_policy[0] & 1);

        let mut dynamic_code_policy = 0u32;
        assert_ne!(
            GetProcessMitigationPolicy(
                process,
                ProcessDynamicCodePolicy,
                (&raw mut dynamic_code_policy).cast(),
                size_of::<u32>(),
            ),
            0
        );
        println!("dynamic_code={}", dynamic_code_policy & 1);
        let child_spawn = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["exit", "0"])
            .status();
        println!("child_spawn={}", if child_spawn.is_err() { "denied" } else { "allowed" });
    }

    0
}

fn touch(path: Option<&OsString>) -> i32 {
    let Some(path) = path else {
        return 64;
    };
    fs::write(Path::new(path), b"created").unwrap();
    #[cfg(unix)]
    {
        let mode = std::os::unix::fs::PermissionsExt::mode(&fs::metadata(path).unwrap().permissions()) & 0o777;
        println!("mode={mode:o}");
    }
    0
}

fn join_hex<'a>(values: impl Iterator<Item = &'a OsStr>) -> String {
    values.map(hex).collect::<Vec<_>>().join(",")
}

fn hex(value: &OsStr) -> String {
    value.as_encoded_bytes().iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(target_os = "linux")]
fn open_descriptors() -> String {
    let descriptors = fs::read_dir("/proc/self/fd")
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().parse::<u32>().unwrap())
        .collect::<Vec<_>>();
    descriptors
        .into_iter()
        .filter(|descriptor| Path::new(&format!("/proc/self/fd/{descriptor}")).exists())
        .map(|descriptor| descriptor.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::mem::{align_of, size_of};

    use windows_sys::Win32::Security::TOKEN_MANDATORY_LABEL;

    #[test]
    fn token_label_can_be_read_from_deliberately_unaligned_storage() {
        let mut storage = vec![0u8; size_of::<TOKEN_MANDATORY_LABEL>() + align_of::<TOKEN_MANDATORY_LABEL>()];
        let offset = (0..align_of::<TOKEN_MANDATORY_LABEL>())
            .find(|offset| (storage.as_ptr() as usize + offset) % align_of::<TOKEN_MANDATORY_LABEL>() != 0)
            .unwrap();
        let pointer = unsafe { storage.as_mut_ptr().add(offset).cast::<TOKEN_MANDATORY_LABEL>() };
        let expected = TOKEN_MANDATORY_LABEL {
            Label: windows_sys::Win32::Security::SID_AND_ATTRIBUTES {
                Sid: std::ptr::null_mut(),
                Attributes: 42,
            },
        };
        unsafe {
            pointer.write_unaligned(expected);
            let actual = pointer.read_unaligned();
            assert!(actual.Label.Sid.is_null());
            assert_eq!(actual.Label.Attributes, 42);
        }
    }
}
