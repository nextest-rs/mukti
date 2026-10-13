// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#[cfg(unix)]
use crate::fake_tools::{
    build_tool_dir, curl_arguments, curl_not_found, fake_tool_cases, ToolDirCurl,
};
#[cfg(windows)]
use crate::host::{powershell_is_emulated, EMULATED_POWERSHELL_VAR};
#[cfg(unix)]
use crate::standalone::signal_cleans_up;
use crate::{
    archive::{ArchiveFormat, Sha256},
    cases::{
        common_cases, BaseUrlChoice, Case, Existing, Expect, PostInstall, PowerShellPlatform,
        PowerShellRun, ShPlatform, ShRun, TargetChoice,
    },
    fixture::{
        dir_entry_names, real_default_binaries, DefaultBaseUrl, Fixture, Invocation, Launch, Mode,
        Outcome, PostInstallStep, PowerShellMode, ServedArchive, BIN_NAME, CALLER_INTACT,
        OLD_BINARY, POST_INSTALL_ARGS, PROJECT,
    },
    host::host_target,
    shells::{LocatedShell, Requirement, Requirements, ShellKind, ShellSpec, REQUIRE_VAR, SHELLS},
    standalone::{
        bad_values_are_rejected, help, https_redirect_to_http, powershell_rejected_arguments,
        rejected_arguments, truncated_script,
    },
    targets::Target,
};
use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::eyre::{bail, eyre, Context, Report, Result};
use libtest_mimic::{Failed, Trial};
#[cfg(windows)]
use std::env;
use std::{fs, io};

#[derive(Clone, Copy, Debug)]
enum ShellRun {
    Sh(ShRun),
    PowerShell(PowerShellRun),
}

impl ShellRun {
    fn launch(self) -> Launch {
        match self {
            Self::Sh(run) => Launch::Sh(run.mode),
            Self::PowerShell(run) => Launch::PowerShell(run.mode),
        }
    }
}

pub(crate) fn trials() -> Result<Vec<Trial>> {
    let requirements = Requirements::from_env()?;
    // TODO-RAINCLAUDE: refuse to start if a file with the test binary's name
    // already exists in a real default install location ($CARGO_HOME/bin or
    // ~/.cargo/bin). Every case asserts that those paths don't exist
    // afterwards, so each one would fail with a misleading "the script created
    // ..." message.
    for path in real_default_binaries() {
        if path.exists() {
            bail!(
                "{path} exists; the install script tests use that name and must never create it \
                 (move it out of the way and run the tests again)"
            );
        }
    }
    // TODO-RAINCLAUDE: a case with neither run configured would produce no
    // trial anywhere, and so would silently test nothing.
    let cases = common_cases();
    #[cfg(unix)]
    let cases = [cases, fake_tool_cases()].concat();
    for case in cases {
        if case.sh.is_none() && case.powershell.is_none() {
            bail!(
                "the case {} does not run under any shell on this platform; \
                 if it only applies to another platform, gate it with #[cfg]",
                case.test_name(),
            );
        }
    }

    let mut trials = vec![Trial::test("generated::bad-values-are-rejected", || {
        bad_values_are_rejected().map_err(failed)
    })];

    for shell in SHELLS {
        let shell_path = match requirements.locate_shell(shell) {
            LocatedShell::Found(path) => path,
            LocatedShell::Missing(requirement) => {
                trials.push(missing_shell_trial(shell, requirement));
                continue;
            }
        };

        let cases = common_cases();
        #[cfg(unix)]
        let fake_tools = shell.kind == ShellKind::Sh && shell.name.honors_path();
        #[cfg(unix)]
        let cases = if fake_tools {
            [cases, fake_tool_cases()].concat()
        } else {
            cases
        };
        for case in cases {
            let name = format!("{}::{}", shell.name, case.test_name());
            let run = match shell.kind {
                ShellKind::Sh => case.sh.map(ShellRun::Sh),
                ShellKind::PowerShell => case.powershell.map(ShellRun::PowerShell),
            };
            let Some(run) = run else {
                continue;
            };

            let shell_path = shell_path.clone();
            trials.push(Trial::test(name, move || {
                run_case(shell, &shell_path, &case, run).map_err(failed)
            }));
        }

        match shell.kind {
            ShellKind::Sh => {
                for (name, test) in [
                    (
                        "truncated-script",
                        truncated_script as fn(&ShellSpec, &Utf8Path) -> Result<()>,
                    ),
                    ("rejected-arguments", rejected_arguments),
                    ("help", help),
                    ("https-redirect-to-http", https_redirect_to_http),
                    #[cfg(unix)]
                    ("signal-cleans-up", signal_cleans_up),
                ] {
                    trials.push(shell_trial(shell, &shell_path, name, test));
                }
            }
            ShellKind::PowerShell => {
                for (name, test) in [
                    (
                        "rejected-arguments",
                        powershell_rejected_arguments as fn(&ShellSpec, &Utf8Path) -> Result<()>,
                    ),
                    ("https-redirect-to-http", https_redirect_to_http),
                ] {
                    trials.push(shell_trial(shell, &shell_path, name, test));
                }
                #[cfg(windows)]
                if env::var_os(EMULATED_POWERSHELL_VAR).is_some() {
                    trials.push(shell_trial(
                        shell,
                        &shell_path,
                        "is-emulated-x64-on-arm64",
                        powershell_is_emulated,
                    ));
                }
            }
        }
        #[cfg(unix)]
        if fake_tools {
            trials.push(shell_trial(
                shell,
                &shell_path,
                "curl::arguments",
                curl_arguments,
            ));
            trials.push(shell_trial(
                shell,
                &shell_path,
                "curl::not-found",
                curl_not_found,
            ));
        }
    }

    Ok(trials)
}

fn shell_trial(
    shell: &'static ShellSpec,
    shell_path: &Utf8Path,
    name: &str,
    test: fn(&ShellSpec, &Utf8Path) -> Result<()>,
) -> Trial {
    let shell_path = shell_path.to_owned();
    Trial::test(format!("{}::{name}", shell.name), move || {
        test(shell, &shell_path).map_err(failed)
    })
}

fn failed(error: Report) -> Failed {
    format!("{error:?}").into()
}

fn missing_shell_trial(shell: &ShellSpec, requirement: Requirement) -> Trial {
    let name = shell.name;
    let (message, ignored) = match requirement {
        Requirement::Required => (
            format!("{name} was not found on PATH, but it is required (by {REQUIRE_VAR}, or because every Unix has sh)"),
            false,
        ),
        Requirement::Optional => (
            format!("{name} was not found on PATH; add it to {REQUIRE_VAR} to make this an error"),
            true,
        ),
    };
    Trial::test(
        format!("{name}::not-installed"),
        move || Err(message.into()),
    )
    .with_ignored_flag(ignored)
}

fn run_case(shell: &ShellSpec, shell_path: &Utf8Path, case: &Case, run: ShellRun) -> Result<()> {
    // * target is the target whose archive should be fetched.
    // * target_option is the target to pass to the script.
    let (target, target_option) = match run {
        ShellRun::Sh(ShRun {
            platform: ShPlatform::Real(choice),
            ..
        })
        | ShellRun::PowerShell(PowerShellRun {
            platform: PowerShellPlatform::Real(choice),
            ..
        }) => match choice {
            TargetChoice::Host => (host_target(shell.kind)?, None),
            TargetChoice::Override(target) => (Some(target), Some(target)),
        },
        #[cfg(unix)]
        ShellRun::Sh(ShRun {
            platform: ShPlatform::Fake(tools),
            ..
        }) => (tools.expected_target, None),
        ShellRun::PowerShell(PowerShellRun {
            platform:
                PowerShellPlatform::FakeArch {
                    expected_target, ..
                },
            ..
        }) => (expected_target, None),
    };
    let format = match shell.kind {
        ShellKind::Sh => ArchiveFormat::TarGz,
        ShellKind::PowerShell => ArchiveFormat::Zip,
    };
    let served = match (case.expect.archive_serving(), target) {
        (Some(serving), Some(target)) => Some(ServedArchive {
            target,
            format,
            serving,
            content: case.expect.content(),
        }),
        (Some(_), None) => {
            bail!("a case that fetches an archive must say which target it expects");
        }
        (None, Some(_) | None) => None,
    };
    let default_base_url = match case.base_url {
        BaseUrlChoice::Default => DefaultBaseUrl::Server,
        BaseUrlChoice::Server
        | BaseUrlChoice::WithSuffix(_)
        | BaseUrlChoice::WithoutScheme
        | BaseUrlChoice::PlainHttp => DefaultBaseUrl::Elsewhere,
    };
    let mut fixture = Fixture::new(
        &case.release,
        case.tweak,
        served,
        shell.kind.post_install_step(),
        default_base_url,
    )?;

    let result = run_script(
        &fixture,
        shell,
        shell_path,
        case,
        run,
        target,
        target_option,
    );
    let (mut problems, outcome) = fixture.verify_requests_after(result)?;
    if let Some(hang_ups) = fixture.take_hang_up_count() {
        let expected = match shell.kind {
            // curl only makes one attempt for dropped connections.
            ShellKind::Sh => 1..=1,
            // TODO-RAINCLAUDE: Get-Url in install.ps1 does not retry a dropped
            // connection itself (no response means status 0, which is not
            // transient), but the HTTP stack under Invoke-WebRequest does: one
            // call makes 4 connections on PowerShell 7 (seen on Linux and
            // Windows) and 2 on Windows PowerShell 5.1 (measured by hand, with a
            // listener that reads the request and hangs up). A script that
            // also retried, with its 4 attempts, would make 16 or 8, so this
            // range still catches that.
            ShellKind::PowerShell => 1..=4,
        };
        if !expected.contains(&hang_ups) {
            problems.push(format!(
                "expected {expected:?} dropped connections, saw {hang_ups}"
            ));
        }
    }
    report_problems(problems).map_err(|error| eyre!("{error}\n{outcome}"))
}

fn run_script(
    fixture: &Fixture,
    shell: &ShellSpec,
    shell_path: &Utf8Path,
    case: &Case,
    run: ShellRun,
    target: Option<Target>,
    target_option: Option<Target>,
) -> Result<(Vec<String>, Outcome)> {
    let (path, mut env): (_, Vec<(&str, Option<String>)>) = match run {
        ShellRun::Sh(ShRun {
            platform: ShPlatform::Real(_),
            ..
        }) => (None, Vec::new()),
        #[cfg(unix)]
        ShellRun::Sh(ShRun {
            platform: ShPlatform::Fake(tools),
            ..
        }) => {
            let tool_dir = fixture.tool_dir();
            build_tool_dir(&tool_dir, ToolDirCurl::Real, tools.hash_tool)?;
            (Some(tool_dir), tools.host.env())
        }
        ShellRun::PowerShell(PowerShellRun { platform, .. }) => match platform {
            PowerShellPlatform::Real(_) => (None, Vec::new()),
            PowerShellPlatform::FakeArch { arch, .. } => (None, arch.env().to_vec()),
        },
    };
    // Extend with the case's env vars last so that they take precedence over
    // anything else here.
    env.extend(
        case.env
            .iter()
            .map(|(name, value)| (*name, value.map(str::to_owned))),
    );

    match case.expect {
        Expect::FailsBeforeManifest => {}
        Expect::FailsAfterManifest
        | Expect::FailsOnArchive { .. }
        | Expect::FailsPostInstall { .. }
        | Expect::Installed { .. } => fixture.expect_manifest(case.version.manifest_spec())?,
        #[cfg(unix)]
        Expect::FailsToStartPostInstall => fixture.expect_manifest(case.version.manifest_spec())?,
    }
    if case.expect.archive_serving().is_some() {
        fixture.expect_archive()?;
    }
    let helper_exit = match case.expect {
        Expect::FailsPostInstall { helper_exit } => Some(helper_exit),
        Expect::FailsBeforeManifest
        | Expect::FailsAfterManifest
        | Expect::FailsOnArchive { .. }
        | Expect::Installed { .. } => None,
        #[cfg(unix)]
        Expect::FailsToStartPostInstall => None,
    };
    let launch = run.launch();
    match launch {
        Launch::PowerShell(PowerShellMode::Piped | PowerShellMode::ScriptBlock) => {
            fixture.expect_powershell_script()?;
        }
        Launch::PowerShell(PowerShellMode::Args)
        | Launch::Sh(Mode::Args | Mode::ArgsWithEquals | Mode::Piped) => {}
        Launch::PowerShell(PowerShellMode::File) => match case.expect {
            Expect::FailsBeforeManifest => {}
            Expect::FailsAfterManifest
            | Expect::FailsOnArchive { .. }
            | Expect::FailsPostInstall { .. }
            | Expect::Installed { .. } => {
                bail!(
                    "a -File launch cannot be told about the test certificate, \
                     so it is only for cases that fail before any download"
                );
            }
            #[cfg(unix)]
            Expect::FailsToStartPostInstall => {
                bail!(
                    "a -File launch cannot be told about the test certificate, \
                     so it is only for cases that fail before any download"
                );
            }
        },
    }

    let install_dir = fixture.install_dir(case.install_dir);
    let write_old_binary = || -> Result<Utf8PathBuf> {
        let binary = install_dir.join(bin_file_name(target)?);
        fs::create_dir_all(&install_dir).wrap_err("failed to create the install directory")?;
        fs::write(&binary, OLD_BINARY).wrap_err("failed to write the old binary")?;
        Ok(binary)
    };
    let lock: Option<fs::File> = match case.existing {
        Existing::Nothing => None,
        Existing::OldBinary => {
            write_old_binary()?;
            None
        }
        #[cfg(windows)]
        Existing::LockedOldBinary => {
            let binary = write_old_binary()?;
            Some(lock_file(&binary)?)
        }
        Existing::Directory => {
            let directory = install_dir.join(bin_file_name(target)?);
            fs::create_dir_all(&directory)
                .wrap_err_with(|| format!("failed to create {directory}"))?;
            None
        }
    };

    let base_url = match case.base_url {
        BaseUrlChoice::Default => None,
        BaseUrlChoice::Server => Some(fixture.base_url()),
        BaseUrlChoice::WithSuffix(suffix) => Some(format!("{}{suffix}", fixture.base_url())),
        BaseUrlChoice::WithoutScheme => Some(host_and_port(&fixture.base_url())?.to_owned()),
        BaseUrlChoice::PlainHttp => Some(format!("http://{}", host_and_port(&fixture.base_url())?)),
    };
    let outcome = fixture.run(
        shell,
        shell_path,
        &Invocation {
            version: case.version,
            target: target_option,
            install_dir: case.install_dir,
            env,
            path,
            helper_exit,
            ..Invocation::latest(base_url, launch)
        },
    )?;
    drop(lock);

    let problems = check_outcome(fixture, shell, case, launch, &outcome, target)?;
    Ok((problems, outcome))
}

fn host_and_port(url: &str) -> Result<&str> {
    url.strip_prefix("https://")
        .ok_or_else(|| eyre!("the test server's URL, {url}, does not start with https://"))
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
    shell: &ShellSpec,
    case: &Case,
    launch: Launch,
    outcome: &Outcome,
    target: Option<Target>,
) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    let stdout = normalize_output(&outcome.stdout);
    let stderr = normalize_output(&outcome.stderr);
    let install_dir = fixture.install_dir(case.install_dir);

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
        .progress_contains
        .iter()
        .map(|text| (*text).to_owned())
        .collect();
    let (result, post_install) = match case.expect {
        Expect::Installed { post_install, .. } => {
            expected_progress.push(format!("installed {PROJECT} {} to", case.release.version));
            if let Some(target) = target {
                expected_progress.push(format!(
                    "installing {PROJECT} {} ({target}) to",
                    case.release.version
                ));
            }
            match launch {
                Launch::PowerShell(PowerShellMode::Piped) => {
                    expected_progress.push(CALLER_INTACT.to_owned());
                }
                Launch::PowerShell(
                    PowerShellMode::Args | PowerShellMode::File | PowerShellMode::ScriptBlock,
                )
                | Launch::Sh(Mode::Args | Mode::ArgsWithEquals | Mode::Piped) => {}
            }
            let post_install = match (shell.kind.post_install_step(), post_install) {
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
            (ExpectedResult::InstalledAndSucceeded, post_install)
        }
        Expect::FailsPostInstall { .. } => (
            ExpectedResult::InstalledButFailed,
            PostInstallExpectation::Ran,
        ),
        #[cfg(unix)]
        Expect::FailsToStartPostInstall => (
            ExpectedResult::InstalledButFailed,
            PostInstallExpectation::CouldNotStart,
        ),
        Expect::FailsBeforeManifest
        | Expect::FailsAfterManifest
        | Expect::FailsOnArchive { .. } => (
            ExpectedResult::FailedWithoutInstalling,
            PostInstallExpectation::NotAttempted,
        ),
    };
    match (result, outcome.status.success()) {
        (ExpectedResult::InstalledAndSucceeded, false) => {
            problems.push("the script failed, but was expected to succeed".to_owned());
        }
        (ExpectedResult::InstalledButFailed | ExpectedResult::FailedWithoutInstalling, true) => {
            problems.push("the script succeeded, but was expected to fail".to_owned());
        }
        (ExpectedResult::InstalledAndSucceeded, true)
        | (ExpectedResult::InstalledButFailed | ExpectedResult::FailedWithoutInstalling, false) => {
        }
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
        ExpectedResult::InstalledAndSucceeded | ExpectedResult::InstalledButFailed => {
            let sha256 = fixture.served_binary_sha256().ok_or_else(|| {
                eyre!("an install is expected, but no archive is served to install from")
            })?;
            Some(ExpectedBinary::Installed(sha256))
        }
        ExpectedResult::FailedWithoutInstalling => match case.existing {
            Existing::Nothing => None,
            Existing::OldBinary => Some(ExpectedBinary::Old),
            #[cfg(windows)]
            Existing::LockedOldBinary => Some(ExpectedBinary::Old),
            Existing::Directory => Some(ExpectedBinary::EmptyDirectory),
        },
    };
    check_install_dir(&install_dir, expected_binary, target, &mut problems)?;

    // * install.sh writes all of its own text to stderr.
    // * Under PowerShell, install.ps1's info and warning lines come out on stdout.
    //   Only errors go to stderr.
    let (progress, progress_stream) = match shell.kind {
        ShellKind::Sh => (&stderr, "stderr"),
        ShellKind::PowerShell => (&stdout, "stdout"),
    };
    for text in expected_progress {
        if !progress.contains(&normalize_output(&text)) {
            problems.push(format!("{progress_stream} does not contain {text:?}"));
        }
    }
    for text in case.error_contains {
        if !stderr.contains(&normalize_output(text)) {
            problems.push(format!("stderr does not contain {text:?}"));
        }
    }
    for text in case.output_excludes {
        let text = normalize_output(text);
        for (stream, name) in [(&stdout, "stdout"), (&stderr, "stderr")] {
            if stream.contains(&text) {
                problems.push(format!("{name} contains {text:?}"));
            }
        }
    }
    match shell.kind {
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
    let strays = fixture.stray_work_dir_entries(case.install_dir)?;
    if !strays.is_empty() {
        problems.push(format!(
            "the script left {strays:?} in the directory it was run in"
        ));
    }
    for path in fixture.forbidden_paths(case.install_dir) {
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
    if present != [bin_name] {
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
enum ExpectedResult {
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
/// PowerShell 7 colors error messages and rewraps them to the console width
/// behind a "|" gutter, so we must compare text with both of these stripped
/// out.
fn normalize_output(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            plain.push(c);
            continue;
        }
        if chars.next() == Some('[') {
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
