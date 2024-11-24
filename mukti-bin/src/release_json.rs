// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Add and update to release JSON.

use crate::checksums::ArchiveWithChecksums;
use atomicwrites::{AtomicFile, OverwriteBehavior};
use camino::Utf8Path;
use color_eyre::eyre::{bail, Result, WrapErr};
use mukti_metadata::{
    MuktiReleasesJson, ReleaseLocation, ReleaseRangeData, ReleaseStatus, ReleaseVersionData,
    VersionRange,
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

        // Look for the latest release that isn't a pre-release.
        // TODO: also consider yanked versions here.
        let latest_non_prerelease = data
            .versions
            .keys()
            .rev()
            .find(|version| version.pre.is_empty());
        match latest_non_prerelease {
            Some(version) => {
                data.latest = version.clone();
                data.is_prerelease = false;
            }
            None => {
                data.latest = data
                    .versions
                    .keys()
                    .next_back()
                    .expect("we just added a release so this can't be empty")
                    .clone();
                data.is_prerelease = true;
            }
        }
    }

    // Check if there's a newer release.
    let latest_range = project
        .ranges
        .iter()
        .filter_map(|(range, data)| (!data.is_prerelease).then_some(*range))
        .max();
    project.latest = latest_range;

    write_releases_json(release_json, path)?;

    Ok(())
}

pub(crate) fn write_releases_json(release_json: &MuktiReleasesJson, path: &Utf8Path) -> Result<()> {
    let file = AtomicFile::new(path, OverwriteBehavior::AllowOverwrite);
    file.write(|f| serde_json::to_writer_pretty(BufWriter::new(f), &release_json))
        .wrap_err_with(|| format!("failed to serialize releases JSON to {}", path))?;

    Ok(())
}
