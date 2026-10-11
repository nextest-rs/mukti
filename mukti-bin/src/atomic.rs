// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use atomicwrites::{AtomicFile, OverwriteBehavior};
use camino::Utf8Path;
use color_eyre::eyre::{Context, Result};
use std::{fs, io::Write as _};

/// Creates the parent directory if needed, then writes the file atomically.
pub(crate) fn write_atomic(path: &Utf8Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_str().is_empty()) {
        fs::create_dir_all(parent)
            .wrap_err_with(|| format!("failed to create directory {parent} for {path}"))?;
    }
    AtomicFile::new(path, OverwriteBehavior::AllowOverwrite)
        .write(|f| f.write_all(contents))
        .wrap_err_with(|| format!("failed to write {path}"))
}
