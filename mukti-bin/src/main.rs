// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A simple script to update a releases.json file, and optionally a netlify.toml.

use clap::Parser;
use color_eyre::Result;
use mukti_bin::MuktiApp;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let app = MuktiApp::parse();
    app.exec().await
}
