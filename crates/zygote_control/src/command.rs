// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
#[cfg(all(not(target_os = "linux"), not(windows)))]
use std::process;
use std::process::{ExitStatus, Output};
#[cfg(not(target_os = "linux"))]
use std::sync::Arc;
use std::{fmt, io};

use crate::SandboxPolicy;
use crate::child::{Child, OutputLimits};
#[cfg(all(not(target_os = "linux"), not(windows)))]
use crate::child::{ChildInner, ChildStderr, ChildStdin, ChildStdout};
use crate::stdio::Stdio;
#[cfg(unix)]
use crate::unix::UnixOptions;
use crate::zygote::{Launcher, LauncherInner};

/// A builder for one process launch.
pub struct Command {
    pub(super) launcher: Launcher,
    pub(super) args: Vec<OsString>,
    pub(super) environment: BTreeMap<OsString, Option<OsString>>,
    pub(super) clear_environment: bool,
    pub(super) current_dir: Option<PathBuf>,
    pub(super) stdin: Option<Stdio>,
    pub(super) stdout: Option<Stdio>,
    pub(super) stderr: Option<Stdio>,
    pub(super) sandbox: SandboxPolicy,
    pub(super) output_limits: OutputLimits,
    #[cfg(unix)]
    pub(super) unix: UnixOptions,
    #[cfg(target_os = "linux")]
    pub(super) linux_sandbox: crate::linux::SandboxOptions,
    #[cfg(windows)]
    pub(super) windows_sandbox: crate::windows::SandboxOptions,
}

#[cfg(target_os = "linux")]
pub(super) struct ResolvedEnvironment<'a> {
    inherited: BTreeMap<OsString, OsString>,
    overrides: &'a BTreeMap<OsString, Option<OsString>>,
}

#[cfg(target_os = "linux")]
impl ResolvedEnvironment<'_> {
    pub(super) fn len(&self) -> usize {
        self.inherited.len() + self.overrides.values().filter(|value| value.is_some()).count()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (&OsStr, &OsStr)> {
        self.inherited
            .iter()
            .map(|(key, value)| (key.as_os_str(), value.as_os_str()))
            .chain(
                self.overrides
                    .iter()
                    .filter_map(|(key, value)| value.as_ref().map(|value| (key.as_os_str(), value.as_os_str()))),
            )
    }
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Command")
            .field("program", &self.get_program())
            .field("args", &self.args)
            .field("environment", &EnvironmentDebug(&self.environment))
            .field("clear_environment", &self.clear_environment)
            .field("current_dir", &self.current_dir)
            .finish_non_exhaustive()
    }
}

struct EnvironmentDebug<'a>(&'a BTreeMap<OsString, Option<OsString>>);

impl fmt::Debug for EnvironmentDebug<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(
                self.0
                    .iter()
                    .map(|(key, value)| (key, if value.is_some() { "<set>" } else { "<removed>" })),
            )
            .finish()
    }
}

impl Command {
    pub(super) fn new(launcher: Launcher) -> Self {
        Self {
            launcher,
            args: Vec::new(),
            environment: BTreeMap::new(),
            clear_environment: false,
            current_dir: None,
            stdin: None,
            stdout: None,
            stderr: None,
            sandbox: SandboxPolicy::new(),
            output_limits: OutputLimits::default(),
            #[cfg(unix)]
            unix: UnixOptions::default(),
            #[cfg(target_os = "linux")]
            linux_sandbox: crate::linux::SandboxOptions::default(),
            #[cfg(windows)]
            windows_sandbox: crate::windows::SandboxOptions::default(),
        }
    }

    /// Adds one argument.
    pub fn arg<S: AsRef<OsStr>>(&mut self, argument: S) -> &mut Self {
        self.args.push(argument.as_ref().to_owned());
        self
    }

    /// Adds multiple arguments.
    pub fn args<I, S>(&mut self, arguments: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args.extend(arguments.into_iter().map(|argument| argument.as_ref().to_owned()));
        self
    }

    /// Sets or replaces one environment variable.
    pub fn env<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        let key = key.as_ref();
        #[cfg(windows)]
        self.environment
            .retain(|existing, _| !crate::windows::environment_names_equal(existing, key));
        self.environment.insert(key.to_owned(), Some(value.as_ref().to_owned()));
        self
    }

    /// Sets multiple environment variables.
    pub fn envs<I, K, V>(&mut self, variables: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        for (key, value) in variables {
            self.env(key, value);
        }
        self
    }

    /// Removes an environment variable.
    pub fn env_remove<K: AsRef<OsStr>>(&mut self, key: K) -> &mut Self {
        let key = key.as_ref();
        #[cfg(windows)]
        self.environment
            .retain(|existing, _| !crate::windows::environment_names_equal(existing, key));
        self.environment.insert(key.to_owned(), None);
        self
    }

    /// Clears the inherited environment.
    pub fn env_clear(&mut self) -> &mut Self {
        self.clear_environment = true;
        self.environment.clear();
        self
    }

    /// Sets the child working directory.
    pub fn current_dir<P: AsRef<Path>>(&mut self, directory: P) -> &mut Self {
        self.current_dir = Some(directory.as_ref().to_owned());
        self
    }

    /// Configures standard input.
    pub fn stdin<T: Into<Stdio>>(&mut self, configuration: T) -> &mut Self {
        self.stdin = Some(configuration.into());
        self
    }

    /// Configures standard output.
    pub fn stdout<T: Into<Stdio>>(&mut self, configuration: T) -> &mut Self {
        self.stdout = Some(configuration.into());
        self
    }

    /// Configures standard error.
    pub fn stderr<T: Into<Stdio>>(&mut self, configuration: T) -> &mut Self {
        self.stderr = Some(configuration.into());
        self
    }

    /// Attaches portable, fail-closed sandbox requirements to this launch.
    pub fn sandbox(&mut self, policy: SandboxPolicy) -> &mut Self {
        self.sandbox = policy;
        self
    }

    /// Sets finite per-stream and aggregate captured-output limits.
    ///
    /// The default is 8 MiB per stream and 16 MiB in aggregate. Exceeding
    /// either limit cancels pipe collection and performs bounded child cleanup.
    pub fn output_limits(&mut self, limits: OutputLimits) -> &mut Self {
        self.output_limits = limits;
        self
    }

    /// Returns the fixed target program.
    #[must_use]
    pub fn get_program(&self) -> &OsStr {
        self.launcher.program.as_ref()
    }

    /// Returns configured arguments, excluding the program.
    pub fn get_args(&self) -> impl Iterator<Item = &OsStr> {
        self.args.iter().map(OsString::as_os_str)
    }

    /// Returns configured environment modifications.
    pub fn get_envs(&self) -> impl Iterator<Item = (&OsStr, Option<&OsStr>)> {
        self.environment.iter().map(|(key, value)| (key.as_os_str(), value.as_deref()))
    }

    /// Returns the configured working directory.
    #[must_use]
    pub fn get_current_dir(&self) -> Option<&Path> {
        self.current_dir.as_deref()
    }

    /// Launches the process.
    ///
    /// # Errors
    ///
    /// Returns an error if launch configuration is invalid or the selected
    /// backend cannot create the process.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "matches std::process::Command and reserves mutable builder state"
    )]
    pub fn spawn(&mut self) -> io::Result<Child> {
        self.spawn_with_defaults(Stdio::inherit(), Stdio::inherit(), Stdio::inherit())
    }

    /// Launches the process and waits for its exit status.
    ///
    /// # Errors
    ///
    /// Returns an error if launching or waiting for the process fails.
    pub fn status(&mut self) -> io::Result<ExitStatus> {
        self.spawn()?.wait()
    }

    /// Launches the process and captures its output.
    ///
    /// # Errors
    ///
    /// Returns an error if launching, waiting, or reading captured output
    /// fails.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "matches std::process::Command and reserves mutable builder state"
    )]
    pub fn output(&mut self) -> io::Result<Output> {
        self.spawn_with_defaults(Stdio::null(), Stdio::piped(), Stdio::piped())?
            .wait_with_output()
    }

    fn spawn_with_defaults(&self, default_stdin: Stdio, default_stdout: Stdio, default_stderr: Stdio) -> io::Result<Child> {
        let lifecycle = self.launcher.begin_launch()?;
        self.sandbox.probe()?;
        let stdin = self.stdin.as_ref().map_or(Ok(default_stdin), Stdio::try_clone)?;
        let stdout = self.stdout.as_ref().map_or(Ok(default_stdout), Stdio::try_clone)?;
        let stderr = self.stderr.as_ref().map_or(Ok(default_stderr), Stdio::try_clone)?;

        let result = match self.launcher.inner.as_ref() {
            #[cfg(test)]
            LauncherInner::Test => Err(io::Error::other("test launcher cannot spawn")),
            #[cfg(all(not(target_os = "linux"), not(windows)))]
            LauncherInner::Native => self.spawn_native(stdin, stdout, stderr),
            #[cfg(windows)]
            LauncherInner::Native => crate::windows::spawn(self, stdin, stdout, stderr),
            #[cfg(target_os = "linux")]
            LauncherInner::Linux(pool) => pool.spawn(self, stdin, stdout, stderr),
        };
        #[cfg(not(target_os = "linux"))]
        let mut result = result;
        if result.is_ok() {
            self.launcher.health.launches.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            #[cfg(not(target_os = "linux"))]
            if let Ok(child) = &mut result {
                self.launcher
                    .health
                    .active_children
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                child.active_health = Some(Arc::clone(&self.launcher.health));
            }
        } else {
            self.launcher
                .health
                .launch_failures
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        drop(lifecycle);
        result
    }

    #[cfg(all(not(target_os = "linux"), not(windows)))]
    fn spawn_native(&self, stdin: Stdio, stdout: Stdio, stderr: Stdio) -> io::Result<Child> {
        if !self.sandbox.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "sandbox policies are unsupported on this target",
            ));
        }
        let mut command = process::Command::new(self.launcher.program.as_ref());
        command.args(&self.args);
        if self.clear_environment {
            command.env_clear();
        }
        for (key, value) in &self.environment {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
        if let Some(directory) = &self.current_dir {
            command.current_dir(directory);
        }
        #[cfg(unix)]
        crate::unix::apply_native(&mut command, &self.unix)?;
        command.stdin(stdin.into_std()).stdout(stdout.into_std()).stderr(stderr.into_std());

        let mut child = command.spawn()?;
        let stdin = child.stdin.take().map(|inner| ChildStdin { inner: Box::new(inner) });
        let stdout = child.stdout.take().map(|inner| ChildStdout { inner: Box::new(inner) });
        let stderr = child.stderr.take().map(|inner| ChildStderr { inner: Box::new(inner) });
        Ok(Child {
            inner: ChildInner::Native(child),
            stdin,
            stdout,
            stderr,
            output_limits: self.output_limits,
            active_health: None,
        })
    }

    #[cfg(target_os = "linux")]
    pub(super) fn resolved_environment(&self) -> ResolvedEnvironment<'_> {
        let mut inherited = if self.clear_environment {
            BTreeMap::new()
        } else {
            std::env::vars_os().collect()
        };
        for key in self.environment.keys() {
            inherited.remove(key);
        }
        ResolvedEnvironment {
            inherited,
            overrides: &self.environment,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn environment_debug_redacts_values() {
        let mut environment = BTreeMap::new();
        environment.insert(OsString::from("TOKEN"), Some(OsString::from("sentinel-secret")));
        environment.insert(OsString::from("REMOVED"), None);

        let output = format!("{:?}", EnvironmentDebug(&environment));

        assert!(output.contains("TOKEN"));
        assert!(output.contains("<set>"));
        assert!(output.contains("REMOVED"));
        assert!(output.contains("<removed>"));
        assert!(!output.contains("sentinel-secret"));
    }

    #[test]
    fn builder_operations_preserve_order_and_last_write_semantics() {
        let mut command = Command::new(Launcher::for_test("fixture"));
        command
            .args(["first", "second"])
            .envs([("A", "one"), ("B", "two")])
            .env("A", "replacement")
            .env_remove("B")
            .current_dir("relative")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        assert_eq!(command.get_program(), OsStr::new("fixture"));
        assert_eq!(command.get_args().collect::<Vec<_>>(), [OsStr::new("first"), OsStr::new("second")]);
        assert_eq!(
            command.get_envs().collect::<Vec<_>>(),
            [(OsStr::new("A"), Some(OsStr::new("replacement"))), (OsStr::new("B"), None),]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("relative")));
        assert!(matches!(command.stdin.as_ref().unwrap().inner, crate::stdio::StdioInner::Null));
        assert!(matches!(command.stdout.as_ref().unwrap().inner, crate::stdio::StdioInner::Piped));
        assert!(matches!(command.stderr.as_ref().unwrap().inner, crate::stdio::StdioInner::Inherit));

        command.env_clear();
        assert!(command.clear_environment);
        assert_eq!(command.get_envs().count(), 0);
    }
}
