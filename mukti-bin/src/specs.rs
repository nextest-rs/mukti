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
    use mukti_metadata::{MuktiProject, MuktiReleasesJson, ReleaseStatus};
    use semver::Version;

    static FIXTURE_JSON: &str = include_str!("../../fixtures/mukti-releases.json");

    pub(crate) fn fixture_json() -> MuktiReleasesJson {
        serde_json::from_str(FIXTURE_JSON).expect("fixture JSON deserialized")
    }

    pub(crate) fn fixture_project() -> MuktiProject {
        fixture_json()
            .projects
            .remove("mukti")
            .expect("fixture has a mukti project")
    }

    pub(crate) fn set_status(project: &mut MuktiProject, version: &str, status: ReleaseStatus) {
        let version: Version = version.parse().expect("version parsed");
        let data = project
            .ranges
            .values_mut()
            .find_map(|range| range.versions.get_mut(&version))
            .expect("version found in fixture");
        data.status = status;
    }
}
