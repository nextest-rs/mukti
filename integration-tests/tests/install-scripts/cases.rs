// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cases that don't need a constructed `PATH`.

#[cfg(unix)]
use crate::fake_tools::{FakeHost, FakeTools};
use crate::{
    fixture::{
        ArchiveContent, ArchiveServing, Given, HomeVars, InstallDir, Launch, ManifestChecksum,
        ManifestTweak, PostInstallStep, PowerShellMode, Release, ScriptOptions, ShMode, Value,
        PROJECT,
    },
    shells::ShellKind,
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
    // On platforms other than Windows, the install.ps1 script cannot detect a
    // target. Instead, we simulate that detection through FakeWindowsArch's
    // test hooks. (We don't actually publish the PowerShell script on
    // non-Windows platforms -- this is just for testing.)
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
    // Indicates that the script fails before it fetches an archive, so the
    // target is not checked.
    NotChecked,
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

    #[cfg(not(windows))]
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
    /// In cases where the post-install is run, its configured exit code. (The
    /// test helper is told to exit with this code.)
    pub(crate) post_install_exit_code: Option<u8>,
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
    /// goes to stdout when run as a subprocess) and errors are thrown (which
    /// goes to stderr).
    pub(crate) progress_contains: &'static [&'static str],
    /// Text that must appear in error output.
    pub(crate) error_contains: &'static [&'static str],
    /// Text that must not appear in the script's progress or error output.
    pub(crate) output_excludes: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Case {
    /// A prefix for the test name, for cases that belong together.
    pub(crate) group: Option<&'static str>,
    pub(crate) name: &'static str,
    pub(crate) sh: Option<ShRun>,
    pub(crate) powershell: Option<PowerShellRun>,
    pub(crate) input: CaseInput,
    pub(crate) expected: CaseExpected,
}

impl Case {
    pub(crate) fn test_name(&self) -> String {
        match self.group {
            Some(group) => format!("{group}::{}", self.name),
            None => self.name.to_owned(),
        }
    }

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
                ArchiveContent::Helper => false,
                ArchiveContent::Unrunnable => true,
            };
            if fails_to_start != unrunnable {
                inconsistencies.push(
                    "the archive must hold an unrunnable binary exactly when the binary is \
                     expected to fail to start"
                        .to_owned(),
                );
            }
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

        let runs = [
            self.sh.map(|run| (ShellKind::Sh, Launch::Sh(run.mode))),
            self.powershell
                .map(|run| (ShellKind::PowerShell, Launch::PowerShell(run.mode))),
        ];
        for (kind, launch) in runs.into_iter().flatten() {
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
            match kind.post_install_step() {
                PostInstallStep::Configured => {}
                #[cfg(not(windows))]
                PostInstallStep::Omitted => {
                    if after_manifest.is_some_and(ExpectedAfterManifest::post_install_fails) {
                        inconsistencies.push(format!(
                            "under {kind:?}, the script has no post-install step, so it \
                             cannot fail"
                        ));
                    }
                }
            }
        }

        inconsistencies
    }
}

pub(crate) const BASE: Case = Case {
    group: None,
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
            group: Some("detect"),
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
                "windows-amd64",
                FakeWindowsArch::new("AMD64", "AMD64", None),
            )
        },
        Case {
            expected: CaseExpected {
                target: ExpectedTarget::Exactly(Target::I686_PC_WINDOWS_MSVC),
                output_excludes: ARCHITECTURE_NOTE,
                ..BASE.expected
            },
            ..row("windows-x86", FakeWindowsArch::new("x86", "x86", None))
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
                "windows-machine-lookup-returns-garbage-in-x86-process",
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
                "windows-machine-lookup-returns-garbage",
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
                "windows-x64-process-on-arm64-falls-back-to-x86_64",
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
    let env_options = |target: Target| CaseInput {
        options: ScriptOptions {
            version: Given::env(Value::Literal("0.9")),
            target: Given::env(Value::Target(target)),
            ..ScriptOptions::ENV
        },
        ..BASE.input
    };

    let mut cases = vec![
        Case {
            name: "piped-with-env-options",
            sh: Some(ShRun::PIPED),
            powershell: None,
            input: env_options(Target::X86_64_UNKNOWN_FREEBSD),
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "0.9",
                    then: ExpectedAfterManifest::Installed {
                        post_install: PostInstall::Runs,
                    },
                },
                target: ExpectedTarget::Exactly(Target::X86_64_UNKNOWN_FREEBSD),
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "piped-with-env-options",
            sh: None,
            powershell: Some(PowerShellRun::PIPED),
            input: env_options(Target::I686_PC_WINDOWS_MSVC),
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "0.9",
                    then: ExpectedAfterManifest::Installed {
                        post_install: PostInstall::Runs,
                    },
                },
                target: ExpectedTarget::Exactly(Target::I686_PC_WINDOWS_MSVC),
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "piped-failure",
            sh: None,
            powershell: Some(PowerShellRun::PIPED),
            input: CaseInput {
                options: ScriptOptions {
                    version: Given::env(Value::Literal("9.9.9")),
                    ..ScriptOptions::ENV
                },
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "9.9.9",
                    then: ExpectedAfterManifest::FailsBeforeArchive,
                },
                error_contains: &["widget-project version \"9.9.9\" was not found"],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "install-dir-from-cargo-home",
            input: CaseInput {
                options: ScriptOptions {
                    install_dir: Given::NONE,
                    ..ScriptOptions::ARGS
                },
                ..BASE.input
            },
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
            ..BASE
        },
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
            input: CaseInput {
                options: ScriptOptions {
                    install_dir: Given::arg(Value::InstallDir(InstallDir::Relative)),
                    ..ScriptOptions::ARGS
                },
                ..BASE.input
            },
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
                options: ScriptOptions {
                    install_dir: Given::NONE,
                    ..ScriptOptions::ARGS
                },
                home: HomeVars::Neither,
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::FailsBeforeManifest,
                destination: None,
                error_contains: &["neither CARGO_HOME nor HOME is set", "--install-dir"],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "install-dir-with-brackets",
            sh: None,
            powershell: Some(PowerShellRun::FILE),
            input: CaseInput {
                options: ScriptOptions {
                    install_dir: Given::arg(Value::InstallDir(InstallDir::WithBrackets)),
                    ..ScriptOptions::ARGS
                },
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::FailsBeforeManifest,
                destination: Some(InstallDir::WithBrackets),
                error_contains: &[
                    "which PowerShell treats as wildcards",
                    "-InstallDir or $env:WIDGET_INSTALL_DIR",
                ],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "replaces-existing-binary",
            input: CaseInput {
                existing: Existing::OldBinary(InstallDir::Plain),
                ..BASE.input
            },
            ..BASE
        },
        #[cfg(windows)]
        Case {
            name: "locked-existing-binary",
            sh: None,
            input: CaseInput {
                existing: Existing::LockedOldBinary(InstallDir::Plain),
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsOnArchive,
                },
                error_contains: &["could not move the existing", "nothing was changed"],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "destination-is-a-directory",
            input: CaseInput {
                existing: Existing::Directory(InstallDir::Plain),
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsOnArchive,
                },
                error_contains: &["cannot install to", "it is a directory"],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "equals-sign-options",
            sh: Some(ShRun {
                mode: ShMode::FileWithEquals,
                ..ShRun::DEFAULT
            }),
            powershell: None,
            input: CaseInput {
                options: ScriptOptions {
                    version: Given::arg(Value::Literal("0.9")),
                    ..ScriptOptions::ARGS
                },
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "0.9",
                    then: ExpectedAfterManifest::Installed {
                        post_install: PostInstall::Runs,
                    },
                },
                ..BASE.expected
            },
            ..BASE
        },
        // A test case to simulate, for example:
        //
        //     WIDGET_INSTALL_VERSION=9.9.9 sh install.sh --version latest ...
        //
        // The server has a manifest for latest but not one for 9.9.9, so this
        // can only succeed if the argument was the one used.
        Case {
            name: "option-overrides-env",
            input: CaseInput {
                options: ScriptOptions {
                    version: Given {
                        arg: Some(Value::Literal("latest")),
                        env: Some(Value::Literal("9.9.9")),
                    },
                    ..ScriptOptions::ARGS
                },
                ..BASE.input
            },
            ..BASE
        },
        // For `target-override` below, the requested target differs from what
        // the shell would detect, so a script that ignored the corresponding
        // argument would fetch a different archive and therefore fail.
        Case {
            name: "target-override",
            powershell: None,
            input: CaseInput {
                options: ScriptOptions {
                    target: Given::arg(Value::Target(Target::X86_64_UNKNOWN_FREEBSD)),
                    ..ScriptOptions::ARGS
                },
                ..BASE.input
            },
            expected: CaseExpected {
                target: ExpectedTarget::Exactly(Target::X86_64_UNKNOWN_FREEBSD),
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "target-override",
            sh: None,
            input: CaseInput {
                options: ScriptOptions {
                    target: Given::arg(Value::Target(Target::I686_PC_WINDOWS_MSVC)),
                    ..ScriptOptions::ARGS
                },
                ..BASE.input
            },
            expected: CaseExpected {
                target: ExpectedTarget::Exactly(Target::I686_PC_WINDOWS_MSVC),
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "base-url-with-query",
            powershell: Some(PowerShellRun::FILE),
            input: CaseInput {
                options: ScriptOptions {
                    base_url: Given::arg(Value::ServerUrlWithSuffix("?mirror=1")),
                    ..ScriptOptions::ARGS
                },
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::FailsBeforeManifest,
                error_contains: &["invalid base URL", "must not contain"],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "base-url-with-trailing-slash",
            input: CaseInput {
                options: ScriptOptions {
                    base_url: Given::arg(Value::ServerUrlWithSuffix("/")),
                    ..ScriptOptions::ARGS
                },
                ..BASE.input
            },
            ..BASE
        },
        Case {
            name: "base-url-without-scheme",
            powershell: Some(PowerShellRun::FILE),
            input: CaseInput {
                options: ScriptOptions {
                    base_url: Given::arg(Value::ServerHostAndPort),
                    ..ScriptOptions::ARGS
                },
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::FailsBeforeManifest,
                error_contains: &["invalid base URL", "must start with"],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "base-url-plain-http",
            powershell: Some(PowerShellRun::FILE),
            input: CaseInput {
                options: ScriptOptions {
                    base_url: Given::arg(Value::ServerUrlAsPlainHttp),
                    ..ScriptOptions::ARGS
                },
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::FailsBeforeManifest,
                error_contains: &["invalid base URL: http://", "must start with \"https://\""],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "yanked-exact-version",
            input: CaseInput {
                options: ScriptOptions {
                    version: Given::arg(Value::Literal("0.9.148")),
                    ..ScriptOptions::ARGS
                },
                release: Release {
                    status: ReleaseStatus::Yanked,
                    ..RELEASE
                },
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "0.9.148",
                    then: ExpectedAfterManifest::Installed {
                        post_install: PostInstall::Runs,
                    },
                },
                progress_contains: &[
                    "widget-project 0.9.148 has been yanked; installing it anyway",
                ],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            group: Some("post-install"),
            name: "failure",
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
            ..WITH_POST_INSTALL
        },
        Case {
            name: "nonexistent-version",
            input: CaseInput {
                options: ScriptOptions {
                    version: Given::arg(Value::Literal("9.9.9")),
                    ..ScriptOptions::ARGS
                },
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "9.9.9",
                    then: ExpectedAfterManifest::FailsBeforeArchive,
                },
                error_contains: &["widget-project version \"9.9.9\" was not found"],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "invalid-version",
            powershell: Some(PowerShellRun::FILE),
            input: CaseInput {
                options: ScriptOptions {
                    version: Given::arg(Value::Literal("a$b")),
                    ..ScriptOptions::ARGS
                },
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::FailsBeforeManifest,
                error_contains: &["invalid version"],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "truncated-archive",
            input: CaseInput {
                archive: ArchiveServing::Truncated,
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsOnArchive,
                },
                error_contains: &["checksum mismatch", "nothing was installed"],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "archive-behind-redirect",
            input: CaseInput {
                archive: ArchiveServing::Redirect,
                ..BASE.input
            },
            ..BASE
        },
        Case {
            name: "archive-not-found",
            input: CaseInput {
                archive: ArchiveServing::NotFound,
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsOnArchive,
                },
                error_contains: &["the archive for", "was not found at"],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "missing-target",
            input: CaseInput {
                release: Release {
                    targets: &[Target::MADE_UP],
                    ..RELEASE
                },
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsBeforeArchive,
                },
                error_contains: &[
                    "archive for this platform",
                    "looked for:",
                    "targets available for this version:",
                ],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "unknown-format",
            input: CaseInput {
                tweak: ManifestTweak::UnknownFormat,
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsBeforeArchive,
                },
                error_contains: &["uses unsupported format 2 (expected format 1)"],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "wrong-project",
            input: CaseInput {
                release: Release {
                    project: "some-other-project",
                    ..RELEASE
                },
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsBeforeArchive,
                },
                error_contains: &["is for project \"some-other-project\", not \"widget-project\""],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "not-a-manifest",
            input: CaseInput {
                tweak: ManifestTweak::NotAManifest,
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsBeforeArchive,
                },
                error_contains: &["is not an install manifest"],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "crlf-line-endings",
            input: CaseInput {
                tweak: ManifestTweak::CrlfLineEndings,
                ..BASE.input
            },
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
            powershell: None,
            input: CaseInput {
                options: ScriptOptions {
                    target: Given::arg(Value::Target(Target::X86_64_UNKNOWN_ILLUMOS)),
                    ..ScriptOptions::ARGS
                },
                tweak: ManifestTweak::NoFinalNewline,
                ..BASE.input
            },
            expected: CaseExpected {
                target: ExpectedTarget::Exactly(Target::X86_64_UNKNOWN_ILLUMOS),
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "no-final-newline",
            sh: None,
            input: CaseInput {
                options: ScriptOptions {
                    target: Given::arg(Value::Target(Target::X86_64_PC_WINDOWS_MSVC)),
                    ..ScriptOptions::ARGS
                },
                release: Release {
                    targets: &[Target::X86_64_PC_WINDOWS_MSVC],
                    ..RELEASE
                },
                tweak: ManifestTweak::NoFinalNewline,
                ..BASE.input
            },
            expected: CaseExpected {
                target: ExpectedTarget::Exactly(Target::X86_64_PC_WINDOWS_MSVC),
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "malformed-checksum",
            input: CaseInput {
                tweak: ManifestTweak::MalformedChecksum,
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsBeforeArchive,
                },
                error_contains: &["has a malformed SHA-256 checksum for target"],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "plain-http-archive-url",
            input: CaseInput {
                tweak: ManifestTweak::PlainHttpArchiveUrl,
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsBeforeArchive,
                },
                error_contains: &[
                    "has an archive URL for target",
                    "that is not an https URL: \"http://archives.widget.example/widget\"",
                ],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "short-archive-line",
            input: CaseInput {
                tweak: ManifestTweak::ShortArchiveLine,
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsBeforeArchive,
                },
                error_contains: &[
                    "has a malformed archive line: \
                     \"archive made-up-target tar.gz 0123456789abcdef",
                    "the manifest may be incomplete",
                ],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "malformed-version",
            input: CaseInput {
                tweak: ManifestTweak::MalformedVersion,
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsBeforeArchive,
                },
                error_contains: &[
                    "has a missing or malformed version: \"0.9\"",
                    "expected MAJOR.MINOR.PATCH",
                ],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "unknown-status",
            input: CaseInput {
                tweak: ManifestTweak::UnknownStatus,
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsBeforeArchive,
                },
                error_contains: &["has an unknown release status: \"retired\""],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "unknown-key",
            input: CaseInput {
                tweak: ManifestTweak::UnknownKey,
                ..BASE.input
            },
            ..BASE
        },
        Case {
            group: Some("manifest"),
            name: "empty-body",
            input: CaseInput {
                tweak: ManifestTweak::EmptyBody,
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsBeforeArchive,
                },
                error_contains: &["is not an install manifest"],
                ..BASE.expected
            },
            ..BASE
        },
        Case {
            name: "connection-dropped",
            input: CaseInput {
                archive: ArchiveServing::HangUp,
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsOnArchive,
                },
                error_contains: &["failed to download"],
                ..BASE.expected
            },
            ..BASE
        },
        // Ensure that both scripts retry 503s.
        Case {
            name: "retries-unavailable-archive",
            input: CaseInput {
                archive: ArchiveServing::UnavailableOnce,
                ..BASE.input
            },
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
                result: ExpectedResult::FailsBeforeManifest,
                target: ExpectedTarget::NotChecked,
                error_contains: &["install.ps1 only supports Windows", "install.sh"],
                ..BASE.expected
            },
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
            group: Some("detect"),
            name: "unknown-architecture",
            sh: None,
            powershell: Some(PowerShellRun {
                platform: PowerShellPlatform::FakeArch(FakeWindowsArch::new("PDP11", "IA64", None)),
                ..PowerShellRun::DEFAULT
            }),
            expected: CaseExpected {
                result: ExpectedResult::FailsBeforeManifest,
                target: ExpectedTarget::NotChecked,
                error_contains: &[
                    "no widget-project build is known for this platform",
                    "architecture: IA64",
                    "-Target <TRIPLE> or set $env:WIDGET_INSTALL_TARGET",
                ],
                ..BASE.expected
            },
            ..BASE
        },
    ];

    cases.extend(powershell_windows_architecture_cases());
    cases.extend(sh_windows_architecture_cases());

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
            input: CaseInput {
                release: Release {
                    checksum,
                    ..RELEASE
                },
                existing: Existing::OldBinary(InstallDir::Plain),
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsOnArchive,
                },
                error_contains: &[
                    "checksum mismatch",
                    "expected SHA-256:",
                    "nothing was installed",
                ],
                ..BASE.expected
            },
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
            input: CaseInput {
                options: ScriptOptions {
                    version: Given::arg(Value::Literal(version)),
                    ..ScriptOptions::ARGS
                },
                release: Release { version, ..RELEASE },
                ..BASE.input
            },
            expected: CaseExpected {
                result: ExpectedResult::ReadsManifest {
                    manifest: version,
                    then: ExpectedAfterManifest::Installed { post_install },
                },
                ..BASE.expected
            },
            ..WITH_POST_INSTALL
        });
    }

    cases
}
