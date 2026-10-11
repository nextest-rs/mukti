// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The shells used by the tests, how to find them, and how they are set up to
//! run a script.

#[cfg(unix)]
use crate::fake_tools::FakeTools;
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

/// A shell that runs `install.sh`.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ShName {
    Sh,
    Dash,
    Bash,
    Busybox,
    Ksh93,
    Zsh,
}

impl ShName {
    /// Returns arguments that must come before the script.
    ///
    /// We need to do this to run busybox's sh applet, via e.g. `busybox ash`.
    pub(crate) const fn args(self) -> &'static [&'static str] {
        match self {
            Self::Busybox => &["ash"],
            Self::Sh | Self::Dash | Self::Bash | Self::Ksh93 | Self::Zsh => &[],
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Sh => "sh",
            Self::Dash => "dash",
            Self::Bash => "bash",
            Self::Busybox => "busybox",
            Self::Ksh93 => "ksh93",
            Self::Zsh => "zsh",
        }
    }
}

#[cfg(unix)]
impl ShName {
    /// Returns false if the shell may prefer its own built-in versions of
    /// external tools over the fake tools installed by the test setup.
    pub(crate) fn honors_path(self) -> bool {
        match self {
            Self::Sh | Self::Dash | Self::Bash | Self::Ksh93 | Self::Zsh => true,
            // Debian's busybox uses its own applets over PATH. Alpine's does
            // not, and is covered there as plain sh.
            Self::Busybox => false,
        }
    }

    /// Returns true if, when HOME is unset in the environment, it is also unset
    /// in the shell.
    ///
    /// zsh sets HOME from the password database when it is unset, so we return
    /// false for that scenario.
    pub(crate) fn keeps_home_unset(self) -> bool {
        match self {
            Self::Sh | Self::Dash | Self::Bash | Self::Busybox | Self::Ksh93 => true,
            Self::Zsh => false,
        }
    }
}

impl fmt::Display for ShName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A shell that runs `install.ps1`.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum PowerShellName {
    Pwsh,
    #[cfg(windows)]
    WindowsPowerShell,
}

impl PowerShellName {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Pwsh => "pwsh",
            #[cfg(windows)]
            Self::WindowsPowerShell => "powershell",
        }
    }
}

impl fmt::Display for PowerShellName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ShellName {
    Sh(ShName),
    PowerShell(PowerShellName),
}

impl ShellName {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Sh(shell) => shell.as_str(),
            Self::PowerShell(shell) => shell.as_str(),
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        SHELLS
            .iter()
            .copied()
            .find(|shell_name| shell_name.as_str() == name)
    }
}

impl fmt::Display for ShellName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub(crate) const SHELLS: &[ShellName] = &[
    ShellName::Sh(ShName::Sh),
    ShellName::Sh(ShName::Dash),
    ShellName::Sh(ShName::Bash),
    ShellName::Sh(ShName::Busybox),
    // illumos /bin/sh is ksh93.
    ShellName::Sh(ShName::Ksh93),
    // zsh is the login shell on macOS, so `curl ... | zsh` is plausible.
    ShellName::Sh(ShName::Zsh),
    ShellName::PowerShell(PowerShellName::Pwsh),
    #[cfg(windows)]
    ShellName::PowerShell(PowerShellName::WindowsPowerShell),
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
        SHELLS.iter().map(|shell| shell.as_str()).collect()
    }

    pub(crate) fn locate_shell(&self, shell: ShellName) -> LocatedShell {
        let by_default = match shell {
            // Every Unix has sh, so it is not optional there.
            #[cfg(unix)]
            ShellName::Sh(ShName::Sh) => Requirement::Required,
            #[cfg(not(unix))]
            ShellName::Sh(ShName::Sh) => Requirement::Optional,
            ShellName::Sh(
                ShName::Dash | ShName::Bash | ShName::Busybox | ShName::Ksh93 | ShName::Zsh,
            )
            | ShellName::PowerShell(PowerShellName::Pwsh) => Requirement::Optional,
            #[cfg(windows)]
            ShellName::PowerShell(PowerShellName::WindowsPowerShell) => Requirement::Optional,
        };
        let requirement = if self.shells.contains(&shell) {
            Requirement::Required
        } else {
            by_default
        };
        match find_program(shell.as_str()) {
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
    let path = env::var_os("PATH")?;
    let file_name = format!("{program}{}", env::consts::EXE_SUFFIX);
    env::split_paths(&path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .filter_map(|dir| Utf8PathBuf::try_from(dir).ok())
        // Entries in PATH can be relative (!). Make them absolute for
        // consistency.
        .filter_map(|dir| camino::absolute_utf8(dir.join(&file_name)).ok())
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

/// The ways in which install.sh can be started.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ShMode {
    /// Run the script file.
    ///
    /// For example, `sh install.sh --version 0.9`.
    File,
    /// Run the script file with each option and its value as one argument separated by `=`.
    ///
    /// For example, `sh install.sh --version=0.9`.
    FileWithEquals,
    /// Simulate `curl | sh`.
    ///
    /// This allows for script parameters to be passed in via environment
    /// variables, but not as CLI arguments.
    Piped,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum PowerShellMode {
    /// Run the script file with options, through the call operator, as someone
    /// who downloaded it would. For example:
    ///
    ///     powershell -Command "<trust setup>; & 'C:\...\install.ps1' -Version '0.9'"
    CallOperator,
    /// Run the script file with `powershell -File`. For example:
    ///
    ///     powershell -File C:\...\install.ps1 -Version 0.9
    ///
    /// In this scenario, there is no way to make PowerShell accept the test
    /// certificate.
    File,
    /// Simulate `irm | iex`.
    Piped,
    /// Fetch the script and invoke it as a script block with parameters.
    ScriptBlock,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Launch {
    Sh(ShMode),
    PowerShell(PowerShellMode),
}

/// A shell, and the way it is set up to run the script.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ShellLaunch {
    Sh(ShName, ShRun),
    PowerShell(PowerShellName, PowerShellRun),
}

impl ShellLaunch {
    pub(crate) fn kind(self) -> ShellKind {
        self.mode().kind()
    }

    pub(crate) fn mode(self) -> Launch {
        match self {
            Self::Sh(_, run) => Launch::Sh(run.mode),
            Self::PowerShell(_, run) => Launch::PowerShell(run.mode),
        }
    }
}

impl Launch {
    pub(crate) fn kind(self) -> ShellKind {
        match self {
            Self::Sh(_) => ShellKind::Sh,
            Self::PowerShell(_) => ShellKind::PowerShell,
        }
    }

    pub(crate) fn takes_options(self) -> bool {
        match self {
            Self::Sh(ShMode::File | ShMode::FileWithEquals)
            | Self::PowerShell(
                PowerShellMode::CallOperator | PowerShellMode::File | PowerShellMode::ScriptBlock,
            ) => true,
            Self::Sh(ShMode::Piped) | Self::PowerShell(PowerShellMode::Piped) => false,
        }
    }
}

/// Configuration for `install.sh` test cases.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ShRun {
    /// The mode to use when running the script.
    pub(crate) mode: ShMode,
    /// The platform (real or fake) configured by the test.
    pub(crate) platform: ShPlatform,
}

impl ShRun {
    pub(crate) const DEFAULT: Self = Self {
        mode: ShMode::File,
        platform: ShPlatform::Host,
    };
    pub(crate) const PIPED: Self = Self {
        mode: ShMode::Piped,
        ..Self::DEFAULT
    };
}

/// Configuration for `install.ps1` test cases.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PowerShellRun {
    /// The mode to use when running the script.
    pub(crate) mode: PowerShellMode,
    /// The platform (real or fake) configured by the test.
    pub(crate) platform: PowerShellPlatform,
}

impl PowerShellRun {
    #[cfg(windows)]
    pub(crate) const DEFAULT: Self = Self {
        mode: PowerShellMode::CallOperator,
        platform: PowerShellPlatform::Host,
    };
    // The install.ps1 script is only supported on Windows, but we can run tests
    // with it on other platforms by faking the Windows arch through
    // FakeWindowsArch's test hooks.
    #[cfg(not(windows))]
    pub(crate) const DEFAULT: Self = Self {
        mode: PowerShellMode::CallOperator,
        platform: PowerShellPlatform::FakeWindowsX64,
    };
    pub(crate) const FILE: Self = Self {
        mode: PowerShellMode::File,
        ..Self::DEFAULT
    };
    pub(crate) const PIPED: Self = Self {
        mode: PowerShellMode::Piped,
        ..Self::DEFAULT
    };
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ShPlatform {
    /// Use tools from the host's `PATH`.
    Host,

    /// Use a real host, but with `PROCESSOR_ARCHITECTURE` set to `x86` and
    /// `PROCESSOR_ARCHITEW6432` removed.
    ///
    /// This introduces a divergence between the environment (which indicates
    /// an x86 machine) and the registry (which has the correct information).
    /// A script that read the environment instead of the registry would pick
    /// the i686 build and fail.
    #[cfg(windows)]
    X86VarsOnHost,

    /// Use a real host and uname, and a fake architecture through
    /// `install.sh`'s test hooks.
    #[cfg(windows)]
    FakeArchOnHost(FakeWindowsArch),

    /// Use a constructed `PATH` with a fake host.
    ///
    /// This is Unix only because the constructed `PATH` is a directory of
    /// symlinks. Using symlinks or hard links is possible on Windows, but
    /// significantly trickier for a variety of reasons:
    ///
    /// * Creating symlinks requires developer mode or the requisite
    ///   privilege. GHA Windows has that privilege configured, but
    ///   developer machines might not be.
    /// * Configuring MINGW binaries requires also ensuring the shared
    ///   DLL is present.
    ///
    /// It is also less useful, because the PowerShell script doesn't use
    /// anything from `PATH` in this manner.
    #[cfg(unix)]
    Fake(FakeTools),
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum PowerShellPlatform {
    /// Use the host's platform.
    ///
    /// Because `install.ps1` only supports Windows, this will fail on
    /// non-Windows platforms (and is used for a test that fails on those
    /// platforms).
    Host,
    /// Fake x86_64 Windows in the script.
    ///
    /// This is used on non-Windows platforms.
    #[cfg(not(windows))]
    FakeWindowsX64,
    FakeArch(FakeWindowsArch),
}

impl PowerShellPlatform {
    pub(crate) fn env(self) -> Vec<(&'static str, Option<String>)> {
        match self {
            Self::Host => Vec::new(),
            #[cfg(not(windows))]
            Self::FakeWindowsX64 => FakeWindowsArch::X64.env().to_vec(),
            Self::FakeArch(arch) => arch.env().to_vec(),
        }
    }
}

/// Configures undocumented test hooks for architecture detection on Windows.
///
/// On Windows, we can't use real environment variables for this, since Windows
/// sets these env vars by itself for each process.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FakeWindowsArch {
    pub(crate) machine: &'static str,
    pub(crate) processor_architecture: Option<&'static str>,
    pub(crate) processor_architew6432: Option<&'static str>,
}

impl FakeWindowsArch {
    #[cfg(not(windows))]
    const X64: Self = Self::new("AMD64", "AMD64", None);

    pub(crate) const fn new(
        machine: &'static str,
        processor_architecture: &'static str,
        processor_architew6432: Option<&'static str>,
    ) -> Self {
        Self {
            machine,
            processor_architecture: Some(processor_architecture),
            processor_architew6432,
        }
    }

    pub(crate) fn env(&self) -> [(&'static str, Option<String>); 3] {
        [
            (
                "__MUKTI_TEST_MACHINE_ARCHITECTURE",
                Some(self.machine.to_owned()),
            ),
            (
                "__MUKTI_TEST_PROCESSOR_ARCHITECTURE",
                self.processor_architecture.map(str::to_owned),
            ),
            (
                "__MUKTI_TEST_PROCESSOR_ARCHITEW6432",
                self.processor_architew6432.map(str::to_owned),
            ),
        ]
    }
}
