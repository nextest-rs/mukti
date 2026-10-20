// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#[cfg(unix)]
use crate::fake_tools::{
    build_tool_dir, curl_arguments, curl_not_found, fake_tool_cases, ToolDirCurl,
};
#[cfg(windows)]
use crate::host::{powershell_is_emulated, windows_host_target, EMULATED_POWERSHELL_VAR};
#[cfg(unix)]
use crate::standalone::{sh_signal_cleans_up, Signal};
use crate::{
    archive::{ArchiveFormat, Sha256},
    cases::{
        common_cases, Case, Existing, ExpectedAfterManifest, ExpectedResult, ExpectedTarget,
        PostInstall, PowerShellPlatform, PowerShellRun, ShPlatform, ShRun,
    },
    fixture::{
        dir_entry_names, real_default_binaries, CheckedRun, DefaultBaseUrl, Fixture, Invocation,
        Launch, Outcome, PostInstallStep, PowerShellMode, ServedArchive, ShMode, ShellLaunch,
        BIN_NAME, CALLER_INTACT, OLD_BINARY, POST_INSTALL_ARGS, PROJECT,
    },
    host::sh_host_target,
    shells::{
        LocatedShell, PowerShellName, Requirement, Requirements, ShName, ShellKind, ShellName,
        REQUIRE_VAR, SHELLS,
    },
    standalone::{
        bad_values_are_rejected, https_redirect_to_http, powershell_interrupt_cleans_up,
        powershell_rejected_arguments, sh_help, sh_rejected_arguments, sh_truncated_script,
    },
    targets::Target,
};
use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::eyre::{bail, eyre, Context, Report, Result};
use libtest_mimic::{Failed, Trial};
#[cfg(windows)]
use std::env;
use std::{fmt, fs, io};

// TODO-RAINCLAUDE: a case's run configuration, together with the shell it
// runs under.
#[derive(Clone, Copy, Debug)]
enum CaseRun {
    Sh(ShName, ShRun),
    PowerShell(PowerShellName, PowerShellRun),
}

impl CaseRun {
    fn kind(self) -> ShellKind {
        match self {
            Self::Sh(..) => ShellKind::Sh,
            Self::PowerShell(..) => ShellKind::PowerShell,
        }
    }

    fn launch(self) -> ShellLaunch {
        match self {
            Self::Sh(shell, run) => ShellLaunch::Sh(shell, run.mode),
            Self::PowerShell(shell, run) => ShellLaunch::PowerShell(shell, run.mode),
        }
    }

    /// Returns true if this shell run configuration uses fake tools.
    ///
    /// In some scenarios, fake tools put on PATH are not available. We must
    /// skip the fake tools tests on those platforms.
    #[cfg(unix)]
    fn uses_fake_tools(self) -> bool {
        match self {
            Self::Sh(_, run) => match run.platform {
                ShPlatform::Host => false,
                ShPlatform::Fake(_) => true,
            },
            Self::PowerShell(..) => false,
        }
    }

    fn expected_target(self, expected: ExpectedTarget) -> Result<Option<Target>> {
        match expected {
            ExpectedTarget::Host => self.host_target().map(Some),
            ExpectedTarget::Exactly(target) => Ok(Some(target)),
            ExpectedTarget::NotChecked => Ok(None),
        }
    }

    fn host_target(self) -> Result<Target> {
        match self {
            Self::Sh(_, run) => match run.platform {
                ShPlatform::Host => sh_host_target(),
                #[cfg(windows)]
                ShPlatform::X86VarsOnHost => sh_host_target(),
                #[cfg(windows)]
                ShPlatform::FakeArchOnHost(_) => Err(faked_platform_needs_exact_target()),
                #[cfg(unix)]
                ShPlatform::Fake(_) => Err(faked_platform_needs_exact_target()),
            },
            Self::PowerShell(_, run) => match run.platform {
                #[cfg(windows)]
                PowerShellPlatform::Host => windows_host_target(),
                #[cfg(not(windows))]
                PowerShellPlatform::Host => Err(eyre!(
                    "install.ps1 only supports Windows, so on this host, the expected target \
                     must be ExpectedTarget::NotChecked"
                )),
                #[cfg(not(windows))]
                PowerShellPlatform::FakeWindowsX64 => Ok(Target::X86_64_PC_WINDOWS_MSVC),
                PowerShellPlatform::FakeArch(_) => Err(faked_platform_needs_exact_target()),
            },
        }
    }
}

fn faked_platform_needs_exact_target() -> Report {
    eyre!(
        "the platform is faked, so the expected target must be \
         ExpectedTarget::Exactly or ExpectedTarget::NotChecked, not ExpectedTarget::Host"
    )
}

fn all_cases() -> Vec<Case> {
    let cases = common_cases();
    #[cfg(unix)]
    let cases = [cases, fake_tool_cases()].concat();

    // Check for any inconsistencies (programming errors) in each case.
    let mut inconsistencies = Vec::new();
    for case in &cases {
        for inconsistency in case.inconsistencies() {
            inconsistencies.push(format!("{}: {inconsistency}", case.test_name()));
        }
    }
    assert!(
        inconsistencies.is_empty(),
        "inconsistent cases:\n{}",
        inconsistencies.join("\n")
    );

    cases
}

pub(crate) fn trials() -> Result<Vec<Trial>> {
    let requirements = Requirements::from_env()?;
    // We can't really run these tests if a file with the test binary's name
    // already exists in a real default install location, such as
    // `$CARGO_HOME/bin` or `~/.cargo/bin`. Our tests assert that that those
    // paths don't exist afterwards.
    for path in real_default_binaries() {
        if path.exists() {
            bail!(
                "{path} exists; the install script tests use that name and must never create it \
                 (move it out of the way and run the tests again)"
            );
        }
    }
    let cases = all_cases();

    let mut trials = vec![Trial::test("generated::bad-values-are-rejected", || {
        bad_values_are_rejected().map_err(failed)
    })];

    for &shell in SHELLS {
        let shell_path = match requirements.locate_shell(shell) {
            LocatedShell::Found(path) => path,
            LocatedShell::Missing(requirement) => {
                trials.push(missing_shell_trial(shell, requirement));
                continue;
            }
        };

        for &case in &cases {
            let name = format!("{shell}::{}", case.test_name());
            let run = match shell {
                ShellName::Sh(sh) => case.sh.map(|run| CaseRun::Sh(sh, run)),
                ShellName::PowerShell(powershell) => case
                    .powershell
                    .map(|run| CaseRun::PowerShell(powershell, run)),
            };
            let Some(run) = run else {
                continue;
            };
            #[cfg(unix)]
            match run {
                CaseRun::Sh(sh, _) => {
                    if run.uses_fake_tools() && !sh.honors_path() {
                        continue;
                    }
                }
                CaseRun::PowerShell(..) => {}
            }

            let shell_path = shell_path.clone();
            trials.push(Trial::test(name, move || {
                run_case(&shell_path, &case, run).map_err(failed)
            }));
        }

        match shell {
            ShellName::Sh(sh) => {
                for (name, test) in [
                    (
                        "truncated-script",
                        sh_truncated_script as fn(ShName, &Utf8Path) -> Result<()>,
                    ),
                    ("rejected-arguments", sh_rejected_arguments),
                    ("help", sh_help),
                ] {
                    trials.push(shell_trial(sh, &shell_path, name, test));
                }
                #[cfg(unix)]
                for signal in Signal::ALL {
                    let shell_path = shell_path.clone();
                    trials.push(Trial::test(
                        format!("{sh}::signal-cleans-up::{}", signal.test_name()),
                        move || sh_signal_cleans_up(sh, &shell_path, signal).map_err(failed),
                    ));
                }
                #[cfg(unix)]
                if sh.honors_path() {
                    trials.push(shell_trial(
                        sh,
                        &shell_path,
                        "curl::arguments",
                        curl_arguments,
                    ));
                    trials.push(shell_trial(
                        sh,
                        &shell_path,
                        "curl::not-found",
                        curl_not_found,
                    ));
                }
            }
            ShellName::PowerShell(powershell) => {
                for (name, test) in [
                    (
                        "rejected-arguments",
                        powershell_rejected_arguments
                            as fn(PowerShellName, &Utf8Path) -> Result<()>,
                    ),
                    ("interrupt-cleans-up", powershell_interrupt_cleans_up),
                ] {
                    trials.push(shell_trial(powershell, &shell_path, name, test));
                }
                #[cfg(windows)]
                if env::var_os(EMULATED_POWERSHELL_VAR).is_some() {
                    trials.push(shell_trial(
                        powershell,
                        &shell_path,
                        "is-emulated-x64-on-arm64",
                        powershell_is_emulated,
                    ));
                }
            }
        }
        trials.push(shell_trial(
            shell,
            &shell_path,
            "https-redirect-to-http",
            https_redirect_to_http,
        ));
    }

    Ok(trials)
}

fn shell_trial<S>(
    shell: S,
    shell_path: &Utf8Path,
    name: &str,
    test: fn(S, &Utf8Path) -> Result<()>,
) -> Trial
where
    S: Copy + fmt::Display + Send + 'static,
{
    let shell_path = shell_path.to_owned();
    Trial::test(format!("{shell}::{name}"), move || {
        test(shell, &shell_path).map_err(failed)
    })
}

fn failed(error: Report) -> Failed {
    format!("{error:?}").into()
}

fn missing_shell_trial(shell: ShellName, requirement: Requirement) -> Trial {
    let (message, ignored) = match requirement {
        Requirement::Required => (
            format!(
                "{shell} was not found on PATH, but it is required \
                 (by {REQUIRE_VAR}, or because every Unix has sh)"
            ),
            false,
        ),
        Requirement::Optional => (
            format!("{shell} was not found on PATH; add it to {REQUIRE_VAR} to make this an error"),
            true,
        ),
    };
    Trial::test(format!("{shell}::not-installed"), move || {
        Err(message.into())
    })
    .with_ignored_flag(ignored)
}

fn run_case(shell_path: &Utf8Path, case: &Case, run: CaseRun) -> Result<()> {
    let target = run.expected_target(case.expected.target)?;
    let format = match run.kind() {
        ShellKind::Sh => ArchiveFormat::TarGz,
        ShellKind::PowerShell => ArchiveFormat::Zip,
    };
    let served = match (case.expected.result.requests_archive(), target) {
        (true, Some(target)) => Some(ServedArchive {
            target,
            format,
            serving: case.input.archive,
            content: case.input.archive_content,
        }),
        (true, None) => {
            bail!("a case that fetches an archive must say which target it expects");
        }
        (false, Some(_) | None) => None,
    };
    let mut fixture = Fixture::new(
        &case.input.release,
        case.input.tweak,
        served,
        run.kind().post_install_step(),
        DefaultBaseUrl::for_options(case.input.options),
    )?;

    let result = run_script(&fixture, shell_path, case, run, target);
    let mut checked = fixture.verify_requests_with(result)?;
    if let Some(hang_ups) = fixture.take_hang_up_count() {
        let expected = match run.kind() {
            // curl only makes one attempt for dropped connections.
            ShellKind::Sh => 1..=1,
            // install.ps1 by itself does not retry a dropped connection
            // (there's no HTTP status, so Get-Url treats it as permanent). But
            // Get-Url calls Invoke-WebRequest, which _does_ retry. We observed
            // that a single Invoke-WebRequest call woulod make 4 connections on
            // PowerShell 7, and 2 on Windows PowerShell 5.1.
            ShellKind::PowerShell => 1..=4,
        };
        if !expected.contains(&hang_ups) {
            checked.problems.push(format!(
                "expected {expected:?} dropped connections, saw {hang_ups}"
            ));
        }
    }
    checked.report()
}

fn run_script(
    fixture: &Fixture,
    shell_path: &Utf8Path,
    case: &Case,
    run: CaseRun,
    target: Option<Target>,
) -> Result<CheckedRun> {
    let (path, env): (_, Vec<(&str, Option<String>)>) = match run {
        CaseRun::Sh(
            _,
            ShRun {
                platform: ShPlatform::Host,
                ..
            },
        ) => (None, Vec::new()),
        #[cfg(windows)]
        CaseRun::Sh(
            _,
            ShRun {
                platform: ShPlatform::X86VarsOnHost,
                ..
            },
        ) => (
            None,
            vec![
                ("PROCESSOR_ARCHITECTURE", Some("x86".to_owned())),
                ("PROCESSOR_ARCHITEW6432", None),
            ],
        ),
        #[cfg(windows)]
        CaseRun::Sh(
            _,
            ShRun {
                platform: ShPlatform::FakeArchOnHost(arch),
                ..
            },
        ) => (None, arch.env().to_vec()),
        #[cfg(unix)]
        CaseRun::Sh(
            _,
            ShRun {
                platform: ShPlatform::Fake(tools),
                ..
            },
        ) => {
            let tool_dir = fixture.tool_dir();
            build_tool_dir(&tool_dir, ToolDirCurl::Real, tools.hash_tool)?;
            (Some(tool_dir), tools.host.env())
        }
        CaseRun::PowerShell(_, PowerShellRun { platform, .. }) => (None, platform.env()),
    };

    match case.expected.result {
        ExpectedResult::FailsBeforeManifest => {}
        ExpectedResult::ReadsManifest { manifest, .. } => fixture.expect_manifest(manifest)?,
    }
    if case.expected.result.requests_archive() {
        fixture.expect_archive()?;
    }
    match run.launch().launch() {
        Launch::PowerShell(PowerShellMode::Piped | PowerShellMode::ScriptBlock) => {
            fixture.expect_powershell_script()?;
        }
        Launch::PowerShell(PowerShellMode::CallOperator | PowerShellMode::File)
        | Launch::Sh(ShMode::File | ShMode::FileWithEquals | ShMode::Piped) => {}
    }

    let lock: Option<fs::File> = match case.input.existing {
        Existing::Nothing => None,
        Existing::OldBinary(dir) => {
            write_old_binary(&fixture.install_dir(dir), target)?;
            None
        }
        #[cfg(windows)]
        Existing::LockedOldBinary(dir) => {
            let binary = write_old_binary(&fixture.install_dir(dir), target)?;
            Some(lock_file(&binary)?)
        }
        Existing::Directory(dir) => {
            let directory = fixture.install_dir(dir).join(bin_file_name(target)?);
            fs::create_dir_all(&directory)
                .wrap_err_with(|| format!("failed to create {directory}"))?;
            None
        }
    };

    let outcome = fixture.run(
        shell_path,
        &Invocation {
            options: case.input.options,
            home: case.input.home,
            env,
            path,
            post_install_exit_code: case.input.post_install_exit_code,
            ..Invocation::new(run.launch())
        },
    )?;
    drop(lock);

    let problems = check_outcome(fixture, case, run, &outcome, target)?;
    Ok(CheckedRun { problems, outcome })
}

pub(crate) fn write_old_binary(
    install_dir: &Utf8Path,
    target: Option<Target>,
) -> Result<Utf8PathBuf> {
    let binary = install_dir.join(bin_file_name(target)?);
    fs::create_dir_all(install_dir).wrap_err("failed to create the install directory")?;
    fs::write(&binary, OLD_BINARY).wrap_err("failed to write the old binary")?;
    Ok(binary)
}

/// Returns the binary's file name in the install directory, including with
/// `.exe` on Windows.
fn bin_file_name(target: Option<Target>) -> Result<&'static str> {
    let target = target.ok_or_else(|| {
        eyre!(
            "a case with a binary in the install directory, before or after the script runs, \
             must say which target it expects, since the binary's file name depends on it"
        )
    })?;
    Ok(target.bin_file_name())
}

pub(crate) fn report_problems(problems: Vec<String>) -> Result<()> {
    if problems.is_empty() {
        Ok(())
    } else {
        Err(eyre!("{}", problems.join("\n")))
    }
}

fn check_outcome(
    fixture: &Fixture,
    case: &Case,
    run: CaseRun,
    outcome: &Outcome,
    target: Option<Target>,
) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    let stdout = normalize_output(&outcome.stdout);
    let stderr = normalize_output(&outcome.stderr);
    let helper_log = fixture.helper_log();
    let logged = match fs::read_to_string(&helper_log) {
        Ok(logged) => Some(logged),
        // No log means the installed binary was never run.
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        // Any other error is a problem with the test itself.
        Err(error) => {
            return Err(error)
                .wrap_err_with(|| format!("failed to read the helper's log at {helper_log}"));
        }
    };
    let mut expected_progress: Vec<String> = case
        .expected
        .progress_contains
        .iter()
        .map(|text| (*text).to_owned())
        .collect();
    let release_version = case.input.release.version;
    let (result, post_install) = match case.expected.result {
        ExpectedResult::ReadsManifest {
            then: ExpectedAfterManifest::Installed { post_install },
            ..
        } => {
            expected_progress.push(format!("installed {PROJECT} {release_version} to"));
            if let Some(target) = target {
                expected_progress.push(format!(
                    "installing {PROJECT} {release_version} ({target}) to"
                ));
            }
            match run.launch().launch() {
                Launch::PowerShell(PowerShellMode::Piped) => {
                    expected_progress.push(CALLER_INTACT.to_owned());
                }
                Launch::PowerShell(
                    PowerShellMode::CallOperator
                    | PowerShellMode::File
                    | PowerShellMode::ScriptBlock,
                )
                | Launch::Sh(ShMode::File | ShMode::FileWithEquals | ShMode::Piped) => {}
            }
            let post_install = match (run.kind().post_install_step(), post_install) {
                (PostInstallStep::Configured, PostInstall::Runs) => PostInstallExpectation::Ran,
                (PostInstallStep::Configured, PostInstall::SkippedBelowMinimumVersion) => {
                    PostInstallExpectation::NotAttempted
                }
                #[cfg(not(windows))]
                (
                    PostInstallStep::Omitted,
                    PostInstall::Runs | PostInstall::SkippedBelowMinimumVersion,
                ) => PostInstallExpectation::NotAttempted,
            };
            (ExpectedOutcome::InstalledAndSucceeded, post_install)
        }
        ExpectedResult::ReadsManifest {
            then: ExpectedAfterManifest::FailsPostInstall,
            ..
        } => (
            ExpectedOutcome::InstalledButFailed,
            PostInstallExpectation::Ran,
        ),
        #[cfg(unix)]
        ExpectedResult::ReadsManifest {
            then: ExpectedAfterManifest::FailsToStartPostInstall,
            ..
        } => (
            ExpectedOutcome::InstalledButFailed,
            PostInstallExpectation::CouldNotStart,
        ),
        ExpectedResult::FailsBeforeManifest
        | ExpectedResult::ReadsManifest {
            then: ExpectedAfterManifest::FailsBeforeArchive | ExpectedAfterManifest::FailsOnArchive,
            ..
        } => (
            ExpectedOutcome::FailedWithoutInstalling,
            PostInstallExpectation::NotAttempted,
        ),
    };
    match (result, outcome.status.success()) {
        (ExpectedOutcome::InstalledAndSucceeded, false) => {
            problems.push("the script failed, but was expected to succeed".to_owned());
        }
        (ExpectedOutcome::InstalledButFailed | ExpectedOutcome::FailedWithoutInstalling, true) => {
            problems.push("the script succeeded, but was expected to fail".to_owned());
        }
        (ExpectedOutcome::InstalledAndSucceeded, true)
        | (ExpectedOutcome::InstalledButFailed | ExpectedOutcome::FailedWithoutInstalling, false) =>
            {}
    }
    let announced = format!(
        "running post-install step: {BIN_NAME} {}",
        POST_INSTALL_ARGS.join(" ")
    );
    // The helper appends its arguments each time it is run, so a step that is
    // run twice shows up as two sets here.
    let expected_log = match post_install {
        PostInstallExpectation::NotAttempted => None,
        PostInstallExpectation::Ran => {
            expected_progress.push(announced);
            Some(format!("{}\n", POST_INSTALL_ARGS.join("\n")))
        }
        #[cfg(unix)]
        PostInstallExpectation::CouldNotStart => {
            expected_progress.push(announced);
            None
        }
    };
    if logged != expected_log {
        problems.push(format!(
            "the post-install step's recorded arguments were {logged:?}, expected {expected_log:?}"
        ));
    }

    let expected_binary = match result {
        ExpectedOutcome::InstalledAndSucceeded | ExpectedOutcome::InstalledButFailed => {
            let sha256 = fixture.served_binary_sha256().ok_or_else(|| {
                eyre!("an install is expected, but no archive is served to install from")
            })?;
            Some(ExpectedBinary::Installed(sha256))
        }
        ExpectedOutcome::FailedWithoutInstalling => match case.input.existing {
            Existing::Nothing => None,
            Existing::OldBinary(_) => Some(ExpectedBinary::Old),
            #[cfg(windows)]
            Existing::LockedOldBinary(_) => Some(ExpectedBinary::Old),
            Existing::Directory(_) => Some(ExpectedBinary::EmptyDirectory),
        },
    };
    if let Some(destination) = case.expected.destination {
        check_install_dir(
            &fixture.install_dir(destination),
            expected_binary,
            target,
            &mut problems,
        )?;
    }

    // * install.sh writes all of its own text to stderr.
    // * Under PowerShell, install.ps1's info and warning lines come out on stdout.
    //   Only errors go to stderr.
    let (progress, progress_stream) = match run.kind() {
        ShellKind::Sh => (&stderr, "stderr"),
        ShellKind::PowerShell => (&stdout, "stdout"),
    };
    for text in expected_progress {
        if !progress.contains(&normalize_output(&text)) {
            problems.push(format!("{progress_stream} does not contain {text:?}"));
        }
    }
    for text in case.expected.error_contains {
        if !stderr.contains(&normalize_output(text)) {
            problems.push(format!("stderr does not contain {text:?}"));
        }
    }
    for text in case.expected.output_excludes {
        let text = normalize_output(text);
        for (stream, name) in [(&stdout, "stdout"), (&stderr, "stderr")] {
            if stream.contains(&text) {
                problems.push(format!("{name} contains {text:?}"));
            }
        }
    }
    match run.kind() {
        // Nothing in install.sh writes to stdout other than --help. (The
        // post-install command could produce output to stdout, but the
        // post-install command we configure doesn't print anything to stdout.)
        ShellKind::Sh => {
            if !outcome.stdout.is_empty() {
                problems.push("install.sh wrote to stdout".to_owned());
            }
        }
        ShellKind::PowerShell => {}
    }
    let strays = fixture.stray_work_dir_entries(case.expected.destination)?;
    if !strays.is_empty() {
        problems.push(format!(
            "the script left {strays:?} in the directory it was run in"
        ));
    }
    for path in fixture.forbidden_paths(case.expected.destination) {
        if path.exists() {
            problems.push(format!(
                "the script created {path}, a default location it was not asked to use"
            ));
        }
    }

    Ok(problems)
}

pub(crate) fn check_install_dir(
    install_dir: &Utf8Path,
    expected: Option<ExpectedBinary>,
    target: Option<Target>,
    problems: &mut Vec<String>,
) -> Result<()> {
    let present = dir_entry_names(install_dir)?;
    let Some(expected) = expected else {
        if !present.is_empty() {
            problems.push(format!(
                "the install directory contains {present:?}, expected nothing"
            ));
        }
        return Ok(());
    };

    let bin_name = bin_file_name(target)?;
    if present.len() != 1 || !present.contains(bin_name) {
        problems.push(format!(
            "the install directory contains {present:?}, expected only {bin_name:?}"
        ));
        return Ok(());
    }
    let binary = install_dir.join(bin_name);
    match expected {
        ExpectedBinary::Installed(expected_sha256) => {
            let contents =
                fs::read(&binary).wrap_err_with(|| format!("failed to read {binary}"))?;
            let sha256 = Sha256::of(&contents);
            if sha256 != expected_sha256 {
                problems.push(format!(
                    "{binary} is not the binary from the archive: \
                     its SHA-256 is {sha256}, expected {expected_sha256}"
                ));
            }
        }
        ExpectedBinary::Old => {
            let contents =
                fs::read(&binary).wrap_err_with(|| format!("failed to read {binary}"))?;
            if contents != OLD_BINARY {
                problems.push(format!("{binary} is no longer the old binary"));
            }
        }
        ExpectedBinary::EmptyDirectory => {
            if !binary.is_dir() {
                problems.push(format!("{binary} is no longer a directory"));
            } else {
                let inside = dir_entry_names(&binary)?;
                if !inside.is_empty() {
                    problems.push(format!("{binary} contains {inside:?}, expected nothing"));
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum ExpectedOutcome {
    InstalledAndSucceeded,
    InstalledButFailed,
    FailedWithoutInstalling,
}

#[derive(Clone, Copy, Debug)]
enum PostInstallExpectation {
    NotAttempted,
    Ran,
    #[cfg(unix)]
    CouldNotStart,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ExpectedBinary {
    Installed(Sha256),
    Old,
    EmptyDirectory,
}

/// Normalizes output into a string.
///
/// PowerShell 7 colors error messages (even with `NO_COLOR` set) and rewraps
/// them to the console width behind a "|" gutter, so we must compare text with
/// both of these stripped out. Some example output (substitute the
/// corresponding ANSI codes for `<red>` and `<reset>`):
//
//     <red>| <red>... argument 'stray'. the options are -Version, -InstallDir,<reset>
//     <red>| <red>-Target, and -BaseUrl<reset>
pub(crate) fn normalize_output(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            plain.push(c);
            continue;
        }
        if chars.next() == Some('[') {
            // A color code runs from ESC [ to its first letter.
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    plain
        .lines()
        .map(|line| line.trim_start().trim_start_matches('|'))
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(windows)]
fn lock_file(path: &Utf8Path) -> Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .wrap_err_with(|| format!("failed to lock {path}"))
}
