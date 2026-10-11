// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::{error, fmt, num::ParseIntError};

#[derive(Clone, Debug)]
pub(crate) struct NameValueParseError {
    pub(crate) input: String,
    pub(crate) delimiter: char,
}

impl fmt::Display for NameValueParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "unable to parse '{}' in the format NAME{}VALUE",
            self.input, self.delimiter,
        )
    }
}

impl error::Error for NameValueParseError {}

#[derive(Clone, Debug)]
pub(crate) struct InvalidScriptValue {
    kind: &'static str,
    input: String,
    expected: &'static str,
    cause: Option<InvalidNumber>,
}

impl InvalidScriptValue {
    pub(crate) fn new(kind: &'static str, input: &str, expected: &'static str) -> Self {
        Self {
            kind,
            input: input.to_owned(),
            expected,
            cause: None,
        }
    }

    pub(crate) fn with_cause(self, cause: InvalidNumber) -> Self {
        Self {
            cause: Some(cause),
            ..self
        }
    }
}

impl fmt::Display for InvalidScriptValue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // These values are embedded in generated scripts, so we deliberately
        // have a small allowlist.
        write!(
            f,
            "invalid {} {:?} for a generated install script (expected {})",
            self.kind, self.input, self.expected,
        )?;
        // (clap only prints the message, so attach the sources ourselves.)
        if let Some(cause) = &self.cause {
            write!(f, ": {cause}")?;
        }
        Ok(())
    }
}

impl error::Error for InvalidScriptValue {}

#[derive(Clone, Debug)]
pub(crate) struct InvalidNumber {
    input: String,
    kind: InvalidNumberKind,
}

#[derive(Clone, Debug)]
pub(crate) enum InvalidNumberKind {
    ParseInt(ParseIntError),
    NotCanonical { canonical: u32 },
}

impl InvalidNumber {
    pub(crate) fn new(input: &str, kind: InvalidNumberKind) -> Self {
        Self {
            input: input.to_owned(),
            kind,
        }
    }
}

impl fmt::Display for InvalidNumber {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.kind {
            InvalidNumberKind::ParseInt(error) => {
                write!(f, "{:?} is not a number: {error}", self.input)
            }
            InvalidNumberKind::NotCanonical { canonical } => write!(
                f,
                "{:?} is not in canonical form (expected {canonical})",
                self.input,
            ),
        }
    }
}
