// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mukti_metadata::{MuktiProject, ReleaseVersionData, VersionRange};
use semver::Version;
use std::fmt;

// Note that variant order determines sort order for redirects.
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub(crate) enum VersionSpec {
    Latest,
    Range(VersionRange),
    Version(Version),
}

impl VersionSpec {
    // Note that 0.0.N is both a range and a version, but we treat it as a range.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        if name == "latest" {
            Some(Self::Latest)
        } else if let Ok(range) = name.parse() {
            Some(Self::Range(range))
        } else if let Ok(version) = name.parse() {
            Some(Self::Version(version))
        } else {
            None
        }
    }
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

#[derive(Clone, Debug)]
pub(crate) struct ResolvedSpec<'a> {
    pub(crate) spec: VersionSpec,
    pub(crate) version: &'a Version,
    pub(crate) data: &'a ReleaseVersionData,
}

pub(crate) fn resolved_specs(project: &MuktiProject) -> Vec<ResolvedSpec<'_>> {
    let mut specs = Vec::new();

    if let Some((_, version, data)) = project.latest_active_stable() {
        specs.push(ResolvedSpec {
            spec: VersionSpec::Latest,
            version,
            data,
        });
    }

    for (range, range_data) in &project.ranges {
        // A range with no active stable version only gets active versions.
        if let Some((version, data)) = range_data.latest_active_stable() {
            specs.push(ResolvedSpec {
                spec: VersionSpec::Range(*range),
                version,
                data,
            });
        }
        for (version, data) in &range_data.versions {
            specs.push(ResolvedSpec {
                spec: VersionSpec::Version(version.clone()),
                version,
                data,
            });
        }
    }

    specs
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

#[cfg(test)]
mod tests {
    use super::{test_helpers::*, *};
    use mukti_metadata::ReleaseStatus;

    fn summarize(project: &MuktiProject) -> Vec<(String, String)> {
        resolved_specs(project)
            .into_iter()
            .map(|resolved| (resolved.spec.to_string(), resolved.version.to_string()))
            .collect()
    }

    fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
        expected
            .iter()
            .map(|(spec, version)| (spec.to_string(), version.to_string()))
            .collect()
    }

    #[test]
    fn from_name_inverts_display() {
        let version =
            |version: &str| VersionSpec::Version(version.parse().expect("version parsed"));
        let range = |range: &str| VersionSpec::Range(range.parse().expect("range parsed"));
        let specs = [
            ("latest", VersionSpec::Latest),
            ("1", range("1")),
            ("0.5", range("0.5")),
            ("0.0.5", range("0.0.5")),
            ("0.5.3", version("0.5.3")),
            ("1.0.0", version("1.0.0")),
            ("0.6.0-alpha.1", version("0.6.0-alpha.1")),
            ("0.0.5-rc.1", version("0.0.5-rc.1")),
        ];
        for (name, spec) in specs {
            assert_eq!(VersionSpec::from_name(name), Some(spec.clone()), "{name}");
            assert_eq!(spec.to_string(), name, "{name} is written as it was read");
        }

        for name in [
            "", "Latest", "backup", "0.05", "+1", "1.2", "0.5.03", "v0.5.3",
        ] {
            assert_eq!(VersionSpec::from_name(name), None, "{name} is not a spec");
        }
    }

    #[test]
    fn specs_skip_yanked_latest() {
        let mut project = fixture_project();
        set_status(&mut project, "0.5.3", ReleaseStatus::Yanked);
        // (Stored latest fields are deliberately left stale here, to ensure
        // mukti doesn't rely on them while generating manifests.)
        assert_eq!(
            summarize(&project),
            pairs(&[
                ("latest", "0.5.1"),
                ("0.5", "0.5.1"),
                ("0.5.1", "0.5.1"),
                ("0.5.2", "0.5.2"),
                ("0.5.3", "0.5.3"),
                ("0.6.0-alpha.1", "0.6.0-alpha.1"),
            ]),
        );
    }

    #[test]
    fn specs_with_no_stable_version() {
        let mut project = fixture_project();
        set_status(&mut project, "0.5.3", ReleaseStatus::Yanked);
        set_status(&mut project, "0.5.1", ReleaseStatus::Yanked);
        assert_eq!(
            summarize(&project),
            pairs(&[
                ("0.5.1", "0.5.1"),
                ("0.5.2", "0.5.2"),
                ("0.5.3", "0.5.3"),
                ("0.6.0-alpha.1", "0.6.0-alpha.1"),
            ]),
        );
    }
}
