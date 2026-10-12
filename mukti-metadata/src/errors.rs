// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::VersionRangeKind;
use std::num::ParseIntError;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
#[error("error parsing version range `{input}` at {} component", .component.description())]
pub struct VersionRangeParseError {
    /// The input that failed to parse.
    pub input: String,

    /// The component that failed to parse.
    pub component: VersionRangeKind,

    /// The error that occurred.
    #[source]
    pub error: VersionRangeComponentError,
}

impl VersionRangeParseError {
    pub(crate) fn new(
        input: &str,
        component: VersionRangeKind,
        error: VersionRangeComponentError,
    ) -> Self {
        Self {
            input: input.to_owned(),
            component,
            error,
        }
    }
}

/// The reason on component of a version range failed to parse.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum VersionRangeComponentError {
    /// The component is not an unsigned integer.
    #[error(transparent)]
    ParseInt(ParseIntError),

    /// The component is an integer, but it isn't in canonical form (no leading
    /// plus sign or leading zeros).
    #[error("not in canonical form (expected `{canonical}`)")]
    NotCanonical { canonical: u64 },
}
