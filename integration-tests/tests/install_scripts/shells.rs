// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Information about the shells used by the tests.

use crate::fixture::PostInstallStep;
use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::eyre::{bail, Result};
use std::{collections::BTreeSet, env, fmt};

/// Environment variable containing a comma-separated list of optional shell
/// names that must be present.
///
/// This is set by CI so that tests aren't skipped by accident.
pub(crate) const REQUIRE_VAR: &str = "MUKTI_TEST_REQUIRE";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ShellKind {
    Sh,
    PowerShell,
}

impl ShellKind {
    // install.ps1 uses a zip file. Due to an eazip limitation, the zip's
    // members are only treated as executable on Windows.
    pub(crate) fn post_install_step(self) -> PostInstallStep {
        match self {
            Self::Sh => PostInstallStep::Configured,
            #[cfg(windows)]
            Self::PowerShell => PostInstallStep::Configured,
            #[cfg(not(windows))]
            Self::PowerShell => PostInstallStep::Omitted,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ShellName {
    Sh,
    Dash,
    Bash,
    Busybox,
    Ksh93,
    Pwsh,
    #[cfg(windows)]
    WindowsPowerShell,
}

impl ShellName {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Sh => "sh",
            Self::Dash => "dash",
            Self::Bash => "bash",
            Self::Busybox => "busybox",
            Self::Ksh93 => "ksh93",
            Self::Pwsh => "pwsh",
            #[cfg(windows)]
            Self::WindowsPowerShell => "powershell",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        SHELLS
            .iter()
            .map(|shell| shell.name)
            .find(|shell_name| shell_name.as_str() == name)
    }
}

#[cfg(unix)]
impl ShellName {
    /// Returns false if the shell may prefer its own built-in versions of
    /// external tools over the fake tools installed by the test setup.
    pub(crate) fn honors_path(self) -> bool {
        match self {
            Self::Sh | Self::Dash | Self::Bash | Self::Ksh93 | Self::Pwsh => true,
            // Debian's busybox uses its own applets over PATH. Alpine's does
            // not, and is covered there as plain sh.
            Self::Busybox => false,
        }
    }
}

impl fmt::Display for ShellName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ShellSpec {
    pub(crate) name: ShellName,
    pub(crate) args: &'static [&'static str],
    pub(crate) kind: ShellKind,
}

pub(crate) const SHELLS: &[ShellSpec] = &[
    ShellSpec {
        name: ShellName::Sh,
        args: &[],
        kind: ShellKind::Sh,
    },
    ShellSpec {
        name: ShellName::Dash,
        args: &[],
        kind: ShellKind::Sh,
    },
    ShellSpec {
        name: ShellName::Bash,
        args: &[],
        kind: ShellKind::Sh,
    },
    ShellSpec {
        name: ShellName::Busybox,
        args: &["ash"],
        kind: ShellKind::Sh,
    },
    // illumos /bin/sh is ksh93.
    ShellSpec {
        name: ShellName::Ksh93,
        args: &[],
        kind: ShellKind::Sh,
    },
    ShellSpec {
        name: ShellName::Pwsh,
        args: &[],
        kind: ShellKind::PowerShell,
    },
    #[cfg(windows)]
    ShellSpec {
        name: ShellName::WindowsPowerShell,
        args: &[],
        kind: ShellKind::PowerShell,
    },
];

/// If a shell is missing, whether we should treat it as a test failure or
/// ignore it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Requirement {
    Required,
    Optional,
}

#[derive(Clone, Debug)]
pub(crate) enum LocatedShell {
    Found(Utf8PathBuf),
    Missing(Requirement),
}

#[derive(Clone, Debug)]
pub(crate) struct Requirements {
    shells: BTreeSet<ShellName>,
}

impl Requirements {
    pub(crate) fn from_env() -> Result<Self> {
        let value = match env::var(REQUIRE_VAR) {
            Ok(value) => Some(value),
            Err(env::VarError::NotPresent) => None,
            Err(env::VarError::NotUnicode(value)) => {
                bail!("{REQUIRE_VAR} is not valid UTF-8: {value:?}");
            }
        };
        let names: Vec<&str> = value
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .collect();
        if value.is_some() && names.is_empty() {
            bail!(
                "{REQUIRE_VAR} is set, but does not name any shells; unset it to require none, \
                 or list the shells that must be installed (the tests on this platform use: {})",
                Self::known_names().join(", "),
            );
        }
        let mut shells = BTreeSet::new();
        for name in names {
            let Some(shell) = ShellName::from_name(name) else {
                bail!(
                    "{REQUIRE_VAR} names an unknown shell: {name} \
                     (the tests on this platform use: {})",
                    Self::known_names().join(", "),
                );
            };
            shells.insert(shell);
        }
        Ok(Self { shells })
    }

    fn known_names() -> Vec<&'static str> {
        SHELLS.iter().map(|shell| shell.name.as_str()).collect()
    }

    pub(crate) fn locate_shell(&self, shell: &ShellSpec) -> LocatedShell {
        let by_default = match shell.name {
            // Every Unix has sh, so it is not optional there.
            #[cfg(unix)]
            ShellName::Sh => Requirement::Required,
            #[cfg(not(unix))]
            ShellName::Sh => Requirement::Optional,
            ShellName::Dash
            | ShellName::Bash
            | ShellName::Busybox
            | ShellName::Ksh93
            | ShellName::Pwsh => Requirement::Optional,
            #[cfg(windows)]
            ShellName::WindowsPowerShell => Requirement::Optional,
        };
        let requirement = if self.shells.contains(&shell.name) {
            Requirement::Required
        } else {
            by_default
        };
        match find_program(shell.name.as_str()) {
            Some(path) => LocatedShell::Found(path),
            None => LocatedShell::Missing(requirement),
        }
    }
}

/// Looks the program up on PATH.
///
/// Done by hand because on Windows, we'd search System32 first and find the WSL
/// bash there.
pub(crate) fn find_program(program: &str) -> Option<Utf8PathBuf> {
    let path = env::var("PATH").ok()?;
    #[cfg(windows)]
    let separator = ';';
    #[cfg(not(windows))]
    let separator = ':';
    let file_name = format!("{program}{}", env::consts::EXE_SUFFIX);
    // TODO-RAINCLAUDE: make the result absolute, since scripts are run in
    // another directory, where a path found through a relative PATH entry
    // would not resolve. A relative entry can only fail to be made absolute if
    // the current directory is unusable, in which case nothing can be found
    // through it anyway.
    path.split(separator)
        .filter(|dir| !dir.is_empty())
        .filter_map(|dir| camino::absolute_utf8(Utf8Path::new(dir).join(&file_name)).ok())
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Utf8Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Utf8Path) -> bool {
    path.is_file()
}
