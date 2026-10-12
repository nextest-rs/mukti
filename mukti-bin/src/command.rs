// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    checksums::{backfill_checksums, fetch_release_checksums},
    errors::NameValueParseError,
    redirects::{generate_redirects, RedirectFlavor},
    release_json::{read_release_json, update_release_json, write_releases_json},
};
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use color_eyre::Result;
use semver::Version;
use std::str::FromStr;

#[doc(hidden)]
#[derive(Debug, Parser)]
#[clap(version)]
pub struct MuktiApp {
    #[clap(subcommand)]
    command: MuktiCommand,

    /// JSON file to edit
    #[clap(long, global = true, default_value = ".releases.json")]
    json: Utf8PathBuf,
}

#[derive(Debug, Subcommand)]
enum MuktiCommand {
    /// Add a release to the release JSON
    AddRelease {
        /// Release URL
        #[clap(long, required = true)]
        release_url: String,

        /// URL prefix to use
        #[clap(long, required = true)]
        archive_prefix: String,

        /// Version to publish
        #[clap(long = "version", required = true)]
        version: Version,

        /// Archive names.
        #[clap(long = "archive", value_name = "TARGET:FORMAT=NAME")]
        archives: Vec<Archive>,

        /// Number of release files to download in parallel.
        #[clap(long, short, default_value = "8")]
        jobs: usize,
    },
    /// Generate a _redirects file from the release JSON
    GenerateRedirects {
        /// Aliases to use.
        #[clap(long = "alias", value_name = "ALIAS=TARGET:FORMAT")]
        aliases: Vec<Alias>,

        /// The flavor of redirects to generate.
        #[clap(long, short, value_enum)]
        flavor: RedirectFlavor,

        /// Prefix for URLs.
        #[clap(long, default_value = "/")]
        prefix: String,

        /// Output directory.
        out_dir: Utf8PathBuf,
    },
    /// Add checksums to the release JSON
    BackfillChecksums {
        /// Number of release files to download in parallel.
        #[clap(long, short, default_value = "8")]
        jobs: usize,
    },
}

impl MuktiApp {
    pub async fn exec(self) -> Result<()> {
        match self.command {
            MuktiCommand::AddRelease {
                release_url,
                archive_prefix,
                version,
                archives,
                jobs,
            } => {
                let mut release_json = read_release_json(&self.json, true)?;

                let archives = fetch_release_checksums(&archive_prefix, archives, jobs).await?;

                update_release_json(
                    &mut release_json,
                    &release_url,
                    &version,
                    archives,
                    &self.json,
                )?;
            }
            MuktiCommand::GenerateRedirects {
                aliases,
                flavor,
                prefix,
                out_dir,
            } => {
                let release_json = read_release_json(&self.json, false)?;
                generate_redirects(&release_json, &aliases, flavor, &prefix, &out_dir)?;
            }
            MuktiCommand::BackfillChecksums { jobs } => {
                let mut release_json = read_release_json(&self.json, false)?;
                backfill_checksums(&mut release_json, jobs).await;
                write_releases_json(&release_json, &self.json)?;
            }
        }

        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Archive {
    pub(crate) target_format: TargetFormat,
    pub(crate) name: String,
}

impl FromStr for Archive {
    type Err = NameValueParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let (target_format, name) = name_value_parse(input, '=')?;
        let target_format: TargetFormat = target_format.parse()?;
        Ok(Self {
            target_format,
            name,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Alias {
    pub(crate) alias: String,
    pub(crate) target_format: TargetFormat,
}

impl FromStr for Alias {
    type Err = NameValueParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let (alias, target_format) = name_value_parse(input, '=')?;
        let target_format: TargetFormat = target_format.parse()?;
        Ok(Self {
            alias,
            target_format,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TargetFormat {
    pub(crate) target: String,
    pub(crate) format: String,
}

impl FromStr for TargetFormat {
    type Err = NameValueParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let (target, format) = name_value_parse(input, ':')?;
        Ok(Self { target, format })
    }
}

fn name_value_parse(input: &str, delimiter: char) -> Result<(String, String), NameValueParseError> {
    match input.split_once(delimiter) {
        Some((k, v)) => Ok((k.to_owned(), v.to_owned())),
        None => Err(NameValueParseError {
            input: input.to_owned(),
            delimiter,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8Path;
    use httptest::{matchers::request, responders::status_code, Expectation, Server};
    use mukti_metadata::{DigestAlgorithm, MuktiReleasesJson};

    static FIXTURE_JSON: &str = include_str!("../../fixtures/mukti-releases.json");
    const ARCHIVE_BODY: &str = "hello";
    const ARCHIVE_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
    const LINUX_ARCHIVE: &str = "mukti-bin-0.5.4-x86_64-unknown-linux-musl.tar.gz";
    const WINDOWS_ARCHIVE: &str = "mukti-bin-0.5.4-x86_64-pc-windows-msvc.zip";

    async fn add_release(json: &Utf8Path, archive_prefix: &str) -> Result<()> {
        let app = MuktiApp::try_parse_from([
            "mukti-bin",
            "--json",
            json.as_str(),
            "add-release",
            "--release-url",
            "https://example.com/releases/mukti-bin-0.5.4",
            "--archive-prefix",
            archive_prefix,
            "--version",
            "0.5.4",
            "--archive",
            &format!("x86_64-unknown-linux-musl:tar.gz={LINUX_ARCHIVE}"),
            "--archive",
            &format!("x86_64-pc-windows-msvc:zip={WINDOWS_ARCHIVE}"),
        ])
        .expect("arguments parsed");
        app.exec().await
    }

    fn expect_archive(server: &Server, name: &str) {
        server.expect(
            Expectation::matching(request::method_path("GET", format!("/{name}")))
                .respond_with(status_code(200).body(ARCHIVE_BODY)),
        );
    }

    #[tokio::test]
    async fn add_release_records_checksums() {
        let dir = camino_tempfile::tempdir().expect("temp dir created");
        let json = dir.path().join("releases.json");
        std::fs::write(&json, FIXTURE_JSON).expect("fixture JSON written");
        let server = Server::run();
        expect_archive(&server, LINUX_ARCHIVE);
        expect_archive(&server, WINDOWS_ARCHIVE);
        let prefix = format!("http://{}", server.addr());

        add_release(&json, &prefix).await.expect("release added");

        let contents = std::fs::read_to_string(&json).expect("releases JSON read");
        let releases: MuktiReleasesJson =
            serde_json::from_str(&contents).expect("releases JSON deserialized");
        let version: Version = "0.5.4".parse().expect("version parsed");
        let (_, data) = releases.projects["mukti"]
            .get_version_data(&version)
            .expect("0.5.4 was added");
        let recorded: Vec<_> = data
            .locations
            .iter()
            .map(|location| {
                (
                    location.url.clone(),
                    location.checksums[&DigestAlgorithm::SHA256].0.as_str(),
                )
            })
            .collect();
        assert_eq!(
            recorded,
            [
                (format!("{prefix}/{LINUX_ARCHIVE}"), ARCHIVE_SHA256),
                (format!("{prefix}/{WINDOWS_ARCHIVE}"), ARCHIVE_SHA256),
            ],
        );
    }

    #[tokio::test]
    async fn add_release_fails_if_an_archive_is_missing() {
        let dir = camino_tempfile::tempdir().expect("temp dir created");
        let json = dir.path().join("releases.json");
        std::fs::write(&json, FIXTURE_JSON).expect("fixture JSON written");
        let server = Server::run();
        expect_archive(&server, LINUX_ARCHIVE);
        // Each archive is tried three times.
        server.expect(
            Expectation::matching(request::method_path("GET", format!("/{WINDOWS_ARCHIVE}")))
                .times(3)
                .respond_with(status_code(404)),
        );
        let prefix = format!("http://{}", server.addr());

        let error = add_release(&json, &prefix)
            .await
            .expect_err("a missing archive fails the release");
        let message = format!("{error:#}");
        assert!(
            message.contains("failed to fetch 1 of 2 archive(s)")
                && message.contains(&format!("{prefix}/{WINDOWS_ARCHIVE}: "))
                && message.contains("404 Not Found"),
            "error: {message}",
        );
        assert_eq!(
            std::fs::read_to_string(&json).expect("releases JSON read"),
            FIXTURE_JSON,
            "the releases JSON is untouched",
        );
    }

    #[tokio::test]
    async fn add_release_fails_with_an_empty_prefix() {
        let dir = camino_tempfile::tempdir().expect("temp dir created");
        let json = dir.path().join("releases.json");

        let error = add_release(&json, "")
            .await
            .expect_err("relative archive URLs fail the release");
        let message = format!("{error:#}");
        assert!(
            message.contains("failed to fetch 2 of 2 archive(s)"),
            "error: {message}",
        );
        assert!(!json.exists(), "the releases JSON was not created");
    }
}
