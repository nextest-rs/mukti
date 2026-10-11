// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Add and update to release JSON.

use crate::checksums::ArchiveWithChecksums;
use atomicwrites::{AtomicFile, OverwriteBehavior};
use camino::Utf8Path;
use color_eyre::eyre::{bail, Result, WrapErr};
use mukti_metadata::{
    MuktiProject, MuktiReleasesJson, ReleaseLocation, ReleaseRangeData, ReleaseStatus,
    ReleaseVersionData, VersionRange,
};
use semver::Version;
use std::{collections::BTreeMap, io::BufWriter};

/// Read the releases.json file.
pub(crate) fn read_release_json(path: &Utf8Path, allow_missing: bool) -> Result<MuktiReleasesJson> {
    let release_json: MuktiReleasesJson = if path.exists() {
        let json = std::fs::read_to_string(path)
            .wrap_err_with(|| format!("failed to read releases JSON file at {}", path))?;
        serde_json::from_str(&json)
            .wrap_err_with(|| format!("failed to deserialize releases JSON at {}", path))?
    } else if allow_missing {
        MuktiReleasesJson::default()
    } else {
        bail!("releases JSON not found at {}", path);
    };

    Ok(release_json)
}

pub(crate) fn single_project(release_json: &MuktiReleasesJson) -> Result<(&str, &MuktiProject)> {
    if release_json.projects.len() != 1 {
        bail!(
            "mukti-bin currently only supports one project, {} found",
            release_json.projects.len()
        );
    }

    let (name, project) = release_json
        .projects
        .iter()
        .next()
        .expect("release_json has one project");
    Ok((name, project))
}

pub(crate) fn update_release_json(
    release_json: &mut MuktiReleasesJson,
    release_url: &str,
    version: &Version,
    archives: Vec<ArchiveWithChecksums>,
    path: &Utf8Path,
) -> Result<()> {
    if archives.is_empty() {
        // No archives to add -- skip this.
        return Ok(());
    }

    if release_json.projects.len() != 1 {
        bail!(
            "mukti-bin currently only supports one project, {} found",
            release_json.projects.len()
        );
    }

    let project = release_json
        .projects
        .values_mut()
        .next()
        .expect("release_json has one project");

    // Read the release JSON file.
    let range = VersionRange::from_version(version);
    {
        let data = project
            .ranges
            .entry(range)
            .or_insert_with(|| ReleaseRangeData {
                latest: version.clone(),
                is_prerelease: !version.pre.is_empty(),
                versions: BTreeMap::new(),
            });

        let locations: Vec<_> = archives
            .into_iter()
            .map(|archive| {
                let checksums = match archive.checksums {
                    Ok(checksums) => checksums.to_checksum_map(),
                    Err(e) => {
                        eprintln!(
                            "failed to compute checksums for {}: {}",
                            archive.archive.name, e
                        );
                        BTreeMap::new()
                    }
                };

                ReleaseLocation {
                    target: archive.archive.target_format.target.clone(),
                    format: archive.archive.target_format.format.clone(),
                    url: archive.url,
                    checksums,
                }
            })
            .collect();
        data.versions.insert(
            version.clone(),
            ReleaseVersionData {
                release_url: release_url.to_owned(),
                status: ReleaseStatus::Active,
                locations,
                metadata: serde_json::Value::Null,
            },
        );
    }

    recompute_latest(project);

    write_releases_json(release_json, path)?;

    Ok(())
}

/// Recomputes the latest for every range.
///
/// We do this in case a yank edited by hand didn't update state properly.
fn recompute_latest(project: &mut MuktiProject) {
    for data in project.ranges.values_mut() {
        // Prefer, in order:
        //
        // * The newest active stable version.
        // * The newest active version.
        // * The newest of any status. (The last one because latest is not
        //   optional in the releases.json schema.)
        let newest_active = data
            .versions
            .iter()
            .rev()
            .find(|(_, version_data)| version_data.status == ReleaseStatus::Active)
            .map(|(version, _)| version);
        let latest = data
            .latest_active_stable()
            .map(|(version, _)| version)
            .or(newest_active)
            .or_else(|| data.versions.keys().next_back());
        if let Some(version) = latest.cloned() {
            data.latest = version;
        }
        data.is_prerelease = data.versions.keys().all(|version| !version.pre.is_empty());
    }

    project.latest = project.latest_active_stable().map(|(range, _, _)| *range);
}

pub(crate) fn write_releases_json(release_json: &MuktiReleasesJson, path: &Utf8Path) -> Result<()> {
    let file = AtomicFile::new(path, OverwriteBehavior::AllowOverwrite);
    file.write(|f| serde_json::to_writer_pretty(BufWriter::new(f), &release_json))
        .wrap_err_with(|| format!("failed to serialize releases JSON to {}", path))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::specs::test_helpers::{fixture_project, set_status};
    use semver::Version;

    fn version(version: &str) -> Version {
        version.parse().expect("version parsed")
    }

    #[test]
    fn recompute_latest_skips_yanked() {
        let mut project = fixture_project();
        let range_0_5: VersionRange = "0.5".parse().expect("range parsed");
        let range_0_6: VersionRange = "0.6".parse().expect("range parsed");

        recompute_latest(&mut project);
        assert_eq!(project.latest, Some(range_0_5));
        assert_eq!(project.ranges[&range_0_5].latest, version("0.5.3"));
        assert!(!project.ranges[&range_0_5].is_prerelease);
        assert_eq!(project.ranges[&range_0_6].latest, version("0.6.0-alpha.1"));
        assert!(project.ranges[&range_0_6].is_prerelease);

        set_status(&mut project, "0.5.3", ReleaseStatus::Yanked);
        recompute_latest(&mut project);
        assert_eq!(project.latest, Some(range_0_5));
        assert_eq!(project.ranges[&range_0_5].latest, version("0.5.1"));
        assert!(!project.ranges[&range_0_5].is_prerelease);

        set_status(&mut project, "0.5.1", ReleaseStatus::Yanked);
        recompute_latest(&mut project);
        assert_eq!(project.latest, None);
        assert_eq!(project.ranges[&range_0_5].latest, version("0.5.3"));
        assert!(
            !project.ranges[&range_0_5].is_prerelease,
            "the range still has stable versions, even though all are yanked",
        );
    }
}
