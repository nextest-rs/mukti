// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    errors::NameValueParseError,
    redirects::{generate_redirects, RedirectFlavor},
    release_json::{read_release_json, update_release_json},
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
}

impl MuktiApp {
    pub fn exec(self) -> Result<()> {
        match self.command {
            MuktiCommand::AddRelease {
                release_url,
                archive_prefix,
                version,
                archives,
            } => {
                let mut release_json = read_release_json(&self.json, true)?;
                update_release_json(
                    &mut release_json,
                    &release_url,
                    &archive_prefix,
                    &version,
                    &archives,
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
        }

        Ok(())
    }
}

#[derive(Debug)]
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

#[derive(Debug)]
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

#[derive(Debug)]
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
