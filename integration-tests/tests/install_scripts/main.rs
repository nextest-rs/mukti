// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! End-to-end tests for generated install scripts.

mod archive;
mod cases;
#[cfg(unix)]
mod fake_tools;
mod fixture;
mod hang_up;
mod host;
mod runner;
mod server;
mod shells;
mod standalone;
mod targets;
mod tls;

use libtest_mimic::Arguments;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = Arguments::from_args();
    match runner::trials() {
        Ok(trials) => libtest_mimic::run(&args, trials).exit_code(),
        Err(error) => {
            eprintln!("error: {error:?}");
            ExitCode::FAILURE
        }
    }
}
