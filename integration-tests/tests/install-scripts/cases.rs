// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cases that don't need a constructed `PATH`.

#[cfg(unix)]
use crate::fake_tools::{FakeHost, FakeTools};
use crate::{
    fixture::{
        ArchiveContent, ArchiveServing, Given, HomeVars, InstallDir, ManifestChecksum,
        ManifestTweak, PostInstallStep, Release, ScriptOptions, Value, PROJECT,
    },
    shells::{
        FakeWindowsArch, Launch, PowerShellMode, PowerShellPlatform, PowerShellRun, ShMode,
        ShPlatform, ShRun, ShellKind,
    },
    targets::Target,
};
use mukti_metadata::{GlibcVersion, ReleaseStatus};

/// Represents a typical set of release targets.
///
/// This list is grabbed from nextest 0.9.148.
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

const GLIBC_FLOORS: &[(Target, GlibcVersion)] = &[
    (Target::X86_64_UNKNOWN_LINUX_GNU, GlibcVersion::new(2, 27)),
    (Target::AARCH64_UNKNOWN_LINUX_GNU, GlibcVersion::new(2, 27)),
    (
        Target::RISCV64GC_UNKNOWN_LINUX_GNU,
        GlibcVersion::new(2, 27),
    ),
];

pub(crate) const RELEASE: Release = Release {
    project: PROJECT,
    version: "0.9.148",
    status: ReleaseStatus::Active,
    targets: ALL_TARGETS,
    glibc_floors: GLIBC_FLOORS,
    checksum: ManifestChecksum::Correct,
};

#[derive(Clone, Copy, Debug)]
pub(crate) enum ShellRun {
    Sh(ShRun),
    PowerShell(PowerShellRun),
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ExpectedTarget {
    /// The test machine's platform, or for PowerShell on non-Windows platforms,
    /// x86_64 Windows.
    Host,
    Exactly(Target),
    /// The script fails before it fetches an archive, so the target is not
    /// checked.
    NotChecked,
}

/// What is in an install directory before the script runs.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Existing {
    Nothing,
    OldBinary(InstallDir),
    /// An old binary held open by another program.
    ///
    /// This (of course) is Windows-only.
    #[cfg(windows)]
    LockedOldBinary(InstallDir),
    /// An empty directory with the same name as the binary.
    Directory(InstallDir),
}

impl Existing {
    pub(crate) fn dir(self) -> Option<InstallDir> {
        match self {
            Self::Nothing => None,
            Self::OldBinary(dir) | Self::Directory(dir) => Some(dir),
            #[cfg(windows)]
            Self::LockedOldBinary(dir) => Some(dir),
        }
    }
}

/// The expected behavior of the script under the provided configuration.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ExpectedResult {
    /// The script is expected to fail before reading the manifest.
    FailsBeforeManifest,
    /// The script is expected to read the manifest.
    ReadsManifest {
        /// The manifest requested by the script, such as "latest" or "0.9".
        manifest: &'static str,
        /// The expected behavior after reading the manifest.
        then: ExpectedAfterManifest,
    },
}

/// The expected behavior of the script after successfully reading the manifest.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ExpectedAfterManifest {
    /// Fails without requesting the archive.
    FailsBeforeArchive,
    /// Requests the archive, then fails and leaves the destination as it was.
    FailsOnArchive,
    /// Installs the binary, but then fails in the post-install step.
    FailsPostInstall,
    /// Installs a file, then fails because the binary could not be started at
    /// all.
    ///
    /// In our tests, we simulate it with a shebang line with a non-existent
    /// interpreter. In reality this is most likely something like a glibc
    /// binary on a musl system.
    #[cfg(unix)]
    FailsToStartPostInstall,
    Installed {
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
    NotConfigured,
}

impl ExpectedTarget {
    fn is_checked(self) -> bool {
        match self {
            Self::Host | Self::Exactly(_) => true,
            Self::NotChecked => false,
        }
    }
}

impl ExpectedResult {
    fn after_manifest(self) -> Option<ExpectedAfterManifest> {
        match self {
            Self::FailsBeforeManifest => None,
            Self::ReadsManifest { then, .. } => Some(then),
        }
    }

    pub(crate) fn requests_archive(self) -> bool {
        self.after_manifest()
            .is_some_and(ExpectedAfterManifest::requests_archive)
    }
}

impl ExpectedAfterManifest {
    fn installs(self) -> bool {
        match self {
            Self::FailsBeforeArchive | Self::FailsOnArchive => false,
            Self::FailsPostInstall | Self::Installed { .. } => true,
            #[cfg(unix)]
            Self::FailsToStartPostInstall => true,
        }
    }

    fn post_install_fails(self) -> bool {
        match self {
            Self::FailsBeforeArchive | Self::FailsOnArchive | Self::Installed { .. } => false,
            Self::FailsPostInstall => true,
            #[cfg(unix)]
            Self::FailsToStartPostInstall => true,
        }
    }

    pub(crate) fn requests_archive(self) -> bool {
        match self {
            Self::FailsBeforeArchive => false,
            Self::FailsOnArchive | Self::FailsPostInstall | Self::Installed { .. } => true,
            #[cfg(unix)]
            Self::FailsToStartPostInstall => true,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CaseInput {
    pub(crate) release: Release,
    pub(crate) tweak: ManifestTweak,
    pub(crate) archive: ArchiveServing,
    pub(crate) archive_content: ArchiveContent,
    pub(crate) home: HomeVars,
    pub(crate) existing: Existing,
    pub(crate) options: ScriptOptions,
    pub(crate) post_install_step: PostInstallStep,
    /// In cases where the post-install is run, its configured exit code. (The
    /// test helper is told to exit with this code.)
    pub(crate) post_install_exit_code: Option<i32>,
}

/// The expected results for a test case.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CaseExpected {
    pub(crate) result: ExpectedResult,
    pub(crate) target: ExpectedTarget,
    /// The expected directory that the script is supposed to install the binary
    /// in.
    ///
    /// This is set for both successes (the binary should be installed) and
    /// failures (the existing contents should be left untouched). This is None
    /// in the `install-dir-unknown` case.
    pub(crate) destination: Option<InstallDir>,
    /// Text that must appear in the script's progress stream.
    ///
    /// This and `error_contains` below are named for the text rather than for
    /// stdout and stderr, because PowerShell's semantics are somewhat different
    /// here. With `install.sh`, progress and error text both go to stderr, but
    /// with `install.ps1` info lines are written via `Write-Information` (which
    /// goes to stdout when run as a subprocess) and errors are thrown (error
    /// messages go to stderr).
    pub(crate) progress_contains: &'static [&'static str],
    /// Text that must appear in error output.
    pub(crate) error_contains: &'static [&'static str],
    /// Text that must not appear in the script's progress or error output.
    pub(crate) output_excludes: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Case {
    pub(crate) name: &'static str,
    pub(crate) sh: Option<ShRun>,
    pub(crate) powershell: Option<PowerShellRun>,
    pub(crate) input: CaseInput,
    pub(crate) expected: CaseExpected,
}

impl Case {
    /// Returns a list of programmer errors while defining the case.
    ///
    /// The harness panics if this list is non-empty.
    pub(crate) fn inconsistencies(&self) -> Vec<String> {
        let input = &self.input;
        let expected = &self.expected;
        let mut inconsistencies = Vec::new();

        if self.sh.is_none() && self.powershell.is_none() {
            inconsistencies.push(
                "it does not run under any shell on this platform; if it only applies \
                 to another platform, gate it with #[cfg]"
                    .to_owned(),
            );
        }
        let after_manifest = expected.result.after_manifest();
        let fails_post_install = match after_manifest {
            Some(ExpectedAfterManifest::FailsPostInstall) => true,
            Some(
                ExpectedAfterManifest::FailsBeforeArchive
                | ExpectedAfterManifest::FailsOnArchive
                | ExpectedAfterManifest::Installed { .. },
            )
            | None => false,
            #[cfg(unix)]
            Some(ExpectedAfterManifest::FailsToStartPostInstall) => false,
        };
        if fails_post_install != input.post_install_exit_code.is_some() {
            inconsistencies.push(
                "post_install_exit_code must be set exactly when the post-install step is expected to fail"
                    .to_owned(),
            );
        }
        #[cfg(unix)]
        {
            let fails_to_start = match after_manifest {
                Some(ExpectedAfterManifest::FailsToStartPostInstall) => true,
                Some(
                    ExpectedAfterManifest::FailsBeforeArchive
                    | ExpectedAfterManifest::FailsOnArchive
                    | ExpectedAfterManifest::FailsPostInstall
                    | ExpectedAfterManifest::Installed { .. },
                )
                | None => false,
            };
            let unrunnable = match input.archive_content {
                ArchiveContent::Unrunnable => true,
                ArchiveContent::Helper
                | ArchiveContent::NotAnArchive
                | ArchiveContent::CorruptTar
                | ArchiveContent::MissingBinary
                | ArchiveContent::BinaryIsADirectory => false,
            };
            if fails_to_start != unrunnable {
                inconsistencies.push(
                    "the archive must hold an unrunnable binary exactly when the binary is \
                     expected to fail to start"
                        .to_owned(),
                );
            }
        }
        let installable = match input.archive_content {
            ArchiveContent::Helper => true,
            #[cfg(unix)]
            ArchiveContent::Unrunnable => true,
            ArchiveContent::NotAnArchive
            | ArchiveContent::CorruptTar
            | ArchiveContent::MissingBinary
            | ArchiveContent::BinaryIsADirectory => false,
        };
        if !installable && after_manifest.is_some_and(ExpectedAfterManifest::installs) {
            inconsistencies.push(format!(
                "the archive holds {:?}, so nothing can be installed from it",
                input.archive_content
            ));
        }
        let expects_not_configured = match after_manifest {
            Some(ExpectedAfterManifest::Installed {
                post_install: PostInstall::NotConfigured,
            }) => true,
            Some(
                ExpectedAfterManifest::Installed {
                    post_install: PostInstall::Runs | PostInstall::SkippedBelowMinimumVersion,
                }
                | ExpectedAfterManifest::FailsBeforeArchive
                | ExpectedAfterManifest::FailsOnArchive
                | ExpectedAfterManifest::FailsPostInstall,
            )
            | None => false,
            #[cfg(unix)]
            Some(ExpectedAfterManifest::FailsToStartPostInstall) => false,
        };
        let omitted = match input.post_install_step {
            PostInstallStep::Configured => false,
            PostInstallStep::Omitted => true,
        };
        if expects_not_configured && !omitted {
            inconsistencies.push(
                "PostInstall::NotConfigured is expected, but the post-install step is configured"
                    .to_owned(),
            );
        }
        if omitted && after_manifest.is_some_and(ExpectedAfterManifest::post_install_fails) {
            inconsistencies
                .push("the script has no post-install step, so it cannot fail".to_owned());
        }
        if input.archive != ArchiveServing::Normal && !expected.result.requests_archive() {
            inconsistencies.push(format!(
                "the archive is served as {:?}, but the script is not expected to request it",
                input.archive
            ));
        }
        if expected.destination.is_none() {
            if input.options.install_dir.arg.is_some() || input.options.install_dir.env.is_some() {
                inconsistencies.push(
                    "an install directory is given, but no destination is expected".to_owned(),
                );
            }
            if after_manifest.is_some_and(ExpectedAfterManifest::installs) {
                inconsistencies
                    .push("the script is expected to install, but to no destination".to_owned());
            }
        }
        if let Some(dir) = input.existing.dir() {
            if expected.destination != Some(dir) {
                inconsistencies.push(format!(
                    "{:?} must be in the expected destination, {:?}",
                    input.existing, expected.destination
                ));
            }
        }

        if !expected.target.is_checked()
            && (expected.result.requests_archive() || input.existing.dir().is_some())
        {
            inconsistencies.push(
                "no target is expected, but the archive or an existing binary needs one".to_owned(),
            );
        }

        let launches = [
            self.sh.map(|run| Launch::Sh(run.mode)),
            self.powershell.map(|run| Launch::PowerShell(run.mode)),
        ];
        for launch in launches.into_iter().flatten() {
            if input.options.has_args() && !launch.takes_options() {
                inconsistencies.push(format!(
                    "{launch:?} runs the script without arguments, so options must be given \
                     through their environment variables"
                ));
            }
            match launch {
                Launch::PowerShell(PowerShellMode::File) => match expected.result {
                    ExpectedResult::FailsBeforeManifest => {}
                    ExpectedResult::ReadsManifest { .. } => inconsistencies.push(
                        "a -File launch cannot be told about the test certificate, so it \
                         is only for cases that fail before any download"
                            .to_owned(),
                    ),
                },
                Launch::PowerShell(
                    PowerShellMode::CallOperator
                    | PowerShellMode::Piped
                    | PowerShellMode::ScriptBlock,
                )
                | Launch::Sh(ShMode::File | ShMode::FileWithEquals | ShMode::Piped) => {}
            }
        }

        inconsistencies
    }
}

pub(crate) const BASE: Case = Case {
    name: "",
    sh: Some(ShRun::DEFAULT),
    powershell: Some(PowerShellRun::DEFAULT),
    input: CaseInput {
        release: RELEASE,
        tweak: ManifestTweak::None,
        archive: ArchiveServing::Normal,
        archive_content: ArchiveContent::Helper,
        home: HomeVars::CargoHomeAndHome,
        existing: Existing::Nothing,
        options: ScriptOptions::ARGS,
        post_install_step: PostInstallStep::Configured,
        post_install_exit_code: None,
    },
    expected: CaseExpected {
        result: ExpectedResult::ReadsManifest {
            manifest: "latest",
            then: ExpectedAfterManifest::Installed {
                post_install: PostInstall::Runs,
            },
        },
        target: ExpectedTarget::Host,
        destination: Some(InstallDir::Plain),
        progress_contains: &[],
        error_contains: &[],
        output_excludes: &[],
    },
};

impl CaseInput {
    pub(crate) const fn with_release(self, release: Release) -> Self {
        Self { release, ..self }
    }

    pub(crate) const fn with_tweak(self, tweak: ManifestTweak) -> Self {
        Self { tweak, ..self }
    }

    pub(crate) const fn with_archive(self, archive: ArchiveServing) -> Self {
        Self { archive, ..self }
    }

    pub(crate) const fn with_existing(self, existing: Existing) -> Self {
        Self { existing, ..self }
    }

    pub(crate) const fn with_options(self, options: ScriptOptions) -> Self {
        Self { options, ..self }
    }
}

/// This section defines constructors for generally useful case outcomes.
/// Individual cases tweak the expectations with e.g. struct FRU syntax.
impl CaseExpected {
    pub(crate) const fn installs(manifest: &'static str) -> Self {
        Self {
            result: ExpectedResult::ReadsManifest {
                manifest,
                then: ExpectedAfterManifest::Installed {
                    post_install: PostInstall::Runs,
                },
            },
            ..BASE.expected
        }
    }

    pub(crate) const fn fails_before_manifest(error_contains: &'static [&'static str]) -> Self {
        Self {
            result: ExpectedResult::FailsBeforeManifest,
            error_contains,
            ..BASE.expected
        }
    }

    pub(crate) const fn fails_before_archive(
        manifest: &'static str,
        error_contains: &'static [&'static str],
    ) -> Self {
        Self {
            result: ExpectedResult::ReadsManifest {
                manifest,
                then: ExpectedAfterManifest::FailsBeforeArchive,
            },
            error_contains,
            ..BASE.expected
        }
    }

    pub(crate) const fn fails_on_archive(
        manifest: &'static str,
        error_contains: &'static [&'static str],
    ) -> Self {
        Self {
            result: ExpectedResult::ReadsManifest {
                manifest,
                then: ExpectedAfterManifest::FailsOnArchive,
            },
            error_contains,
            ..BASE.expected
        }
    }

    pub(crate) const fn with_target(self, target: Target) -> Self {
        Self {
            target: ExpectedTarget::Exactly(target),
            ..self
        }
    }
}

/// Printed when the architecture of the shell's own process differs from the
/// machine's, as "PowerShell is an ... process on an ... machine" or "the shell
/// is an ... process on an ... machine". When they match, this must _not_ be
/// printed.
const ARCHITECTURE_NOTE: &[&str] = &[" process on an "];

fn windows_architecture_cases(run: impl Fn(FakeWindowsArch) -> ShellRun) -> Vec<Case> {
    let row = |name: &'static str, arch: FakeWindowsArch| {
        let (sh, powershell) = match run(arch) {
            ShellRun::Sh(run) => (Some(run), None),
            ShellRun::PowerShell(run) => (None, Some(run)),
        };
        Case {
            name,
            sh,
            powershell,
            ..BASE
        }
    };

    vec![
        Case {
            expected: CaseExpected {
                target: ExpectedTarget::Exactly(Target::X86_64_PC_WINDOWS_MSVC),
                output_excludes: ARCHITECTURE_NOTE,
                ..BASE.expected
            },
            ..row(
                "detect::windows-amd64",
                FakeWindowsArch::new("AMD64", "AMD64", None),
            )
        },
        Case {
            expected: CaseExpected {
                target: ExpectedTarget::Exactly(Target::I686_PC_WINDOWS_MSVC),
                output_excludes: ARCHITECTURE_NOTE,
                ..BASE.expected
            },
            ..row(
                "detect::windows-x86",
                FakeWindowsArch::new("x86", "x86", None),
            )
        },
        // These rows have an unrecognized machine-level value, so the process's
        // variables decide.
        Case {
            expected: CaseExpected {
                target: ExpectedTarget::Exactly(Target::X86_64_PC_WINDOWS_MSVC),
                progress_contains: &[
                    "is an x86 process on an AMD64 machine; preferring AMD64 builds",
                ],
                ..BASE.expected
            },
            ..row(
                "detect::windows-machine-lookup-returns-garbage-in-x86-process",
                FakeWindowsArch::new("PDP11", "x86", Some("AMD64")),
            )
        },
        Case {
            expected: CaseExpected {
                target: ExpectedTarget::Exactly(Target::AARCH64_PC_WINDOWS_MSVC),
                output_excludes: ARCHITECTURE_NOTE,
                ..BASE.expected
            },
            ..row(
                "detect::windows-machine-lookup-returns-garbage",
                FakeWindowsArch::new("PDP11", "ARM64", None),
            )
        },
        Case {
            input: CaseInput {
                release: Release {
                    targets: &[Target::X86_64_PC_WINDOWS_MSVC, Target::I686_PC_WINDOWS_MSVC],
                    ..RELEASE
                },
                ..BASE.input
            },
            expected: CaseExpected {
                target: ExpectedTarget::Exactly(Target::X86_64_PC_WINDOWS_MSVC),
                progress_contains: &["preferring ARM64 builds"],
                ..BASE.expected
            },
            ..row(
                "detect::windows-x64-process-on-arm64-falls-back-to-x86_64",
                FakeWindowsArch::new("ARM64", "AMD64", None),
            )
        },
    ]
}

/// Returns test cases for `install.ps1` Windows architecture detection.
pub(crate) fn powershell_windows_architecture_cases() -> Vec<Case> {
    windows_architecture_cases(|arch| {
        ShellRun::PowerShell(PowerShellRun {
            platform: PowerShellPlatform::FakeArch(arch),
            ..PowerShellRun::DEFAULT
        })
    })
}

/// Returns test cases for `install.sh` Windows architecture detection.
pub(crate) fn sh_windows_architecture_cases() -> Vec<Case> {
    windows_architecture_cases(|arch| {
        // On Unix, we fake `uname` to be `MINGW*` to test `install.sh`'s
        // Windows support.
        #[cfg(unix)]
        let platform = ShPlatform::Fake(FakeTools::new(FakeHost::mingw(arch)));
        #[cfg(windows)]
        let platform = ShPlatform::FakeArchOnHost(arch);
        ShellRun::Sh(ShRun {
            platform,
            ..ShRun::DEFAULT
        })
    })
}

/// Returns the cases that don't need fake tools -- most run against the real
/// host.
pub(crate) fn common_cases() -> Vec<Case> {
    let env_options = |target: Target| {
        BASE.input.with_options(ScriptOptions {
            version: Given::env(Value::Literal("0.9")),
            target: Given::env(Value::Target(target)),
            ..ScriptOptions::ENV
        })
    };
    let version_arg = |version: &'static str| {
        BASE.input
            .with_options(ScriptOptions::ARGS.with_version(Given::arg(Value::Literal(version))))
    };
    let target_arg = |target: Target| {
        BASE.input
            .with_options(ScriptOptions::ARGS.with_target(Given::arg(Value::Target(target))))
    };
    let install_dir = |install_dir: Given| {
        BASE.input
            .with_options(ScriptOptions::ARGS.with_install_dir(install_dir))
    };
    let base_url_arg = |base_url: Value| {
        BASE.input
            .with_options(ScriptOptions::ARGS.with_base_url(Given::arg(base_url)))
    };

    let mut cases = vec![
        Case {
            name: "piped-with-env-options",
            sh: Some(ShRun::PIPED),
            powershell: None,
            input: env_options(Target::X86_64_UNKNOWN_FREEBSD),
            expected: CaseExpected::installs("0.9").with_target(Target::X86_64_UNKNOWN_FREEBSD),
        },
        Case {
            name: "piped-with-env-options",
            sh: None,
            powershell: Some(PowerShellRun::PIPED),
            input: env_options(Target::I686_PC_WINDOWS_MSVC),
            expected: CaseExpected::installs("0.9").with_target(Target::I686_PC_WINDOWS_MSVC),
        },
        Case {
            name: "piped-failure",
            sh: None,
            powershell: Some(PowerShellRun::PIPED),
            input: BASE.input.with_options(ScriptOptions {
                version: Given::env(Value::Literal("9.9.9")),
                ..ScriptOptions::ENV
            }),
            expected: CaseExpected::fails_before_archive(
                "9.9.9",
                &["widget-project version \"9.9.9\" was not found"],
            ),
        },
        Case {
            name: "install-dir-from-cargo-home",
            input: install_dir(Given::NONE),
            expected: CaseExpected {
                destination: Some(InstallDir::CargoHomeBin),
                ..BASE.expected
            },
            ..BASE
        },
        // Configuration if you just run the script via `curl | sh` or the PowerShell
        // equivalent.
        Case {
            name: "no-options",
            sh: Some(ShRun::PIPED),
            powershell: Some(PowerShellRun::PIPED),
            input: CaseInput {
                options: ScriptOptions::NONE,
                home: HomeVars::HomeOnly,
                ..BASE.input
            },
            expected: CaseExpected {
                destination: Some(InstallDir::HomeCargoBin),
                ..BASE.expected
            },
        },
        // Empty env vars are treated as unset.
        Case {
            name: "empty-env-options",
            input: CaseInput {
                options: ScriptOptions {
                    version: Given::env(Value::Literal("")),
                    install_dir: Given::env(Value::Literal("")),
                    target: Given::env(Value::Literal("")),
                    base_url: Given::env(Value::Literal("")),
                },
                home: HomeVars::HomeOnly,
                ..BASE.input
            },
            expected: CaseExpected {
                destination: Some(InstallDir::HomeCargoBin),
                ..BASE.expected
            },
            ..BASE
        },
        // A PowerShell session has two current directories:
        //
        // 1. PowerShell's own location (`$PWD`). This is changed when
        //    Set-Location is invoked.
        // 2. The process's working directory, which Set-Location does not
        //    change, and which .NET APIs such as [IO.Directory]::CreateDirectory
        //    resolve relative paths against.
        //
        // A relative install directory is relative to 1, not 2, so install.ps1
        // turns the path into an absolute one before passing it to .NET.
        //
        // The two are the same unless something changes one of them before the
        // script runs. With the default way to launch the script, via
        // `-Command` + a call operator, there's no room for anything like that,
        // so this case would pass even if `install.ps1` skipped that step. In
        // other scenarios, the caller _can_ run `Set-Location` first (and we do,
        // in `Fixture::command`). That's why we use a script block to simulate
        // that scenario.
        Case {
            name: "install-dir-relative",
            powershell: Some(PowerShellRun {
                mode: PowerShellMode::ScriptBlock,
                ..PowerShellRun::DEFAULT
            }),
            input: install_dir(Given::arg(Value::InstallDir(InstallDir::Relative))),
            expected: CaseExpected {
                destination: Some(InstallDir::Relative),
                ..BASE.expected
            },
            ..BASE
        },
        // This can't be used on Windows since the MSYS and Cygwin shells set
        // HOME themselves when it is unset.
        #[cfg(not(windows))]
        Case {
            name: "install-dir-unknown",
            powershell: None,
            input: CaseInput {
                home: HomeVars::Neither,
                ..install_dir(Given::NONE)
            },
            expected: CaseExpected {
                destination: None,
                ..CaseExpected::fails_before_manifest(&[
                    "neither CARGO_HOME nor HOME is set",
                    "--install-dir",
                ])
            },
            ..BASE
        },
        Case {
            name: "install-dir-cannot-be-created",
            input: install_dir(Given::arg(Value::InstallDir(InstallDir::UnderFile))),
            expected: CaseExpected {
                destination: Some(InstallDir::UnderFile),
                ..CaseExpected::fails_before_manifest(&["could not create the install directory"])
            },
            ..BASE
        },
        Case {
            name: "install-dir-with-brackets",
            sh: None,
            powershell: Some(PowerShellRun::FILE),
            input: install_dir(Given::arg(Value::InstallDir(InstallDir::WithBrackets))),
            expected: CaseExpected {
                destination: Some(InstallDir::WithBrackets),
                ..CaseExpected::fails_before_manifest(&[
                    "which PowerShell treats as wildcards",
                    "-InstallDir or $env:WIDGET_INSTALL_DIR",
                ])
            },
        },
        Case {
            name: "replaces-existing-binary",
            input: BASE
                .input
                .with_existing(Existing::OldBinary(InstallDir::Plain)),
            ..BASE
        },
        #[cfg(windows)]
        Case {
            name: "locked-existing-binary",
            sh: None,
            input: BASE
                .input
                .with_existing(Existing::LockedOldBinary(InstallDir::Plain)),
            expected: CaseExpected::fails_on_archive(
                "latest",
                &["could not move the existing", "nothing was changed"],
            ),
            ..BASE
        },
        Case {
            name: "destination-is-a-directory",
            input: BASE
                .input
                .with_existing(Existing::Directory(InstallDir::Plain)),
            expected: CaseExpected::fails_on_archive(
                "latest",
                &["cannot install to", "it is a directory"],
            ),
            ..BASE
        },
        Case {
            name: "equals-sign-options",
            sh: Some(ShRun {
                mode: ShMode::FileWithEquals,
                ..ShRun::DEFAULT
            }),
            powershell: None,
            input: version_arg("0.9"),
            expected: CaseExpected::installs("0.9"),
        },
        // A test case to simulate, for example:
        //
        //     WIDGET_INSTALL_VERSION=9.9.9 sh install.sh --version latest ...
        //
        // The server has a manifest for latest but not one for 9.9.9, so this
        // can only succeed if the argument was the one used.
        Case {
            name: "option-overrides-env::version",
            input: BASE
                .input
                .with_options(ScriptOptions::ARGS.with_version(Given {
                    arg: Some(Value::Literal("latest")),
                    env: Some(Value::Literal("9.9.9")),
                })),
            ..BASE
        },
        // Test case for CLI option > environment variable. Here, if the env won
        // over the CLI option, then the expectation would fail because it
        // wouldn't find the installed binary in the expected location.
        Case {
            name: "option-overrides-env::install-dir",
            input: BASE
                .input
                .with_options(ScriptOptions::ARGS.with_install_dir(Given {
                    arg: Some(Value::InstallDir(InstallDir::Plain)),
                    env: Some(Value::InstallDir(InstallDir::HomeCargoBin)),
                })),
            ..BASE
        },
        // Test case for CLI option > environment variable. Here, the env var's
        // base URL is plain http, which is refused by the install scripts.
        Case {
            name: "option-overrides-env::base-url",
            input: BASE
                .input
                .with_options(ScriptOptions::ARGS.with_base_url(Given {
                    arg: Some(Value::ServerUrl),
                    env: Some(Value::ServerUrlAsPlainHttp),
                })),
            ..BASE
        },
        // Test case for CLI option > environment variable. Here, the env var's
        // target does not have an archive available.
        Case {
            name: "option-overrides-env::target",
            powershell: None,
            input: BASE
                .input
                .with_options(ScriptOptions::ARGS.with_target(Given {
                    arg: Some(Value::Target(Target::X86_64_UNKNOWN_FREEBSD)),
                    env: Some(Value::Target(Target::MADE_UP)),
                })),
            expected: BASE.expected.with_target(Target::X86_64_UNKNOWN_FREEBSD),
            ..BASE
        },
        Case {
            name: "option-overrides-env::target",
            sh: None,
            input: BASE
                .input
                .with_options(ScriptOptions::ARGS.with_target(Given {
                    arg: Some(Value::Target(Target::I686_PC_WINDOWS_MSVC)),
                    env: Some(Value::Target(Target::MADE_UP)),
                })),
            expected: BASE.expected.with_target(Target::I686_PC_WINDOWS_MSVC),
            ..BASE
        },
        // For `target-override` below, the requested target differs from what
        // the shell would detect, so a script that ignored the corresponding
        // argument would fetch a different archive and therefore fail.
        Case {
            name: "target-override",
            powershell: None,
            input: target_arg(Target::X86_64_UNKNOWN_FREEBSD),
            expected: BASE.expected.with_target(Target::X86_64_UNKNOWN_FREEBSD),
            ..BASE
        },
        Case {
            name: "target-override",
            sh: None,
            input: target_arg(Target::I686_PC_WINDOWS_MSVC),
            expected: BASE.expected.with_target(Target::I686_PC_WINDOWS_MSVC),
            ..BASE
        },
        Case {
            name: "base-url-with-query",
            powershell: Some(PowerShellRun::FILE),
            input: base_url_arg(Value::ServerUrlWithSuffix("?mirror=1")),
            expected: CaseExpected::fails_before_manifest(&[
                "invalid base URL",
                "must not contain",
            ]),
            ..BASE
        },
        Case {
            name: "base-url-with-trailing-slash",
            input: base_url_arg(Value::ServerUrlWithSuffix("/")),
            ..BASE
        },
        Case {
            name: "base-url-with-trailing-slashes",
            input: base_url_arg(Value::ServerUrlWithSuffix("//")),
            ..BASE
        },
        Case {
            name: "base-url-without-scheme",
            powershell: Some(PowerShellRun::FILE),
            input: base_url_arg(Value::ServerHostAndPort),
            expected: CaseExpected::fails_before_manifest(&["invalid base URL", "must start with"]),
            ..BASE
        },
        Case {
            name: "base-url-plain-http",
            powershell: Some(PowerShellRun::FILE),
            input: base_url_arg(Value::ServerUrlAsPlainHttp),
            expected: CaseExpected::fails_before_manifest(&[
                "invalid base URL: http://",
                "must start with \"https://\"",
            ]),
            ..BASE
        },
        Case {
            name: "yanked-exact-version",
            input: version_arg("0.9.148").with_release(Release {
                status: ReleaseStatus::Yanked,
                ..RELEASE
            }),
            expected: CaseExpected {
                progress_contains: &[
                    "widget-project 0.9.148 has been yanked; installing it anyway",
                ],
                ..CaseExpected::installs("0.9.148")
            },
            ..BASE
        },
        Case {
            name: "post-install::not-configured",
            input: CaseInput {
                post_install_step: PostInstallStep::Omitted,
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::Installed {
                        post_install: PostInstall::NotConfigured,
                    },
                },
                output_excludes: &["post-install"],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "post-install::failure",
            input: CaseInput {
                post_install_exit_code: Some(3),
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsPostInstall,
                },
                error_contains: &[
                    "widget-project 0.9.148 was installed to",
                    "the post-install step (widget self setup) failed with exit code 3",
                    "the binary is in place",
                ],
                ..BASE.expected
            },
            ..BASE
        },
        // The exit code is STATUS_DLL_NOT_FOUND, to simulate the situation
        // where Windows cannot load the binary.
        #[cfg(windows)]
        Case {
            name: "post-install::loader-failure",
            sh: None,
            input: CaseInput {
                post_install_exit_code: Some(0xC000_0135_u32 as i32),
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsPostInstall,
                },
                error_contains: &[
                    "but Windows could not start it (status 0xC0000135)",
                    "pass -Target <TRIPLE>",
                ],
                ..BASE.expected
            },
            ..BASE
        },
        // The binary cannot be started at all.
        //
        // The installed file cannot be exec'd (its shebang line has a missing
        // interpreter), so the status comes from the shell.
        #[cfg(unix)]
        Case {
            name: "post-install::binary-cannot-be-started",
            input: CaseInput {
                archive_content: ArchiveContent::Unrunnable,
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsToStartPostInstall,
                },
                error_contains: &[
                    "widget-project 0.9.148",
                    // A shell can report either 126 or 127 for a missing
                    // interpreter, so we just check for "exit code 12".
                    "but it cannot run on this system (exit code 12",
                    "pass --target <TRIPLE>",
                ],
                ..BASE.expected
            },
            powershell: None,
            ..BASE
        },
        #[cfg(unix)]
        Case {
            name: "post-install::binary-cannot-be-started",
            sh: None,
            input: CaseInput {
                archive_content: ArchiveContent::Unrunnable,
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsToStartPostInstall,
                },
                error_contains: &[
                    "widget-project 0.9.148",
                    // Under pwsh, the process cannot be started, so install.ps1
                    // reports the launch error rather than an exit status.
                    "but it cannot run on this system: ",
                    "pass -Target <TRIPLE>",
                ],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "nonexistent-version",
            input: version_arg("9.9.9"),
            expected: CaseExpected::fails_before_archive(
                "9.9.9",
                &["widget-project version \"9.9.9\" was not found"],
            ),
            ..BASE
        },
        Case {
            name: "invalid-version",
            powershell: Some(PowerShellRun::FILE),
            input: version_arg("a$b"),
            expected: CaseExpected::fails_before_manifest(&["invalid version"]),
            ..BASE
        },
        Case {
            name: "truncated-archive",
            input: BASE.input.with_archive(ArchiveServing::Truncated),
            expected: CaseExpected::fails_on_archive(
                "latest",
                &["checksum mismatch", "nothing was installed"],
            ),
            ..BASE
        },
        Case {
            name: "archive-behind-redirect",
            input: BASE.input.with_archive(ArchiveServing::Redirect),
            ..BASE
        },
        Case {
            name: "archive-not-found",
            input: BASE.input.with_archive(ArchiveServing::NotFound),
            expected: CaseExpected::fails_on_archive(
                "latest",
                &["the archive for", "was not found at"],
            ),
            ..BASE
        },
        Case {
            name: "missing-target",
            input: BASE.input.with_release(Release {
                targets: &[Target::MADE_UP],
                ..RELEASE
            }),
            expected: CaseExpected::fails_before_archive(
                "latest",
                &[
                    "archive for this platform",
                    "looked for:",
                    "targets available for this version:",
                ],
            ),
            ..BASE
        },
        Case {
            name: "manifest::wrong-project",
            input: BASE.input.with_release(Release {
                project: "some-other-project",
                ..RELEASE
            }),
            expected: CaseExpected::fails_before_archive(
                "latest",
                &["is for project \"some-other-project\", not \"widget-project\""],
            ),
            ..BASE
        },
        Case {
            name: "manifest::crlf-line-endings",
            input: BASE.input.with_tweak(ManifestTweak::CrlfLineEndings),
            ..BASE
        },
        // Ensure that a manifest without a final newline still works.
        //
        // The platform must be the last one in the manifest:
        //
        // * illumos is the last of ALL_TARGETS.
        // * The zip is the last line for a Windows target.
        Case {
            name: "manifest::no-final-newline",
            powershell: None,
            input: target_arg(Target::X86_64_UNKNOWN_ILLUMOS)
                .with_tweak(ManifestTweak::NoFinalNewline),
            expected: BASE.expected.with_target(Target::X86_64_UNKNOWN_ILLUMOS),
            ..BASE
        },
        Case {
            name: "manifest::no-final-newline",
            sh: None,
            input: target_arg(Target::X86_64_PC_WINDOWS_MSVC)
                .with_release(Release {
                    targets: &[Target::X86_64_PC_WINDOWS_MSVC],
                    ..RELEASE
                })
                .with_tweak(ManifestTweak::NoFinalNewline),
            expected: BASE.expected.with_target(Target::X86_64_PC_WINDOWS_MSVC),
            ..BASE
        },
        Case {
            name: "manifest::unknown-key",
            input: BASE.input.with_tweak(ManifestTweak::UnknownKey),
            ..BASE
        },
        Case {
            name: "connection-dropped",
            input: BASE.input.with_archive(ArchiveServing::HangUp),
            expected: CaseExpected::fails_on_archive("latest", &["failed to download"]),
            ..BASE
        },
        Case {
            name: "stalled-download-times-out",
            input: BASE.input.with_archive(ArchiveServing::StallUntilTimeout),
            expected: CaseExpected::fails_on_archive("latest", &["failed to download"]),
            ..BASE
        },
        // Ensure that both scripts retry 503s.
        Case {
            name: "retries-unavailable-archive",
            input: BASE.input.with_archive(ArchiveServing::UnavailableOnce),
            ..BASE
        },
        #[cfg(not(windows))]
        Case {
            name: "refuses-other-platforms-without-target",
            sh: None,
            powershell: Some(PowerShellRun {
                platform: PowerShellPlatform::Host,
                ..PowerShellRun::DEFAULT
            }),
            expected: CaseExpected {
                target: ExpectedTarget::NotChecked,
                ..CaseExpected::fails_before_manifest(&[
                    "install.ps1 only supports Windows",
                    "install.sh",
                ])
            },
            ..BASE
        },
        // This test checks that the registry is preferred over the other alternatives.
        //
        // The process's variables say x86. But a correct script must ignore them and
        // read from `/proc/registry`.
        #[cfg(windows)]
        Case {
            name: "detect::machine-architecture-from-registry",
            sh: Some(ShRun {
                platform: ShPlatform::X86VarsOnHost,
                ..ShRun::DEFAULT
            }),
            powershell: None,
            expected: CaseExpected {
                // We only expect this note on x64 builds. On ARM64, it depends
                // which platform `sh` is compiled for.
                #[cfg(target_arch = "x86_64")]
                progress_contains: &[
                    "the shell is an x86 process on an AMD64 machine; preferring AMD64 builds",
                ],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "detect::unknown-architecture",
            sh: None,
            powershell: Some(PowerShellRun {
                platform: PowerShellPlatform::FakeArch(FakeWindowsArch::new("PDP11", "IA64", None)),
                ..PowerShellRun::DEFAULT
            }),
            expected: CaseExpected {
                target: ExpectedTarget::NotChecked,
                ..CaseExpected::fails_before_manifest(&[
                    "no widget-project build is known for this platform",
                    "architecture: IA64",
                    "-Target <TRIPLE> or set $env:WIDGET_INSTALL_TARGET",
                ])
            },
            ..BASE
        },
    ];

    cases.extend(powershell_windows_architecture_cases());
    cases.extend(sh_windows_architecture_cases());

    // Manifests that the script must refuse after reading them, before
    // requesting an archive.
    let rejected_manifests: [(&str, ManifestTweak, &'static [&'static str]); 11] = [
        (
            "manifest::unknown-format",
            ManifestTweak::UnknownFormat,
            &["uses unsupported format 2 (expected format 1)"],
        ),
        (
            "manifest::not-a-manifest",
            ManifestTweak::NotAManifest,
            &["is not an install manifest"],
        ),
        (
            "manifest::malformed-checksum",
            ManifestTweak::MalformedChecksum,
            &["has a malformed SHA-256 checksum for target"],
        ),
        (
            "manifest::plain-http-archive-url",
            ManifestTweak::PlainHttpArchiveUrl,
            &[
                "has an archive URL for target",
                "that is not an https URL: \"http://archives.widget.example/widget\"",
            ],
        ),
        (
            "manifest::short-archive-line",
            ManifestTweak::ShortArchiveLine,
            &[
                "has a malformed archive line: \
                 \"archive made-up-target tar.gz 0123456789abcdef",
                "the manifest may be incomplete",
            ],
        ),
        (
            "manifest::missing-glibc-floor",
            ManifestTweak::MissingGlibcFloor,
            &[
                "has a malformed archive line: \"archive x86_64-unknown-linux-gnu tar.gz ",
                "the manifest may be incomplete",
            ],
        ),
        (
            "manifest::malformed-glibc-floor",
            ManifestTweak::MalformedGlibcFloor,
            &[
                "has a malformed archive line: \"archive x86_64-unknown-linux-gnu tar.gz ",
                " min-glibc=2.x\"",
            ],
        ),
        (
            "manifest::glibc-floor-on-other-target",
            ManifestTweak::GlibcFloorOnOtherTarget,
            &[
                "has a malformed archive line: \"archive x86_64-unknown-linux-musl tar.gz ",
                " min-glibc=2.27\"",
            ],
        ),
        (
            "manifest::malformed-version",
            ManifestTweak::MalformedVersion,
            &[
                "has a missing or malformed version: \"0.9\"",
                "expected MAJOR.MINOR.PATCH",
            ],
        ),
        (
            "manifest::unknown-status",
            ManifestTweak::UnknownStatus,
            &["has an unknown release status: \"retired\""],
        ),
        (
            "manifest::empty-body",
            ManifestTweak::EmptyBody,
            &["is not an install manifest"],
        ),
    ];
    for (name, tweak, error_contains) in rejected_manifests {
        cases.push(Case {
            name,
            input: BASE.input.with_tweak(tweak),
            expected: CaseExpected::fails_before_archive("latest", error_contains),
            ..BASE
        });
    }

    // Archives that are valid per checksum but fail to extract in some manner.
    // In these cases, the old binary must be left alone.
    let broken_archives: [(&str, ArchiveContent, ShellKind, &'static [&'static str]); 7] = [
        (
            "archive::not-an-archive",
            ArchiveContent::NotAnArchive,
            ShellKind::Sh,
            &[
                "failed to decompress the downloaded archive",
                "nothing was installed",
            ],
        ),
        (
            "archive::not-an-archive",
            ArchiveContent::NotAnArchive,
            ShellKind::PowerShell,
            &[
                "failed to extract the downloaded archive",
                "nothing was installed",
            ],
        ),
        (
            "archive::corrupt-tar",
            ArchiveContent::CorruptTar,
            ShellKind::Sh,
            // "widget" is a prefix of "widget.exe", so these also match on
            // Windows.
            &[
                "failed to extract widget",
                "from the downloaded archive",
                "nothing was installed",
            ],
        ),
        (
            "archive::missing-binary",
            ArchiveContent::MissingBinary,
            ShellKind::Sh,
            // "widget" is a prefix of "widget.exe", so these also match on
            // Windows.
            &[
                "failed to extract widget",
                "from the downloaded archive",
                "nothing was installed",
            ],
        ),
        (
            "archive::missing-binary",
            ArchiveContent::MissingBinary,
            ShellKind::PowerShell,
            &[
                "the downloaded archive did not contain widget.exe; nothing was installed",
                "this is a problem with the release",
            ],
        ),
        (
            "archive::binary-is-a-directory",
            ArchiveContent::BinaryIsADirectory,
            ShellKind::Sh,
            &[
                "the downloaded archive did not contain widget",
                "nothing was installed",
                "this is a problem with the release",
            ],
        ),
        (
            "archive::binary-is-a-directory",
            ArchiveContent::BinaryIsADirectory,
            ShellKind::PowerShell,
            &[
                "the downloaded archive did not contain widget.exe; nothing was installed",
                "this is a problem with the release",
            ],
        ),
    ];
    for (name, archive_content, kind, error_contains) in broken_archives {
        let (sh, powershell) = match kind {
            ShellKind::Sh => (BASE.sh, None),
            ShellKind::PowerShell => (None, BASE.powershell),
        };
        cases.push(Case {
            name,
            sh,
            powershell,
            input: CaseInput {
                archive_content,
                ..BASE
                    .input
                    .with_existing(Existing::OldBinary(InstallDir::Plain))
            },
            expected: CaseExpected::fails_on_archive("latest", error_contains),
        });
    }

    const MAJOR_SERIES_RELEASE: Release = Release {
        version: "1.2.3",
        ..RELEASE
    };
    cases.push(Case {
        name: "major-series",
        input: version_arg("1").with_release(MAJOR_SERIES_RELEASE),
        expected: CaseExpected::installs("1"),
        ..BASE
    });
    // Test cases for manifests with mismatched versions.
    let mismatched_versions: [(&str, CaseInput, &str, &'static [&'static str]); 4] = [
        (
            "manifest::version-mismatch-exact",
            version_arg("0.9.148").with_tweak(ManifestTweak::VersionLine("0.9.147")),
            "0.9.148",
            &["is for widget-project 0.9.147, which version \"0.9.148\" cannot resolve to"],
        ),
        (
            "manifest::version-mismatch-series",
            version_arg("0.9").with_tweak(ManifestTweak::VersionLine("0.10.0")),
            "0.9",
            &["is for widget-project 0.10.0, which version \"0.9\" cannot resolve to"],
        ),
        (
            "manifest::version-mismatch-major-series",
            version_arg("1")
                .with_release(MAJOR_SERIES_RELEASE)
                .with_tweak(ManifestTweak::VersionLine("2.0.0")),
            "1",
            &["is for widget-project 2.0.0, which version \"1\" cannot resolve to"],
        ),
        (
            "manifest::prerelease-for-latest",
            BASE.input
                .with_tweak(ManifestTweak::VersionLine("0.9.149-rc.1")),
            "latest",
            &["is for widget-project 0.9.149-rc.1, which version \"latest\" cannot resolve to"],
        ),
    ];
    for (name, input, manifest, error_contains) in mismatched_versions {
        cases.push(Case {
            name,
            input,
            expected: CaseExpected::fails_before_archive(manifest, error_contains),
            ..BASE
        });
    }

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
            input: BASE
                .input
                .with_release(Release {
                    checksum,
                    ..RELEASE
                })
                .with_existing(Existing::OldBinary(InstallDir::Plain)),
            expected: CaseExpected::fails_on_archive(
                "latest",
                &[
                    "checksum mismatch",
                    "expected SHA-256:",
                    "nothing was installed",
                ],
            ),
            ..BASE
        });
    }

    for (name, version, post_install) in [
        (
            "post-install::at-minimum-version",
            "0.9.62",
            PostInstall::Runs,
        ),
        (
            "post-install::build-metadata-at-minimum",
            "0.9.62+build-1",
            PostInstall::Runs,
        ),
        (
            "post-install::prerelease-above-minimum",
            "0.9.63-rc.1",
            PostInstall::Runs,
        ),
        (
            "post-install::next-minor-version",
            "0.10.0",
            PostInstall::Runs,
        ),
        (
            "post-install::next-major-version",
            "1.0.0",
            PostInstall::Runs,
        ),
        (
            "post-install::skipped-below-minimum",
            "0.9.61",
            PostInstall::SkippedBelowMinimumVersion,
        ),
        (
            "post-install::skipped-for-prerelease-of-minimum",
            "0.9.62-rc.1",
            PostInstall::SkippedBelowMinimumVersion,
        ),
        (
            "post-install::skipped-for-lower-minor-with-higher-patch",
            "0.8.100",
            PostInstall::SkippedBelowMinimumVersion,
        ),
    ] {
        cases.push(Case {
            name,
            input: version_arg(version).with_release(Release { version, ..RELEASE }),
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: version,
                    then: ExpectedAfterManifest::Installed { post_install },
                },
                ..BASE.expected
            },
            ..BASE
        });
    }

    cases
}
