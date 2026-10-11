// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::{error, fmt};

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
}

impl InvalidScriptValue {
    pub(crate) fn new(kind: &'static str, input: &str, expected: &'static str) -> Self {
        Self {
            kind,
            input: input.to_owned(),
            expected,
        }
    }
}

impl fmt::Display for InvalidScriptValue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "invalid {} {:?} for a generated install script (expected {})",
            self.kind, self.input, self.expected,
        )
    }
}

impl error::Error for InvalidScriptValue {}
