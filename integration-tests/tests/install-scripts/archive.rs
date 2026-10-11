// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Builds release archives in memory.

use byte_wrapper::HexArray;
use color_eyre::eyre::{Context, Result};
use eazip::{write::FileOptions, ArchiveWriter, CompressionMethod};
use flate2::{write::GzEncoder, Compression};
use mukti_metadata::Digest;
use sha2::Digest as _;
use std::{fmt, io::Write};

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

    pub(crate) fn build(self, members: &[Member<'_>]) -> Result<Vec<u8>> {
        match self {
            Self::TarGz => build_tar_gz(members),
            Self::Zip => build_zip(members),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Member<'a> {
    File { name: &'a str, contents: &'a [u8] },
    Directory { name: &'a str },
}

pub(crate) fn gzip(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::none());
    encoder
        .write_all(bytes)
        .and_then(|()| encoder.finish())
        .wrap_err("failed to gzip bytes")
}

/// Returns a tar of executable members in gzip framing.
///
/// The gzip stream uses compression level none, because we only care about the
/// output format.
fn build_tar_gz(members: &[Member<'_>]) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::none()));
    for member in members {
        let (name, contents, entry_type): (_, &[u8], _) = match *member {
            Member::File { name, contents } => (name, contents, tar::EntryType::Regular),
            Member::Directory { name } => (name, &[], tar::EntryType::Directory),
        };
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(entry_type);
        header.set_size(contents.len() as u64);
        header.set_mode(0o755);
        header.set_mtime(0);
        builder
            .append_data(&mut header, name, contents)
            .wrap_err_with(|| format!("failed to add {name} to the tar archive"))?;
    }
    let encoder = builder
        .into_inner()
        .wrap_err("failed to finish the tar archive")?;
    encoder
        .finish()
        .wrap_err("failed to finish the gzip stream")
}

/// Returns a zip of stored (uncompressed) members.
fn build_zip(members: &[Member<'_>]) -> Result<Vec<u8>> {
    let options = FileOptions::default()
        .compression(CompressionMethod::STORE, None)
        .unix_mode(0o755);

    let mut writer = ArchiveWriter::new(Vec::new());
    for member in members {
        match *member {
            Member::File { name, contents } => writer
                .add_file(name, contents, &options)
                .wrap_err_with(|| format!("failed to add {name} to the zip archive"))?,
            Member::Directory { name } => writer
                .add_directory(&format!("{name}/"))
                .wrap_err_with(|| format!("failed to add {name}/ to the zip archive"))?,
        }
    }
    writer.finish().wrap_err("failed to finish the zip archive")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Sha256(HexArray<32>);

impl Sha256 {
    pub(crate) fn of(bytes: &[u8]) -> Self {
        Self(HexArray::new(sha2::Sha256::digest(bytes).into()))
    }

    pub(crate) fn to_digest(self) -> Digest {
        Digest::new(self.0.into_inner().to_vec())
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
