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
