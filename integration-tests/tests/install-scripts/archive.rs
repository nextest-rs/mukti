// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Builds release archives in memory.

use byte_wrapper::HexArray;
use color_eyre::eyre::{Context, Result};
use eazip::{write::FileOptions, ArchiveWriter, CompressionMethod};
use flate2::{write::GzEncoder, Compression};
use sha2::Digest as _;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArchiveFormat {
    TarGz,
    Zip,
}

impl ArchiveFormat {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
            Self::Zip => "zip",
        }
    }

    /// Builds an archive in this format, holding one member.
    pub(crate) fn build(self, name: &str, contents: &[u8]) -> Result<Vec<u8>> {
        match self {
            Self::TarGz => build_tar_gz(name, contents),
            Self::Zip => build_zip(name, contents),
        }
    }
}

/// Returns a gzipped tar of executable members.
///
/// We don't use compression here because we only care about the output format.
fn build_tar_gz(name: &str, contents: &[u8]) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::none()));
    let mut header = tar::Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(0o755);
    header.set_mtime(0);
    builder
        .append_data(&mut header, name, contents)
        .wrap_err_with(|| format!("failed to add {name} to the tar archive"))?;
    let encoder = builder
        .into_inner()
        .wrap_err("failed to finish the tar archive")?;
    encoder
        .finish()
        .wrap_err("failed to finish the gzip stream")
}

/// Returns a zip of stored (uncompressed) members.
///
/// Currently, eazip doesn't support Unix permissions. So on platforms other
/// than Windows, the members are not extracted as executable.
fn build_zip(name: &str, contents: &[u8]) -> Result<Vec<u8>> {
    let mut options = FileOptions::default();
    options.compression_method = CompressionMethod::STORE;

    let mut writer = ArchiveWriter::new(Vec::new());
    writer
        .add_file(name, contents, &options)
        .wrap_err_with(|| format!("failed to add {name} to the zip archive"))?;
    writer.finish().wrap_err("failed to finish the zip archive")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Sha256(HexArray<32>);

impl Sha256 {
    pub(crate) fn of(bytes: &[u8]) -> Self {
        Self(HexArray::new(sha2::Sha256::digest(bytes).into()))
    }

    pub(crate) fn with_first_digit_changed(self) -> Self {
        let mut bytes = self.0.into_inner();
        bytes[0] ^= 0x10;
        Self(HexArray::new(bytes))
    }

    pub(crate) fn with_last_digit_changed(self) -> Self {
        let mut bytes = self.0.into_inner();
        bytes[31] ^= 0x01;
        Self(HexArray::new(bytes))
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}
