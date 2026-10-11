// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mukti_metadata::VersionRange;
use semver::Version;
use std::fmt;

// Note that variant order determines sort order for redirects.
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub(crate) enum VersionSpec {
    Latest,
    Range(VersionRange),
    Version(Version),
}

impl fmt::Display for VersionSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Latest => write!(f, "latest"),
            Self::Range(range) => write!(f, "{}", range),
            Self::Version(version) => write!(f, "{}", version),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use mukti_metadata::MuktiReleasesJson;

    static FIXTURE_JSON: &str = include_str!("../../fixtures/mukti-releases.json");

    pub(crate) fn fixture_json() -> MuktiReleasesJson {
        serde_json::from_str(FIXTURE_JSON).expect("fixture JSON deserialized")
    }
}
