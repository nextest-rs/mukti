// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::VersionRangeKind;
use std::{error, fmt, num::ParseIntError};

#[derive(Debug)]
#[non_exhaustive]
pub struct VersionRangeParseError {
    /// The input that failed to parse.
    pub input: String,

    /// The component that failed to parse.
    pub component: VersionRangeKind,

    /// The error that occurred.
    pub error: ParseIntError,
}

impl VersionRangeParseError {
    pub(crate) fn new(input: &str, component: VersionRangeKind, error: ParseIntError) -> Self {
        Self {
            input: input.to_owned(),
            component,
            error,
        }
    }
}

impl fmt::Display for VersionRangeParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "unable to parse version range input {} at component {}",
            self.input,
            self.component.description()
        )
    }
}

impl error::Error for VersionRangeParseError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        Some(&self.error)
    }
}
