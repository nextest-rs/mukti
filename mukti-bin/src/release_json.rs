// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Add and update to release JSON.

use crate::command::Archive;
use atomicwrites::{AtomicFile, OverwriteBehavior};
use camino::Utf8Path;
use color_eyre::{eyre::Context, Result};
use mukti_metadata::{ReleaseData, ReleaseLocation, ReleasesJson, VersionRange};
use semver::Version;
use std::{collections::BTreeMap, io::BufWriter};

/// Read the releases.json file.
pub(crate) fn read_release_json(path: &Utf8Path) -> Result<ReleasesJson> {
    let release_json: ReleasesJson = if path.exists() {
        let json = std::fs::read_to_string(path)
            .wrap_err_with(|| format!("failed to read releases JSON file at {}", path))?;
        serde_json::from_str(&json)
            .wrap_err_with(|| format!("failed to deserialize releases JSON at {}", path))?
    } else {
        ReleasesJson::default()
    };

    Ok(release_json)
}

pub(crate) fn update_release_json(
    release_json: &mut ReleasesJson,
    url_prefix: &str,
    version: &Version,
    archives: &[Archive],
    path: &Utf8Path,
) -> Result<()> {
    if archives.is_empty() {
        // No archives to add -- skip this.
        return Ok(());
    }

    // Read the release JSON file.
    let range = VersionRange::from_version(version);
    {
        let data = release_json
            .ranges
            .entry(range)
            .or_insert_with(|| ReleaseData {
                latest: version.clone(),
                is_prerelease: !version.pre.is_empty(),
                versions: BTreeMap::new(),
            });

        let locations: Vec<_> = archives
            .iter()
            .map(|archive| ReleaseLocation {
                target: archive.target.clone(),
                url: format!("{}/{}", url_prefix, archive.name),
            })
            .collect();
        data.versions.insert(version.clone(), locations);

        if version > &data.latest {
            data.latest = version.clone();
        }
        data.is_prerelease = !data.latest.pre.is_empty();
    }

    // Check if there's a newer release.
    let latest_range = release_json
        .ranges
        .iter()
        .filter_map(|(range, data)| (!data.is_prerelease).then(|| *range))
        .max();
    release_json.latest = latest_range;

    let file = AtomicFile::new(path, OverwriteBehavior::AllowOverwrite);
    file.write(|f| serde_json::to_writer(BufWriter::new(f), &release_json))
        .wrap_err_with(|| format!("failed to serialize releases JSON to {}", path))?;

    Ok(())
}
