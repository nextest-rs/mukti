// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    atomic::write_atomic,
    release_json::single_project,
    specs::{resolved_specs, ResolvedSpec, VersionSpec},
};
use camino::Utf8Path;
use color_eyre::eyre::{bail, Context, Result};
use mukti_metadata::{DigestAlgorithm, MuktiProject, MuktiReleasesJson, ReleaseStatus};
use semver::Version;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs, io,
};

pub(crate) const MANIFEST_FILE_NAME: &str = "install-manifest.txt";
pub(crate) const MANIFEST_HEADER: &str = "mukti-install-manifest";
/// Bump the number on any change that existing install scripts cannot read.
pub(crate) const MANIFEST_FORMAT_VERSION: u32 = 1;

/// The name of the directory a spec's manifest is written to.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct ManifestDirName(String);

impl ManifestDirName {
    fn new(spec: &VersionSpec) -> Self {
        Self(spec.to_string())
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

pub(crate) fn generate_install_manifests(
    release_json: &MuktiReleasesJson,
    out_dir: &Utf8Path,
) -> Result<()> {
    let (project_name, project) = single_project(release_json)?;
    let manifests = render_manifests(project_name, project)?;
    if manifests.is_empty() {
        bail!(
            "the release JSON has no versions for {project_name}, so there are no install manifests to write; \
             nothing under {out_dir} was changed"
        );
    }

    for (dir_name, contents) in &manifests {
        write_atomic(
            &out_dir.join(dir_name.as_str()).join(MANIFEST_FILE_NAME),
            contents.as_bytes(),
        )?;
    }
    remove_stale_manifests(out_dir, project_name, &manifests.keys().collect())?;

    eprintln!(
        "wrote {} install manifests under {out_dir}",
        manifests.len()
    );
    Ok(())
}

/// Returns the contents of the manifest to write in each directory.
fn render_manifests(
    project_name: &str,
    project: &MuktiProject,
) -> Result<BTreeMap<ManifestDirName, String>> {
    check_field("project name", project_name)?;

    // Render each version once (latest and range specs will reuse the text).
    let mut by_version: BTreeMap<&Version, String> = BTreeMap::new();
    let mut manifests = BTreeMap::new();

    for resolved in resolved_specs(project) {
        let contents = match by_version.get(resolved.version) {
            Some(contents) => contents.clone(),
            None => {
                let contents = render_manifest(project_name, &resolved).wrap_err_with(|| {
                    format!(
                        "cannot write the install manifest for version {}",
                        resolved.version
                    )
                })?;
                by_version.insert(resolved.version, contents.clone());
                contents
            }
        };
        manifests.insert(ManifestDirName::new(&resolved.spec), contents);
    }

    Ok(manifests)
}

fn render_manifest(project_name: &str, resolved: &ResolvedSpec<'_>) -> Result<String> {
    let version = resolved.version;
    let status = match resolved.data.status {
        ReleaseStatus::Active => "active",
        ReleaseStatus::Yanked => "yanked",
    };

    let mut out = String::new();
    writeln!(out, "{MANIFEST_HEADER} {MANIFEST_FORMAT_VERSION}")?;
    writeln!(out, "project {project_name}")?;
    writeln!(out, "version {version}")?;
    writeln!(out, "status {status}")?;

    for location in &resolved.data.locations {
        let Some(sha256) = location.checksums.get(&DigestAlgorithm::SHA256) else {
            bail!(
                "the release JSON has no sha256 checksum for {}.{}; \
                 run `mukti-bin backfill-checksums`, then generate the manifests again",
                location.target,
                location.format,
            );
        };
        let sha256 = sha256.0.as_str();

        check_field("target", &location.target)?;
        check_field("format", &location.format)?;
        check_field("URL", &location.url)?;
        if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!(
                "sha256 checksum `{sha256}` for {}.{} is not 64 hex characters",
                location.target,
                location.format,
            );
        }

        writeln!(
            out,
            "archive {} {} {} {}",
            location.target,
            location.format,
            sha256.to_ascii_lowercase(),
            location.url,
        )?;
    }

    Ok(out)
}

fn check_field(what: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        bail!("{what} is empty");
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        bail!("{what} `{value}` contains whitespace or control characters");
    }
    Ok(())
}

fn remove_stale_manifests(
    out_dir: &Utf8Path,
    project_name: &str,
    current: &BTreeSet<&ManifestDirName>,
) -> Result<()> {
    let entries = out_dir
        .read_dir_utf8()
        .wrap_err_with(|| format!("failed to read directory {out_dir}"))?;
    let expected_start = format!("{MANIFEST_HEADER} ");
    let expected_project = format!("project {project_name}");

    for entry in entries {
        // A non-UTF-8 name in the output directory is an error.
        let entry = entry.wrap_err_with(|| format!("failed to read directory {out_dir}"))?;
        let dir = entry.path();
        // A directory that is not named like a spec is ignored by the tool.
        // (XXX should we fail here?)
        let Some(spec) = VersionSpec::from_name(entry.file_name()) else {
            continue;
        };
        if current.contains(&ManifestDirName::new(&spec)) {
            continue;
        }
        // entry.file_type does not follow symlinks, so a link to a directory
        // elsewhere is not entered.
        let file_type = entry
            .file_type()
            .wrap_err_with(|| format!("failed to get the file type of {dir}"))?;
        if !file_type.is_dir() {
            continue;
        }

        let path = dir.join(MANIFEST_FILE_NAME);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).wrap_err_with(|| format!("failed to get metadata for {path}"));
            }
        }
        let contents = fs::read(&path).wrap_err_with(|| format!("failed to read {path}"))?;
        let contents = String::from_utf8_lossy(&contents);
        let mut lines = contents.lines();
        let is_ours = lines
            .next()
            .is_some_and(|line| line.starts_with(&expected_start))
            && lines.next() == Some(expected_project.as_str());
        if !is_ours {
            continue;
        }

        fs::remove_file(&path)
            .wrap_err_with(|| format!("failed to remove stale install manifest {path}"))?;
        eprintln!("removed stale install manifest {path}");

        match fs::remove_dir(dir) {
            Ok(()) => {}
            // The directory contains other files, so leave it alone.
            Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {}
            Err(error) => {
                return Err(error).wrap_err_with(|| format!("failed to remove directory {dir}"));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::specs::test_helpers::{fixture_json, set_status};
    use camino_tempfile::Utf8TempDir;
    use mukti_metadata::{Digest, ReleaseLocation, ReleaseRangeData, VersionRange};

    fn temp_out_dir() -> Utf8TempDir {
        camino_tempfile::tempdir().expect("temp dir created")
    }

    fn read_tree(root: &Utf8Path) -> BTreeMap<String, String> {
        fn visit(root: &Utf8Path, dir: &Utf8Path, out: &mut BTreeMap<String, String>) {
            for entry in dir.read_dir_utf8().expect("directory read") {
                let entry = entry.expect("directory entry read");
                let path = entry.path();
                if path.is_dir() {
                    visit(root, path, out);
                } else {
                    let relative = path
                        .strip_prefix(root)
                        .expect("path is under root")
                        .as_str()
                        .replace('\\', "/");
                    let contents = fs::read_to_string(path).expect("file read");
                    out.insert(relative, contents);
                }
            }
        }

        let mut out = BTreeMap::new();
        visit(root, root, &mut out);
        out
    }

    fn first_location(json: &mut MuktiReleasesJson) -> &mut ReleaseLocation {
        let version: Version = "0.5.3".parse().expect("version parsed");
        let project = json.projects.get_mut("mukti").expect("fixture has mukti");
        let data = project
            .ranges
            .values_mut()
            .find_map(|range| range.versions.get_mut(&version))
            .expect("0.5.3 is in the fixture");
        &mut data.locations[0]
    }

    fn manifest(project: &str, version: &str) -> String {
        format!("{MANIFEST_HEADER} {MANIFEST_FORMAT_VERSION}\nproject {project}\nversion {version}\nstatus active\n")
    }

    fn write(path: &Utf8Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("path has a parent")).expect("directory created");
        fs::write(path, contents).expect("file written");
    }

    #[test]
    fn manifests_match_expected() {
        let json = fixture_json();
        let temp_dir = temp_out_dir();
        let out_dir = temp_dir.path();
        generate_install_manifests(&json, out_dir).expect("manifests written");
        let tree: Vec<_> = read_tree(out_dir)
            .iter()
            .map(|(path, contents)| format!("==> {path} <==\n{contents}"))
            .collect();
        insta::assert_snapshot!(tree.join("\n"));
    }

    #[test]
    fn missing_sha256_is_an_error_and_changes_nothing() {
        let mut json = fixture_json();
        let removed = first_location(&mut json)
            .checksums
            .remove(&DigestAlgorithm::SHA256);
        assert!(removed.is_some(), "the fixture has a sha256 to remove");

        let temp_dir = temp_out_dir();
        let out_dir = temp_dir.path();
        // Simulate a manifest that would be rewritten and one that would be
        // removed as stale, had the command got that far.
        write(
            &out_dir.join("0.5.1").join(MANIFEST_FILE_NAME),
            &manifest("mukti", "0.5.1"),
        );
        write(
            &out_dir.join("0.4").join(MANIFEST_FILE_NAME),
            &manifest("mukti", "0.4.0"),
        );
        let before = read_tree(out_dir);

        let error = generate_install_manifests(&json, out_dir)
            .expect_err("a missing sha256 checksum is an error");
        let message = format!("{error:#}");
        assert!(
            message.contains("for version 0.5.3")
                && message.contains("no sha256 checksum for x86_64-unknown-linux-gnu.tar.gz")
                && message.contains("mukti-bin backfill-checksums"),
            "error: {message}",
        );
        assert_eq!(read_tree(out_dir), before, "nothing is written or removed");
    }

    #[test]
    fn stale_manifests_are_removed() {
        let mut json = fixture_json();
        let temp_dir = temp_out_dir();
        let out_dir = temp_dir.path();
        generate_install_manifests(&json, out_dir).expect("manifests written");
        assert!(out_dir.join("latest").join(MANIFEST_FILE_NAME).exists());

        // A stale manifest -- a spec-named directory holding this project's
        // manifest.
        let stale = out_dir.join("0.4").join(MANIFEST_FILE_NAME);
        write(&stale, &manifest("mukti", "0.4.0"));
        // Non-stale examples: wrong directory name, another project's manifest,
        // no header, and unrelated files.
        let odd_name = out_dir.join("backup").join(MANIFEST_FILE_NAME);
        write(&odd_name, &manifest("mukti", "0.4.0"));
        let other_project = out_dir.join("0.3").join(MANIFEST_FILE_NAME);
        write(&other_project, &manifest("another-project", "0.3.0"));
        let no_header = out_dir.join("0.2").join(MANIFEST_FILE_NAME);
        write(&no_header, "project mukti\n");
        let neighbor = out_dir.join("0.5").join("neighbor.txt");
        write(&neighbor, "keep me\n");
        let top_level = out_dir.join("top-level.txt");
        write(&top_level, "keep me\n");

        {
            let project = json.projects.get_mut("mukti").expect("fixture has mukti");
            set_status(project, "0.5.3", ReleaseStatus::Yanked);
            set_status(project, "0.5.1", ReleaseStatus::Yanked);
        }
        generate_install_manifests(&json, out_dir).expect("manifests rewritten");

        assert!(
            !out_dir.join("latest").exists(),
            "latest no longer resolves, so its directory is removed",
        );
        assert!(
            !out_dir.join("0.5").join(MANIFEST_FILE_NAME).exists(),
            "0.5 no longer resolves, so its manifest is removed",
        );
        assert!(
            !out_dir.join("0.4").exists(),
            "a stale manifest and its directory are removed"
        );
        assert!(
            neighbor.exists(),
            "an unrelated file in a stale directory is kept"
        );
        assert!(
            odd_name.exists(),
            "a manifest in a directory not named like a spec is kept"
        );
        assert!(other_project.exists(), "another project's manifest is kept");
        assert!(
            no_header.exists(),
            "a file without the manifest header is kept"
        );
        assert!(top_level.exists(), "a top-level file is kept");
        let yanked = fs::read_to_string(out_dir.join("0.5.3").join(MANIFEST_FILE_NAME))
            .expect("exact-version manifest still exists");
        assert!(yanked.contains("\nstatus yanked\n"), "manifest: {yanked}");
    }

    #[test]
    fn patch_range_shares_a_directory_with_its_version() {
        let mut json = fixture_json();
        let version: Version = "0.0.5".parse().expect("version parsed");
        {
            let project = json.projects.get_mut("mukti").expect("fixture has mukti");
            let (_, data) = project
                .get_version_data(&"0.5.3".parse().expect("version parsed"))
                .expect("0.5.3 is in the fixture");
            let data = data.clone();
            project.ranges.insert(
                VersionRange::from_version(&version),
                ReleaseRangeData {
                    latest: version.clone(),
                    is_prerelease: false,
                    versions: BTreeMap::from([(version, data)]),
                },
            );
        }
        let temp_dir = temp_out_dir();
        let out_dir = temp_dir.path();
        let path = out_dir.join("0.0.5").join(MANIFEST_FILE_NAME);

        generate_install_manifests(&json, out_dir).expect("manifests written");
        let active = fs::read_to_string(&path).expect("manifest for 0.0.5 read");
        assert!(
            active.contains("\nversion 0.0.5\nstatus active\n"),
            "manifest: {active}"
        );

        // Once yanked, the range no longer resolves and only the version is
        // left to keep the directory current.
        set_status(
            json.projects.get_mut("mukti").expect("fixture has mukti"),
            "0.0.5",
            ReleaseStatus::Yanked,
        );
        generate_install_manifests(&json, out_dir).expect("manifests rewritten");
        let yanked = fs::read_to_string(&path).expect("manifest for 0.0.5 is still there");
        assert!(yanked.contains("\nstatus yanked\n"), "manifest: {yanked}");
    }

    #[cfg(unix)]
    #[test]
    fn stale_removal_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;

        let json = fixture_json();
        let temp_dir = temp_out_dir();
        let out_dir = temp_dir.path().join("out");
        let elsewhere = temp_dir.path().join("elsewhere");

        // A spec-named symlink to a directory, and a spec-named directory whose
        // manifest is a symlink.
        let linked_dir_manifest = elsewhere.join("dir").join(MANIFEST_FILE_NAME);
        write(&linked_dir_manifest, &manifest("mukti", "0.4.0"));
        let linked_file = elsewhere.join("file.txt");
        write(&linked_file, &manifest("mukti", "0.3.0"));
        fs::create_dir_all(out_dir.join("0.3")).expect("directory created");
        symlink(elsewhere.join("dir"), out_dir.join("0.4")).expect("directory symlink created");
        symlink(&linked_file, out_dir.join("0.3").join(MANIFEST_FILE_NAME))
            .expect("file symlink created");

        generate_install_manifests(&json, &out_dir).expect("manifests written");

        assert!(
            linked_dir_manifest.exists(),
            "a manifest behind a directory symlink is kept"
        );
        assert!(
            linked_file.exists(),
            "the target of a manifest symlink is kept"
        );
        assert!(
            out_dir.join("0.4").is_symlink(),
            "the directory symlink is kept"
        );
        assert!(
            out_dir.join("0.3").join(MANIFEST_FILE_NAME).is_symlink(),
            "the manifest symlink is kept",
        );
    }

    #[test]
    fn no_versions_is_an_error_and_removes_nothing() {
        let mut json = fixture_json();
        json.projects
            .get_mut("mukti")
            .expect("fixture has mukti")
            .ranges
            .clear();
        let temp_dir = temp_out_dir();
        let out_dir = temp_dir.path();
        let existing = out_dir.join("0.5.3").join(MANIFEST_FILE_NAME);
        write(&existing, &manifest("mukti", "0.5.3"));

        let error =
            generate_install_manifests(&json, out_dir).expect_err("no versions is an error");
        assert!(
            error.to_string().contains("has no versions"),
            "error: {error}"
        );
        assert!(existing.exists(), "existing manifests are left alone");
    }

    #[test]
    fn whitespace_in_field_is_rejected() {
        let mut json = fixture_json();
        first_location(&mut json).url = "https://example.com/a b.tar.gz".to_owned();
        let temp_dir = temp_out_dir();
        let error =
            generate_install_manifests(&json, temp_dir.path()).expect_err("whitespace rejected");
        let message = format!("{error:#}");
        assert!(
            message.contains("for version 0.5.3") && message.contains("contains whitespace"),
            "error: {message}",
        );
    }

    #[test]
    fn malformed_sha256_is_rejected() {
        let mut json = fixture_json();
        first_location(&mut json)
            .checksums
            .insert(DigestAlgorithm::SHA256, Digest("abc123".to_owned()));
        let temp_dir = temp_out_dir();
        let error =
            generate_install_manifests(&json, temp_dir.path()).expect_err("bad sha256 rejected");
        let message = format!("{error:#}");
        assert!(
            message.contains("for version 0.5.3") && message.contains("not 64 hex characters"),
            "error: {message}",
        );
    }
}
