// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

mod atomic;
mod checksums;
mod command;
mod errors;
mod glibc;
mod manifests;
mod redirects;
mod release_json;
mod specs;

use clap::Parser;
use color_eyre::Result;
use command::MuktiApp;

#[doc(hidden)]
#[tokio::main]
pub async fn main_impl() -> Result<()> {
    color_eyre::install()?;
    let app = MuktiApp::parse();
    app.exec().await
}
