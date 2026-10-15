// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generated mocks support internal adapters without a public production API.

#![deny(clippy::unused_self, private_interfaces, unexpected_cfgs, unfulfilled_lint_expectations)]

use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Output};

#[cfg_attr(test, fakeable::fakeable(fake_impl = fakes::MockBackend, fakes_feature = false))]
pub(crate) struct Backend;

#[derive(Debug)]
pub(crate) struct Request {
    value: u32,
}

#[cfg_attr(
    test,
    fakeable::fakeable(generate_mockall_fake = true, fakes_feature = false, mockall_fake_visibility = "pub(crate)")
)]
impl Backend {
    pub(crate) const fn new() -> Self {
        Self {}
    }

    pub(crate) fn open(&self, path: &Path) -> io::Result<Box<dyn Write + Send>> {
        Ok(Box::new(std::fs::File::create(path)?))
    }

    #[expect(
        clippy::unused_self,
        reason = "Verifies that explicit method lint expectations still take precedence"
    )]
    pub(crate) fn output(&self, command: &mut Command) -> io::Result<Output> {
        command.output()
    }

    pub(crate) fn write(&self, contents: &[u8], writer: &mut dyn Write) -> io::Result<()> {
        writer.write_all(contents)
    }

    pub(crate) fn value(&self, request: &Request) -> u32 {
        request.value
    }
}

#[cfg(test)]
mod tests {
    use mockall::Sequence;
    use mockall::predicate::eq;

    use super::fakes::MockBackend;
    use super::{Backend, Command, Path, Request, io};

    mod local {
        #[fakeable::fakeable(fake_impl = MockLocal, fakes_feature = false)]
        pub(super) struct Local;

        #[fakeable::fakeable(
            generate_mockall_fake = true,
            mockall_fake_module = ".",
            fakes_feature = false,
            mockall_fake_visibility = "pub(super)"
        )]
        #[expect(
            clippy::unused_self,
            reason = "Verifies that explicit impl lint expectations still take precedence"
        )]
        impl Local {
            pub(crate) fn value(&self) -> u32 {
                42
            }
        }
    }

    #[test]
    fn crate_private_adapter_supports_ordered_calls_and_argument_matching() {
        let mut mock = MockBackend::new();
        let mut sequence = Sequence::new();
        mock.expect_open()
            .with(eq(Path::new("virtual.log")))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(Box::new(Vec::<u8>::new())));
        mock.expect_write()
            .withf(|contents, _| contents == b"hello")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|contents, writer| writer.write_all(contents));
        mock.expect_output()
            .withf(|command| command.get_program() == "virtual-tool" && command.get_args().eq(["--version"]))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Err(io::Error::new(io::ErrorKind::PermissionDenied, "scripted launch failure")));

        let backend = Backend::fake(mock);
        let mut writer = backend.open(Path::new("virtual.log")).unwrap();
        backend.write(b"hello", &mut writer).unwrap();
        let error = backend.output(Command::new("virtual-tool").arg("--version")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn crate_private_mock_is_available_in_the_current_module() {
        let mut mock = local::MockLocal::new();
        mock.expect_value().times(1).return_const(7u32);
        assert_eq!(local::Local::fake(mock).value(), 7);
    }

    #[test]
    fn real_adapter_writes_to_the_supplied_writer() {
        let mut contents = Vec::new();
        let backend = Backend::new();
        backend.write(b"real", &mut contents).unwrap();
        assert_eq!(contents, b"real");
        assert_eq!(backend.value(&Request { value: 42 }), 42);
    }

    #[test]
    fn crate_private_mock_accepts_crate_private_types() {
        let mut mock = MockBackend::new();
        mock.expect_value().withf(|request| request.value == 3).times(1).return_const(7u32);
        assert_eq!(Backend::fake(mock).value(&Request { value: 3 }), 7);
    }
}
