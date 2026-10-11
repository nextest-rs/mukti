// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A duplicate of mukti-bin's `main.rs` for use in integration tests.

use color_eyre::Result;

fn main() -> Result<()> {
    mukti_bin::main_impl()
}
