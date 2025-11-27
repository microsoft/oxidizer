// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `Unimplemented` error type.

use std::borrow::Cow;

use crate::OhnoCore;

#[derive(crate::Error, Clone)]
#[no_constructors]
#[display("not implemented at {file}:{line}")]
pub struct Unimplemented {
    file: Cow<'static, str>,
    line: usize,
    core: OhnoCore,
}

impl Unimplemented {
    #[must_use]
    pub fn new(file: Cow<'static, str>, line: usize) -> Self {
        Self {
            file,
            line,
            core: OhnoCore::new(),
        }
    }

    #[must_use]
    pub fn with_message(
        message: impl Into<Cow<'static, str>>,
        file: Cow<'static, str>,
        line: usize,
    ) -> Self {
        Self {
            file,
            line,
            core: OhnoCore::from(message.into()),
        }
    }

    #[must_use]
    pub fn file(&self) -> &str {
        &self.file
    }

    #[must_use]
    pub fn line(&self) -> usize {
        self.line
    }
}

#[macro_export]
macro_rules! unimplemented_error {
    () => {
        return Err($crate::Unimplemented::new(
            std::borrow::Cow::Borrowed(file!()),
            line!() as usize,
        )
        .into())
    };
    ($ex:expr) => {
        return Err($crate::Unimplemented::with_message(
            $ex,
            std::borrow::Cow::Borrowed(file!()),
            line!() as usize,
        )
        .into())
    };
}

#[cfg(test)]
mod test {
    use ohno::ErrorExt;

    use super::*;

    #[test]
    fn basic() {
        fn return_err() -> Result<(), Unimplemented> {
            unimplemented_error!()
        }
        let err = return_err().unwrap_err();
        assert!(err.message().starts_with("not implemented at "), "{err}");
    }

    #[test]
    fn with_message() {
        fn return_err() -> Result<(), Unimplemented> {
            unimplemented_error!("custom message")
        }

        let err = return_err().unwrap_err();
        let message = err.message();
        assert!(message.starts_with("not implemented at "), "{message}");
        assert!(message.contains("custom message"), "{message}");
    }

    #[test]
    fn automatic_conversion() {
        #[derive(Debug)]
        struct CustomError(Unimplemented);

        impl From<Unimplemented> for CustomError {
            fn from(err: Unimplemented) -> Self {
                Self(err)
            }
        }

        fn return_custom_err() -> Result<(), CustomError> {
            unimplemented_error!()
        }

        let err = return_custom_err().unwrap_err();
        let message = err.0.message();
        assert!(message.starts_with("not implemented at "), "{message}");
    }
}
