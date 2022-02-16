// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    errors::NameValueParseError,
    netlify_toml::generate_netlify_redirects,
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
    #[clap(long, default_value = ".releases.json")]
    json: Utf8PathBuf,
}

#[derive(Debug, Subcommand)]
enum MuktiCommand {
    /// Add a release to the release JSON
    AddRelease {
        /// URL prefix to use
        #[clap(long = "url-prefix", required = true)]
        url_prefix: String,

        /// Version to publish
        #[clap(long = "version", required = true)]
        version: Version,

        /// Archive names.
        #[clap(long = "archive", value_name = "TARGET=NAME")]
        archives: Vec<Archive>,
    },
    /// Generate a netlify _redirects file from the release JSON
    GenerateNetlify {
        /// Aliases to use.
        #[clap(long = "alias", value_name = "ALIAS=TARGET")]
        aliases: Vec<Alias>,

        /// Prefix for URLs
        #[clap(long, default_value = "/")]
        netlify_prefix: String,

        /// Output directory.
        out_dir: Utf8PathBuf,
    },
}

impl MuktiApp {
    pub fn exec(self) -> Result<()> {
        let mut release_json = read_release_json(&self.json)?;

        match self.command {
            MuktiCommand::AddRelease {
                url_prefix,
                version,
                archives,
            } => {
                update_release_json(
                    &mut release_json,
                    &url_prefix,
                    &version,
                    &archives,
                    &self.json,
                )?;
            }
            MuktiCommand::GenerateNetlify {
                aliases,
                netlify_prefix,
                out_dir,
            } => {
                generate_netlify_redirects(&release_json, &aliases, &netlify_prefix, &out_dir)?;
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct Archive {
    pub(crate) target: String,
    pub(crate) name: String,
}

impl FromStr for Archive {
    type Err = NameValueParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let (target, name) = name_value_parse(input)?;
        Ok(Self { target, name })
    }
}

#[derive(Debug)]
pub(crate) struct Alias {
    pub(crate) alias: String,
    pub(crate) target: String,
}

impl FromStr for Alias {
    type Err = NameValueParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let (alias, target) = name_value_parse(input)?;
        Ok(Self { alias, target })
    }
}

fn name_value_parse(input: &str) -> Result<(String, String), NameValueParseError> {
    match input.split_once('=') {
        Some((k, v)) => Ok((k.to_owned(), v.to_owned())),
        None => Err(NameValueParseError {
            input: input.to_owned(),
        }),
    }
}
