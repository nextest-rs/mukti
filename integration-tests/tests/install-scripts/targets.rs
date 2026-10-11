// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    archive::ArchiveFormat,
    fixture::{BIN_NAME, BIN_NAME_EXE},
};
use std::fmt;

/// A release target.
///
/// This can be either a target triple or some other name like
/// `"universal-apple-darwin"`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Target(&'static str);

impl Target {
    pub(crate) const X86_64_UNKNOWN_LINUX_GNU: Self = Self("x86_64-unknown-linux-gnu");
    pub(crate) const X86_64_UNKNOWN_LINUX_MUSL: Self = Self("x86_64-unknown-linux-musl");
    pub(crate) const AARCH64_UNKNOWN_LINUX_GNU: Self = Self("aarch64-unknown-linux-gnu");
    pub(crate) const AARCH64_UNKNOWN_LINUX_MUSL: Self = Self("aarch64-unknown-linux-musl");
    pub(crate) const RISCV64GC_UNKNOWN_LINUX_GNU: Self = Self("riscv64gc-unknown-linux-gnu");
    pub(crate) const X86_64_PC_WINDOWS_MSVC: Self = Self("x86_64-pc-windows-msvc");
    pub(crate) const I686_PC_WINDOWS_MSVC: Self = Self("i686-pc-windows-msvc");
    pub(crate) const AARCH64_PC_WINDOWS_MSVC: Self = Self("aarch64-pc-windows-msvc");
    pub(crate) const UNIVERSAL_APPLE_DARWIN: Self = Self("universal-apple-darwin");
    pub(crate) const X86_64_UNKNOWN_FREEBSD: Self = Self("x86_64-unknown-freebsd");
    pub(crate) const X86_64_UNKNOWN_ILLUMOS: Self = Self("x86_64-unknown-illumos");

    // These are only named by the fake-tools cases, which are Unix-only.
    #[cfg(unix)]
    pub(crate) const AARCH64_APPLE_DARWIN: Self = Self("aarch64-apple-darwin");
    #[cfg(unix)]
    pub(crate) const X86_64_APPLE_DARWIN: Self = Self("x86_64-apple-darwin");

    // This is not a recognized triple at all.
    pub(crate) const MADE_UP: Self = Self("made-up-target");

    pub(crate) const fn as_str(self) -> &'static str {
        self.0
    }

    pub(crate) fn is_windows(self) -> bool {
        self.0.contains("-windows-")
    }

    pub(crate) fn bin_file_name(self) -> &'static str {
        if self.is_windows() {
            BIN_NAME_EXE
        } else {
            BIN_NAME
        }
    }

    pub(crate) fn archive_formats(self) -> &'static [ArchiveFormat] {
        if self.is_windows() {
            &[ArchiveFormat::TarGz, ArchiveFormat::Zip]
        } else {
            &[ArchiveFormat::TarGz]
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
