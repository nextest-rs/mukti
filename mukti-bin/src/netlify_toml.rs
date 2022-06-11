// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::command::Alias;
use atomicwrites::{AtomicFile, OverwriteBehavior};
use camino::Utf8Path;
use color_eyre::eyre::{bail, Context, Result};
use mukti_metadata::{MuktiReleasesJson, ReleaseVersionData};
use std::{fmt::Write as _, io::Write as _};

pub(crate) fn generate_netlify_redirects(
    release_json: &MuktiReleasesJson,
    aliases: &[Alias],
    netlify_prefix: &str,
    out_dir: &Utf8Path,
) -> Result<()> {
    if release_json.projects.len() != 1 {
        bail!(
            "mukti-bin currently only supports one project, {} found",
            release_json.projects.len()
        );
    }

    let project = release_json
        .projects
        .values()
        .next()
        .expect("release_json has one project");

    let netlify_prefix = netlify_prefix.trim_end_matches('/');
    let mut out = String::with_capacity(4096);

    writeln!(&mut out, "# Generated by mukti\n")?;

    if let Some(range) = &project.latest {
        let latest_range_data = &project.ranges[range];
        let latest_version_data = &latest_range_data.versions[&latest_range_data.latest];
        write_entries(
            &"latest",
            latest_version_data,
            aliases,
            netlify_prefix,
            &mut out,
        );
    }

    for (range, data) in &project.ranges {
        if !data.is_prerelease {
            let version_data = &data.versions[&data.latest];
            write_entries(range, version_data, aliases, netlify_prefix, &mut out);
        }
        for (version, version_data) in &data.versions {
            write_entries(version, version_data, aliases, netlify_prefix, &mut out);
        }
    }

    let file = AtomicFile::new(
        out_dir.join("_redirects"),
        OverwriteBehavior::AllowOverwrite,
    );
    file.write(|f| f.write_all(out.as_bytes()))
        .wrap_err("failed to write _redirects")?;

    Ok(())
}

fn write_entries(
    version: &dyn std::fmt::Display,
    version_data: &ReleaseVersionData,
    aliases: &[Alias],
    netlify_prefix: &str,
    out: &mut String,
) {
    writeln!(
        out,
        "{}/{}/release {} 302",
        netlify_prefix, version, &version_data.release_url
    )
    .expect("writing to a string is infallible");
    for location in &version_data.locations {
        writeln!(
            out,
            "{}/{}/{}.{} {} 302",
            netlify_prefix, version, location.target, location.format, location.url
        )
        .expect("writing to a string is infallible");
        for alias in aliases.iter().filter(|alias| {
            alias.target_format.target == location.target
                && alias.target_format.format == location.format
        }) {
            writeln!(
                out,
                "{}/{}/{} {} 302",
                netlify_prefix, version, alias.alias, location.url
            )
            .expect("writing to a string is infallible");
        }
    }
}