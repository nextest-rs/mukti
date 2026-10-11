// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cases that don't need a constructed `PATH`.

#[cfg(unix)]
use crate::fake_tools::FakeTools;
use crate::{
    fixture::{
        ArchiveContent, ArchiveServing, InstallDirSource, ManifestChecksum, ManifestTweak, Mode,
        PowerShellMode, Release, VersionChoice, PROJECT,
    },
    targets::Target,
};
use mukti_metadata::ReleaseStatus;

/// Represents a typical set of release targets.
///
/// This list is grabbed from nextest 0.9.146.
const ALL_TARGETS: &[Target] = &[
    Target::X86_64_UNKNOWN_LINUX_GNU,
    Target::X86_64_UNKNOWN_LINUX_MUSL,
    Target::AARCH64_UNKNOWN_LINUX_GNU,
    Target::AARCH64_UNKNOWN_LINUX_MUSL,
    Target::RISCV64GC_UNKNOWN_LINUX_GNU,
    Target::X86_64_PC_WINDOWS_MSVC,
    Target::I686_PC_WINDOWS_MSVC,
    Target::AARCH64_PC_WINDOWS_MSVC,
    Target::UNIVERSAL_APPLE_DARWIN,
    Target::X86_64_UNKNOWN_FREEBSD,
    Target::X86_64_UNKNOWN_ILLUMOS,
];

pub(crate) const RELEASE: Release = Release {
    project: PROJECT,
    version: "0.9.148",
    status: ReleaseStatus::Active,
    targets: ALL_TARGETS,
    checksum: ManifestChecksum::Correct,
};

/// Configuration for `install.sh` test cases.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ShRun {
    /// The mode to use when running the script.
    pub(crate) mode: Mode,
    /// The platform (real or fake) configured by the test.
    pub(crate) platform: ShPlatform,
}

impl ShRun {
    pub(crate) const DEFAULT: Self = Self {
        mode: Mode::Args,
        platform: ShPlatform::DEFAULT,
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
    pub(crate) const DEFAULT: Self = Self {
        mode: PowerShellMode::Args,
        platform: PowerShellPlatform::DEFAULT,
    };
    pub(crate) const FILE: Self = Self {
        mode: PowerShellMode::File,
        platform: PowerShellPlatform::DEFAULT,
    };
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum TargetChoice {
    /// The host platform.
    Host,
    /// The script should use this target because it is passed into it.
    Override(Target),
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ShPlatform {
    /// Use tools from the host's `PATH`.
    Real(TargetChoice),

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

impl ShPlatform {
    pub(crate) const DEFAULT: Self = Self::Real(TargetChoice::Host);
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum PowerShellPlatform {
    Real(TargetChoice),
    FakeArch {
        arch: FakeWindowsArch,
        expected_target: Option<Target>,
    },
}

impl PowerShellPlatform {
    #[cfg(windows)]
    pub(crate) const DEFAULT: Self = Self::Real(TargetChoice::Host);
    /// On platforms other than Windows, `install.ps1` cannot detect the target,
    /// so we've got to provide a name.
    #[cfg(not(windows))]
    pub(crate) const DEFAULT: Self =
        Self::Real(TargetChoice::Override(Target::X86_64_PC_WINDOWS_MSVC));
}

#[cfg(windows)]
const NO_OPTIONS_POWERSHELL_PLATFORM: PowerShellPlatform = PowerShellPlatform::DEFAULT;
// TODO-RAINCLAUDE: off Windows, install.ps1 cannot detect a target; the test
// hook for the machine's architecture stands in, since a target override
// would be an option.
#[cfg(not(windows))]
const NO_OPTIONS_POWERSHELL_PLATFORM: PowerShellPlatform = PowerShellPlatform::FakeArch {
    arch: FakeWindowsArch::new("AMD64", "AMD64", None),
    expected_target: Some(Target::X86_64_PC_WINDOWS_MSVC),
};

// TODO-RAINCLAUDE: the documented commands (curl | sh, and irm | iex), with
// nothing set: no version, base URL, install directory, or target, by option
// or environment variable.
const NO_OPTIONS: Case = Case {
    name: "no-options",
    sh: Some(ShRun {
        mode: Mode::Piped,
        ..ShRun::DEFAULT
    }),
    powershell: Some(PowerShellRun {
        mode: PowerShellMode::Piped,
        platform: NO_OPTIONS_POWERSHELL_PLATFORM,
    }),
    base_url: BaseUrlChoice::Default,
    install_dir: InstallDirSource::Home,
    ..BASE
};

/// The state of the destination before the script runs.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Existing {
    Nothing,
    OldBinary,
    /// An old binary held open by another program.
    ///
    /// This (of course) is Windows-only.
    #[cfg(windows)]
    LockedOldBinary,
    /// An empty directory with the same name as the binary.
    Directory,
}

/// The expected behavior of the script under the provided configuration.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Expect {
    FailsBeforeManifest,
    FailsAfterManifest,
    /// Requests the archive, then fails and leaves the destination as it was.
    FailsOnArchive {
        serving: ArchiveServing,
    },
    /// Installs the binary, but then fails in the post-install step.
    FailsPostInstall {
        helper_exit: u8,
    },
    /// Installs a file, then fails because the binary could not be started at
    /// all.
    ///
    /// In our tests, we simulate it with a shebang line with a non-existent
    /// interpreter. In reality this is most likely something like a glibc
    /// binary on a musl system.
    #[cfg(unix)]
    FailsToStartPostInstall,
    Installed {
        serving: ArchiveServing,
        post_install: PostInstall,
    },
}

/// Whether an install runs the post-install step.
///
/// A release older than the step's minimum version is installed without it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PostInstall {
    Runs,
    SkippedBelowMinimumVersion,
}

impl Expect {
    /// Returns the expected way in which archives are served.
    ///
    /// Returns `None` if the script should not get as far as asking for the archive.
    pub(crate) fn archive_serving(self) -> Option<ArchiveServing> {
        match self {
            Self::FailsBeforeManifest | Self::FailsAfterManifest => None,
            Self::FailsOnArchive { serving } | Self::Installed { serving, .. } => Some(serving),
            Self::FailsPostInstall { .. } => Some(ArchiveServing::Normal),
            #[cfg(unix)]
            Self::FailsToStartPostInstall => Some(ArchiveServing::Normal),
        }
    }

    pub(crate) fn content(self) -> ArchiveContent {
        match self {
            Self::FailsBeforeManifest
            | Self::FailsAfterManifest
            | Self::FailsOnArchive { .. }
            | Self::FailsPostInstall { .. }
            | Self::Installed { .. } => ArchiveContent::Helper,
            #[cfg(unix)]
            Self::FailsToStartPostInstall => ArchiveContent::Unrunnable,
        }
    }
}

/// The base URL option passed to the script, in terms of the test server's URL.
#[derive(Clone, Copy, Debug)]
pub(crate) enum BaseUrlChoice {
    Default,
    Server,
    WithSuffix(&'static str),
    /// The server's host and port with the https:// taken off.
    WithoutScheme,
    PlainHttp,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Case {
    /// A prefix for the test name, for cases that belong together.
    pub(crate) group: Option<&'static str>,
    pub(crate) name: &'static str,
    pub(crate) sh: Option<ShRun>,
    pub(crate) powershell: Option<PowerShellRun>,
    pub(crate) release: Release,
    pub(crate) tweak: ManifestTweak,

    pub(crate) version: VersionChoice,
    pub(crate) base_url: BaseUrlChoice,
    pub(crate) install_dir: InstallDirSource,
    pub(crate) existing: Existing,
    pub(crate) env: Vars,
    pub(crate) expect: Expect,
    /// Text that must appear in the script's progress stream.
    ///
    /// This and `error_contains` below are named for the text rather than for
    /// stdout and stderr, because PowerShell's semantics are somewhat different
    /// here. With `install.sh`, progress and error text both go to stderr, but
    /// with `install.ps1` info lines are written via `Write-Information` (which
    /// goes to stdout when run as a subprocess) and errors are thrown (which
    /// goes to stderr).
    pub(crate) progress_contains: &'static [&'static str],
    /// Text that must appear in error output.
    pub(crate) error_contains: &'static [&'static str],
    /// Text that must not appear in the script's progress or error output.
    pub(crate) output_excludes: &'static [&'static str],
}

impl Case {
    pub(crate) fn test_name(&self) -> String {
        match self.group {
            Some(group) => format!("{group}::{}", self.name),
            None => self.name.to_owned(),
        }
    }
}

const INSTALLED: Expect = Expect::Installed {
    serving: ArchiveServing::Normal,
    post_install: PostInstall::Runs,
};

pub(crate) const FAILS_ON_ARCHIVE: Expect = Expect::FailsOnArchive {
    serving: ArchiveServing::Normal,
};

pub(crate) const BASE: Case = Case {
    group: None,
    name: "",
    sh: Some(ShRun::DEFAULT),
    powershell: Some(PowerShellRun::DEFAULT),
    release: RELEASE,
    tweak: ManifestTweak::None,
    version: VersionChoice::Default,
    base_url: BaseUrlChoice::Server,
    install_dir: InstallDirSource::Explicit,
    existing: Existing::Nothing,
    env: &[],
    expect: INSTALLED,
    progress_contains: &[],
    error_contains: &[],
    output_excludes: &[],
};

/// Applies to shells whose script is generated with a post-install step.
///
/// This includes sh anywhere and PowerShell/Pwsh on Windows. (Why not Pwsh
/// on other platforms? For a somewhat silly reason -- because the zip
/// library we use, eazip, doesn't currently support setting Unix
/// permissions.)
#[cfg(windows)]
const WITH_POST_INSTALL: Case = BASE;
#[cfg(not(windows))]
const WITH_POST_INSTALL: Case = Case {
    powershell: None,
    ..BASE
};

pub(crate) type Vars = &'static [(&'static str, Option<&'static str>)];

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

/// Printed when the architecture of the shell's own process differs from the
/// machine's, as "PowerShell is an ... process on an ... machine" or "the shell
/// is an ... process on an ... machine". When they match, this must _not_ be
/// printed.
const ARCHITECTURE_NOTE: &[&str] = &[" process on an "];

pub(crate) fn windows_architecture_cases(
    base: impl Fn(FakeWindowsArch, Target) -> Case,
) -> Vec<Case> {
    let row = |name: &'static str, arch: FakeWindowsArch, target: Target| Case {
        group: Some("detect"),
        name,
        ..base(arch, target)
    };

    vec![
        Case {
            output_excludes: ARCHITECTURE_NOTE,
            ..row(
                "windows-amd64",
                FakeWindowsArch::new("AMD64", "AMD64", None),
                Target::X86_64_PC_WINDOWS_MSVC,
            )
        },
        Case {
            output_excludes: ARCHITECTURE_NOTE,
            ..row(
                "windows-x86",
                FakeWindowsArch::new("x86", "x86", None),
                Target::I686_PC_WINDOWS_MSVC,
            )
        },
        // These rows have an unrecognized machine-level value, so the process's
        // variables decide.
        Case {
            progress_contains: &["is an x86 process on an AMD64 machine; preferring AMD64 builds"],
            ..row(
                "windows-machine-lookup-returns-garbage-in-x86-process",
                FakeWindowsArch::new("PDP11", "x86", Some("AMD64")),
                Target::X86_64_PC_WINDOWS_MSVC,
            )
        },
        Case {
            output_excludes: ARCHITECTURE_NOTE,
            ..row(
                "windows-machine-lookup-returns-garbage",
                FakeWindowsArch::new("PDP11", "ARM64", None),
                Target::AARCH64_PC_WINDOWS_MSVC,
            )
        },
        Case {
            release: Release {
                targets: &[Target::X86_64_PC_WINDOWS_MSVC, Target::I686_PC_WINDOWS_MSVC],
                ..RELEASE
            },
            progress_contains: &["preferring ARM64 builds"],
            ..row(
                "windows-x64-process-on-arm64-falls-back-to-x86_64",
                FakeWindowsArch::new("ARM64", "AMD64", None),
                Target::X86_64_PC_WINDOWS_MSVC,
            )
        },
    ]
}

/// Returns the cases that don't need fake tools -- most run against the real
/// host.
pub(crate) fn common_cases() -> Vec<Case> {
    let mut cases = vec![
        // TODO-RAINCLAUDE: every option given through its environment
        // variable, each with a value other than the default: the version,
        // the base URL, the install directory and, for sh, the target.
        // (PowerShell cases already pass a target off Windows.)
        Case {
            name: "piped-with-env-options",
            sh: Some(ShRun {
                mode: Mode::Piped,
                platform: ShPlatform::Real(TargetChoice::Override(Target::X86_64_UNKNOWN_FREEBSD)),
            }),
            powershell: Some(PowerShellRun {
                mode: PowerShellMode::Piped,
                ..PowerShellRun::DEFAULT
            }),
            version: VersionChoice::Explicit("0.9"),
            ..BASE
        },
        Case {
            name: "piped-failure",
            sh: None,
            powershell: Some(PowerShellRun {
                mode: PowerShellMode::Piped,
                ..PowerShellRun::DEFAULT
            }),
            version: VersionChoice::Explicit("9.9.9"),
            expect: Expect::FailsAfterManifest,
            error_contains: &["widget-project version \"9.9.9\" was not found"],
            ..BASE
        },
        Case {
            name: "install-dir-from-cargo-home",
            install_dir: InstallDirSource::CargoHome,
            ..BASE
        },
        NO_OPTIONS,
        // A PowerShell session has two current directories:
        //
        // 1. PowerShell's own location (`$PWD`). This is changed when
        //    Set-Location changes.
        // 2. The process's working directory, which Set-Location does not
        //    change, and which .NET APIs such as [IO.Directory]::CreateDirectory
        //    resolve relative paths against.
        //
        // A relative install directory is relative to 1, not 2, so install.ps1
        // turns the path into an absolute one before passing it to .NET.
        //
        // TODO-RAINCLAUDE: the next paragraph is out of date in one respect.
        // The default launch is no longer `-File`: it runs the script file
        // through the call operator inside `-Command`. That launch also
        // leaves the two the same, so the conclusion holds.
        //
        // The two are the same unless something changes one of them before the
        // script runs. With the default `-File` launch, there's no room for
        // anything like that, so this case would pass even if `install.ps1`
        // skipped that step. With a `-Command` launch the caller can call
        // `Set-Location` first (and we do, in `Fixture::command`), which is why
        // this case uses the script block mode.
        Case {
            name: "install-dir-relative",
            powershell: Some(PowerShellRun {
                mode: PowerShellMode::ScriptBlock,
                ..PowerShellRun::DEFAULT
            }),
            install_dir: InstallDirSource::ExplicitRelative,
            ..BASE
        },
        // This can't be used on Windows since the MSYS and Cygwin shells set
        // HOME themselves when it is unset.
        #[cfg(not(windows))]
        Case {
            name: "install-dir-unknown",
            powershell: None,
            install_dir: InstallDirSource::Unset,
            expect: Expect::FailsBeforeManifest,
            error_contains: &["neither CARGO_HOME nor HOME is set", "--install-dir"],
            ..BASE
        },
        Case {
            name: "install-dir-with-brackets",
            sh: None,
            powershell: Some(PowerShellRun::FILE),
            install_dir: InstallDirSource::ExplicitWithBrackets,
            expect: Expect::FailsBeforeManifest,
            error_contains: &[
                "which PowerShell treats as wildcards",
                "-InstallDir or $env:WIDGET_INSTALL_DIR",
            ],
            ..BASE
        },
        Case {
            name: "replaces-existing-binary",
            existing: Existing::OldBinary,
            ..BASE
        },
        #[cfg(windows)]
        Case {
            name: "locked-existing-binary",
            sh: None,
            existing: Existing::LockedOldBinary,
            expect: FAILS_ON_ARCHIVE,
            error_contains: &["could not move the existing", "nothing was changed"],
            ..BASE
        },
        Case {
            name: "destination-is-a-directory",
            existing: Existing::Directory,
            expect: FAILS_ON_ARCHIVE,
            error_contains: &["cannot install to", "it is a directory"],
            ..BASE
        },
        Case {
            name: "equals-sign-options",
            sh: Some(ShRun {
                mode: Mode::ArgsWithEquals,
                ..ShRun::DEFAULT
            }),
            powershell: None,
            version: VersionChoice::Explicit("0.9"),
            ..BASE
        },
        // TODO-RAINCLAUDE: an option passed as an argument wins over its
        // environment variable. For example, sh runs the script as:
        //
        //     WIDGET_INSTALL_VERSION=9.9.9 sh install.sh --version latest ...
        //
        // The server has a manifest for latest and none for 9.9.9, so the
        // install can only succeed if the argument was the one used.
        Case {
            name: "option-overrides-env",
            version: VersionChoice::Explicit("latest"),
            env: &[("WIDGET_INSTALL_VERSION", Some("9.9.9"))],
            ..BASE
        },
        Case {
            name: "target-override",
            sh: Some(ShRun {
                platform: ShPlatform::Real(TargetChoice::Override(Target::X86_64_UNKNOWN_FREEBSD)),
                ..ShRun::DEFAULT
            }),
            powershell: None,
            ..BASE
        },
        Case {
            name: "base-url-with-query",
            powershell: Some(PowerShellRun::FILE),
            base_url: BaseUrlChoice::WithSuffix("?mirror=1"),
            expect: Expect::FailsBeforeManifest,
            error_contains: &["invalid base URL", "must not contain"],
            ..BASE
        },
        Case {
            name: "base-url-with-trailing-slash",
            base_url: BaseUrlChoice::WithSuffix("/"),
            ..BASE
        },
        Case {
            name: "base-url-without-scheme",
            powershell: Some(PowerShellRun::FILE),
            base_url: BaseUrlChoice::WithoutScheme,
            expect: Expect::FailsBeforeManifest,
            error_contains: &["invalid base URL", "must start with"],
            ..BASE
        },
        Case {
            name: "base-url-plain-http",
            powershell: Some(PowerShellRun::FILE),
            base_url: BaseUrlChoice::PlainHttp,
            expect: Expect::FailsBeforeManifest,
            error_contains: &["invalid base URL: http://", "must start with \"https://\""],
            ..BASE
        },
        Case {
            name: "yanked-exact-version",
            release: Release {
                status: ReleaseStatus::Yanked,
                ..RELEASE
            },
            version: VersionChoice::Explicit("0.9.148"),
            progress_contains: &["widget-project 0.9.148 has been yanked; installing it anyway"],
            ..BASE
        },
        Case {
            group: Some("post-install"),
            name: "failure",
            expect: Expect::FailsPostInstall { helper_exit: 3 },
            error_contains: &[
                "widget-project 0.9.148 was installed to",
                "the post-install step (widget self setup) failed with exit code 3",
                "the binary is in place",
            ],
            ..WITH_POST_INSTALL
        },
        // The binary cannot be started at all.
        //
        // The installed file cannot be exec'd (its shebang line has a missing
        // interpreter), so the status comes from the shell.
        #[cfg(unix)]
        Case {
            group: Some("post-install"),
            name: "binary-cannot-be-started",
            expect: Expect::FailsToStartPostInstall,
            // TODO-RAINCLAUDE: the text stops at "12" on purpose: a shell may
            // report either 126 or 127 for a missing interpreter.
            error_contains: &[
                "widget-project 0.9.148",
                "but it cannot run on this system (exit code 12",
                "pass --target <TRIPLE>",
            ],
            ..WITH_POST_INSTALL
        },
        Case {
            name: "nonexistent-version",
            version: VersionChoice::Explicit("9.9.9"),
            expect: Expect::FailsAfterManifest,
            error_contains: &["widget-project version \"9.9.9\" was not found"],
            ..BASE
        },
        Case {
            name: "invalid-version",
            powershell: Some(PowerShellRun::FILE),
            version: VersionChoice::Explicit("a$b"),
            expect: Expect::FailsBeforeManifest,
            error_contains: &["invalid version"],
            ..BASE
        },
        Case {
            name: "truncated-archive",
            expect: Expect::FailsOnArchive {
                serving: ArchiveServing::Truncated,
            },
            error_contains: &["checksum mismatch", "nothing was installed"],
            ..BASE
        },
        Case {
            name: "archive-behind-redirect",
            expect: Expect::Installed {
                serving: ArchiveServing::Redirect,
                post_install: PostInstall::Runs,
            },
            ..BASE
        },
        Case {
            name: "archive-not-found",
            expect: Expect::FailsOnArchive {
                serving: ArchiveServing::NotFound,
            },
            error_contains: &["the archive for", "was not found at"],
            ..BASE
        },
        Case {
            name: "missing-target",
            release: Release {
                targets: &[Target::MADE_UP],
                ..RELEASE
            },
            expect: Expect::FailsAfterManifest,
            error_contains: &[
                "archive for this platform",
                "looked for:",
                "targets available for this version:",
            ],
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "unknown-format",
            tweak: ManifestTweak::UnknownFormat,
            expect: Expect::FailsAfterManifest,
            error_contains: &["uses unsupported format 2 (expected format 1)"],
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "wrong-project",
            release: Release {
                project: "some-other-project",
                ..RELEASE
            },
            expect: Expect::FailsAfterManifest,
            error_contains: &["is for project \"some-other-project\", not \"widget-project\""],
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "not-a-manifest",
            tweak: ManifestTweak::NotAManifest,
            expect: Expect::FailsAfterManifest,
            error_contains: &["is not an install manifest"],
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "crlf-line-endings",
            tweak: ManifestTweak::CrlfLineEndings,
            ..BASE
        },
        // Ensure that if the last line is missing a newline, we expect things
        // to still work.
        //
        // The platform must be the last one in the manifest:
        //
        // * illumos is the last of ALL_TARGETS.
        // * The zip is the last line for a Windows target.
        Case {
            group: Some("manifest"),
            name: "no-final-newline",
            sh: Some(ShRun {
                platform: ShPlatform::Real(TargetChoice::Override(Target::X86_64_UNKNOWN_ILLUMOS)),
                ..ShRun::DEFAULT
            }),
            powershell: None,
            tweak: ManifestTweak::NoFinalNewline,
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "no-final-newline",
            sh: None,
            powershell: Some(PowerShellRun {
                platform: PowerShellPlatform::Real(TargetChoice::Override(
                    Target::X86_64_PC_WINDOWS_MSVC,
                )),
                ..PowerShellRun::DEFAULT
            }),
            release: Release {
                targets: &[Target::X86_64_PC_WINDOWS_MSVC],
                ..RELEASE
            },
            tweak: ManifestTweak::NoFinalNewline,
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "malformed-checksum",
            tweak: ManifestTweak::MalformedChecksum,
            expect: Expect::FailsAfterManifest,
            error_contains: &["has a malformed SHA-256 checksum for target"],
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "uppercase-checksum",
            tweak: ManifestTweak::UppercaseChecksum,
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "plain-http-archive-url",
            tweak: ManifestTweak::PlainHttpArchiveUrl,
            expect: Expect::FailsAfterManifest,
            error_contains: &[
                "has an archive URL for target",
                "that is not an https URL: \"http://archives.widget.example/widget\"",
            ],
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "short-archive-line",
            tweak: ManifestTweak::ShortArchiveLine,
            expect: Expect::FailsAfterManifest,
            error_contains: &[
                "has a malformed archive line: \"archive made-up-target tar.gz 0123456789abcdef",
                "the manifest may be incomplete",
            ],
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "malformed-version",
            tweak: ManifestTweak::MalformedVersion,
            expect: Expect::FailsAfterManifest,
            error_contains: &[
                "has a missing or malformed version: \"0.9\"",
                "expected MAJOR.MINOR.PATCH",
            ],
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "unknown-status",
            tweak: ManifestTweak::UnknownStatus,
            expect: Expect::FailsAfterManifest,
            error_contains: &["has an unknown release status: \"retired\""],
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "unknown-key",
            tweak: ManifestTweak::UnknownKey,
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "empty-body",
            tweak: ManifestTweak::EmptyBody,
            expect: Expect::FailsAfterManifest,
            error_contains: &["is not an install manifest"],
            ..BASE
        },
        Case {
            name: "connection-dropped",
            expect: Expect::FailsOnArchive {
                serving: ArchiveServing::HangUp,
            },
            error_contains: &["failed to download"],
            ..BASE
        },
        // Ensure that both scripts retry 503s.
        Case {
            name: "retries-unavailable-archive",
            expect: Expect::Installed {
                serving: ArchiveServing::UnavailableOnce,
                post_install: PostInstall::Runs,
            },
            ..BASE
        },
        #[cfg(not(windows))]
        Case {
            name: "refuses-other-platforms-without-target",
            sh: None,
            powershell: Some(PowerShellRun {
                platform: PowerShellPlatform::Real(TargetChoice::Host),
                ..PowerShellRun::DEFAULT
            }),
            expect: Expect::FailsBeforeManifest,
            error_contains: &["install.ps1 only supports Windows", "install.sh"],
            ..BASE
        },
        // This test checks that the registry is preferred over the other alternatives.
        //
        // The process's variables say x86. But a correct script must ignore them and
        // read from `/proc/registry`.
        #[cfg(windows)]
        Case {
            group: Some("detect"),
            name: "machine-architecture-from-registry",
            powershell: None,
            env: &[
                ("PROCESSOR_ARCHITECTURE", Some("x86")),
                ("PROCESSOR_ARCHITEW6432", None),
            ],
            // We only expect this note on x64 builds. On ARM64, it depends
            // which platform `sh` is compiled for.
            #[cfg(target_arch = "x86_64")]
            progress_contains: &[
                "the shell is an x86 process on an AMD64 machine; preferring AMD64 builds",
            ],
            ..BASE
        },
        Case {
            group: Some("detect"),
            name: "unknown-architecture",
            sh: None,
            powershell: Some(PowerShellRun {
                platform: PowerShellPlatform::FakeArch {
                    arch: FakeWindowsArch::new("PDP11", "IA64", None),
                    expected_target: None,
                },
                ..PowerShellRun::DEFAULT
            }),
            expect: Expect::FailsBeforeManifest,
            error_contains: &[
                "no widget-project build is known for this platform",
                "architecture: IA64",
                "-Target <TRIPLE> or set $env:WIDGET_INSTALL_TARGET",
            ],
            ..BASE
        },
    ];

    cases.extend(windows_architecture_cases(|arch, target| Case {
        sh: None,
        powershell: Some(PowerShellRun {
            platform: PowerShellPlatform::FakeArch {
                arch,
                expected_target: Some(target),
            },
            ..PowerShellRun::DEFAULT
        }),
        ..BASE
    }));

    for (name, checksum) in [
        (
            "checksum-mismatch-in-first-digit",
            ManifestChecksum::FirstDigitWrong,
        ),
        (
            "checksum-mismatch-in-last-digit",
            ManifestChecksum::LastDigitWrong,
        ),
    ] {
        cases.push(Case {
            name,
            release: Release {
                checksum,
                ..RELEASE
            },
            existing: Existing::OldBinary,
            expect: FAILS_ON_ARCHIVE,
            error_contains: &[
                "checksum mismatch",
                "expected SHA-256:",
                "nothing was installed",
            ],
            ..BASE
        });
    }

    for (name, version, post_install) in [
        ("at-minimum-version", "0.9.62", PostInstall::Runs),
        (
            "build-metadata-at-minimum",
            "0.9.62+build-1",
            PostInstall::Runs,
        ),
        ("prerelease-above-minimum", "0.9.63-rc.1", PostInstall::Runs),
        ("next-minor-version", "0.10.0", PostInstall::Runs),
        ("next-major-version", "1.0.0", PostInstall::Runs),
        (
            "skipped-below-minimum",
            "0.9.61",
            PostInstall::SkippedBelowMinimumVersion,
        ),
        (
            "skipped-for-prerelease-of-minimum",
            "0.9.62-rc.1",
            PostInstall::SkippedBelowMinimumVersion,
        ),
        (
            "skipped-for-lower-minor-with-higher-patch",
            "0.8.100",
            PostInstall::SkippedBelowMinimumVersion,
        ),
    ] {
        cases.push(Case {
            group: Some("post-install"),
            name,
            release: Release { version, ..RELEASE },
            version: VersionChoice::Explicit(version),
            expect: Expect::Installed {
                serving: ArchiveServing::Normal,
                post_install,
            },
            ..WITH_POST_INSTALL
        });
    }

    cases
}
