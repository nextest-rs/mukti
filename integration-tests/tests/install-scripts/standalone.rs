// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    archive::ArchiveFormat,
    cases::RELEASE,
    fixture::{
        dir_entry_names, generate_scripts, ArchiveContent, ArchiveServing, CheckedRun,
        DefaultBaseUrl, Fixture, Given, InstallDir, Invocation, ManifestTweak, Outcome,
        PostInstallStep, ScriptOptions, ServedArchive, Value, BIN_NAME_EXE, MANIFEST_FILE_NAME,
        PROJECT,
    },
    runner::{
        check_binary_sha256, check_install_dir, check_no_install, write_old_binary, ExpectedBinary,
        ExpectedNoInstall, ExpectedNoInstallDir, ExpectedStatus, ExpectedStream,
    },
    shells::{
        PowerShellMode, PowerShellName, PowerShellPlatform, PowerShellRun, ShName, ShRun,
        ShellKind, ShellLaunch, ShellName,
    },
    targets::Target,
};
#[cfg(windows)]
use crate::{
    fixture::{run_checked, HELPER},
    host::EMULATED_POWERSHELL_VAR,
};
use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::eyre::{bail, eyre, Context, Report, Result};
#[cfg(windows)]
use std::process::{Command, Stdio};
use std::{
    collections::BTreeSet,
    fs,
    process::{Child, ExitStatus},
};

pub(crate) fn sh_truncated_script(shell: ShName, shell_path: &Utf8Path) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE)?;
    let script =
        fs::read(fixture.script(ShellKind::Sh)).wrap_err("failed to read the install script")?;

    // The body is one brace group. Before its opening brace there are only
    // comments.
    let body_start = script
        .windows(3)
        .position(|window| window == b"\n{\n")
        .ok_or_else(|| eyre!("the install script has no opening brace on its own line"))?
        + 1;
    let Some(longest) = script.strip_suffix(b"}\n").map(<[u8]>::len) else {
        bail!("the install script does not end with a closing brace and a newline");
    };

    // Sample the body to keep this fast, but try every cut in the bytes
    // immediately before the closing brace.
    let tail_start = longest.saturating_sub(32);
    let cuts = (0..tail_start).step_by(499).chain(tail_start..=longest);
    let install_dir = fixture.install_dir(InstallDir::Plain);
    let run_cuts = || -> Result<Vec<String>> {
        let mut problems = Vec::new();
        for cut in cuts {
            let outcome = fixture.run(
                shell_path,
                &Invocation {
                    options: ScriptOptions::ENV,
                    stdin: Some(script[..cut].to_vec()),
                    ..Invocation::new(ShellLaunch::Sh(shell, ShRun::PIPED))
                },
            )?;
            if install_dir.exists() {
                problems.push(format!(
                    "the first {cut} bytes of the script created {install_dir}"
                ));
                break;
            }
            if cut > body_start && outcome.status.success() {
                problems.push(format!(
                    "the first {cut} bytes of the script ran without an error:\n{outcome}"
                ));
                break;
            }
        }
        Ok(problems)
    };
    let result = run_cuts();
    fixture.finish_problems(result)
}

/// Ensures that unknown or extra arguments are rejected before the install
/// directory is created and before any request is made.
pub(crate) fn sh_rejected_arguments(shell: ShName, shell_path: &Utf8Path) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE)?;
    let install_dir = fixture.install_dir(InstallDir::Plain);
    let rows: [(&[&str], &[&str]); 8] = [
        (
            &["--bogus"],
            &["unexpected argument: --bogus", "run with --help for usage"],
        ),
        (&["stray"], &["unexpected argument: stray"]),
        (
            &["--version"],
            &[
                "option --version requires a value",
                "run with --help for usage",
            ],
        ),
        (
            &["--target="],
            &["option --target requires a non-empty value"],
        ),
        (
            &["--install-dir", ""],
            &["option --install-dir requires a non-empty value"],
        ),
        (
            &["--version="],
            &["option --version requires a non-empty value"],
        ),
        (
            &["--base-url", ""],
            &["option --base-url requires a non-empty value"],
        ),
        (
            &["--target", "not a target"],
            &["invalid target: not a target", "expected a target triple"],
        ),
    ];

    let run_rows = || -> Result<Vec<String>> {
        let mut problems = Vec::new();
        for (extra_args, expected_stderr) in rows {
            let outcome = fixture.run(
                shell_path,
                &Invocation {
                    extra_args,
                    ..Invocation::new(ShellLaunch::Sh(shell, ShRun::DEFAULT))
                },
            )?;
            let expected = ExpectedNoInstall {
                status: ExpectedStatus::Failure,
                stdout: ExpectedStream::Empty,
                stderr: ExpectedStream::Contains(expected_stderr),
                install_dir: ExpectedNoInstallDir::NotCreated,
            };
            let checked = CheckedRun {
                problems: check_no_install(&outcome, &install_dir, &expected)?,
                outcome,
            };
            problems.extend(checked.into_labeled_problem(&format!("{extra_args:?}")));
            if install_dir.exists() {
                break;
            }
        }
        Ok(problems)
    };
    let result = run_rows();
    fixture.finish_problems(result)
}

/// `install.ps1` must refuse an unknown or stray argument at the beginning of
/// the script.
pub(crate) fn powershell_rejected_arguments(
    shell: PowerShellName,
    shell_path: &Utf8Path,
) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE)?;
    let install_dir = fixture.install_dir(InstallDir::Plain);
    const OPTIONS: &str = "the options are -Version, -InstallDir, -Target, and -BaseUrl";
    const EMPTY: Given = Given::arg(Value::Literal(""));
    let args = ScriptOptions::ARGS;
    let rows: [(PowerShellMode, ScriptOptions, &[&str], &[&str]); 9] = [
        (
            PowerShellMode::File,
            args,
            &["-Bogus", "x"],
            &["'Bogus'", OPTIONS],
        ),
        (
            PowerShellMode::CallOperator,
            args,
            &["-Bogus", "x"],
            &["'Bogus'", OPTIONS],
        ),
        (
            PowerShellMode::File,
            args,
            &["stray"],
            &["'stray'", OPTIONS],
        ),
        (
            PowerShellMode::CallOperator,
            args,
            &["stray"],
            &["'stray'", OPTIONS],
        ),
        // Why no File rejection for --version? Because PowerShell in file mode
        // actually translates `--version` to `-Version`. Pretty wild, right?
        (
            PowerShellMode::CallOperator,
            args,
            &["--version", "1.2.3"],
            &["'--version'", OPTIONS],
        ),
        (
            PowerShellMode::CallOperator,
            args.with_version(EMPTY),
            &[],
            &["option -Version requires a non-empty value"],
        ),
        (
            PowerShellMode::CallOperator,
            args.with_install_dir(EMPTY),
            &[],
            &["option -InstallDir requires a non-empty value"],
        ),
        (
            PowerShellMode::CallOperator,
            args.with_target(EMPTY),
            &[],
            &["option -Target requires a non-empty value"],
        ),
        (
            PowerShellMode::CallOperator,
            args.with_base_url(EMPTY),
            &[],
            &["option -BaseUrl requires a non-empty value"],
        ),
    ];

    let run_rows = || -> Result<Vec<String>> {
        let mut problems = Vec::new();
        for (mode, options, extra_args, expected_stderr) in rows {
            let outcome = fixture.run(
                shell_path,
                &Invocation {
                    options,
                    extra_args,
                    ..Invocation::new(ShellLaunch::PowerShell(
                        shell,
                        PowerShellRun {
                            mode,
                            platform: PowerShellPlatform::Host,
                        },
                    ))
                },
            )?;
            let expected = ExpectedNoInstall {
                status: ExpectedStatus::Failure,
                stdout: ExpectedStream::Empty,
                stderr: ExpectedStream::Contains(expected_stderr),
                install_dir: ExpectedNoInstallDir::NotCreated,
            };
            let checked = CheckedRun {
                problems: check_no_install(&outcome, &install_dir, &expected)?,
                outcome,
            };
            problems.extend(
                checked.into_labeled_problem(&format!("{mode:?} {options:?} {extra_args:?}")),
            );
            if install_dir.exists() {
                break;
            }
        }
        Ok(problems)
    };
    let result = run_rows();
    fixture.finish_problems(result)
}

pub(crate) fn sh_help(shell: ShName, shell_path: &Utf8Path) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE)?;
    let install_dir = fixture.install_dir(InstallDir::Plain);

    let expected = ExpectedNoInstall {
        status: ExpectedStatus::Success,
        stdout: ExpectedStream::Contains(&[
            "Install a pre-built widget-project binary.",
            "Usage: install.sh [OPTIONS]",
            "--version <VERSION>",
            "[env: WIDGET_INSTALL_VERSION]",
            "Directory to install widget into",
            "[default: $CARGO_HOME/bin, or ~/.cargo/bin]",
            "[default: https://get.widget.example]",
            "For more information, see https://widget.example/docs/install/",
        ]),
        stderr: ExpectedStream::Empty,
        install_dir: ExpectedNoInstallDir::NotCreated,
    };
    let run_flags = || -> Result<Vec<String>> {
        let mut problems = Vec::new();
        for flag in ["--help", "-h"] {
            let outcome = fixture.run(
                shell_path,
                &Invocation {
                    extra_args: &[flag],
                    ..Invocation::new(ShellLaunch::Sh(shell, ShRun::DEFAULT))
                },
            )?;
            let checked = CheckedRun {
                problems: check_no_install(&outcome, &install_dir, &expected)?,
                outcome,
            };
            problems.extend(checked.into_labeled_problem(flag));
            if install_dir.exists() {
                break;
            }
        }
        Ok(problems)
    };
    let result = run_flags();
    fixture.finish_problems(result)
}

/// Tests that a script signalled in the middle of a download exits with
/// the signal's status and leaves nothing behind in the install directory.
///
/// dash does not run its EXIT trap when a signal kills it, so this depends on
/// the script's signal traps.
#[cfg(unix)]
pub(crate) fn sh_signal_cleans_up(
    shell: ShName,
    shell_path: &Utf8Path,
    signal: Signal,
) -> Result<()> {
    interrupt_mid_download(
        shell_path,
        ShellLaunch::Sh(shell, ShRun::DEFAULT),
        Interrupt::Signal(signal),
    )
}

/// Tests that if a PowerShell script is sent a Ctrl-C mid-download, it
/// successfully cleans up.
///
/// On platforms other than Windows, we send `SIGINT`, not `SIGTERM`, because
/// with the latter `pwsh` exits immediately without running any `finally`
/// blocks.
pub(crate) fn powershell_interrupt_cleans_up(
    shell: PowerShellName,
    shell_path: &Utf8Path,
) -> Result<()> {
    interrupt_mid_download(
        shell_path,
        ShellLaunch::PowerShell(
            shell,
            PowerShellRun {
                mode: PowerShellMode::CallOperator,
                platform: PowerShellPlatform::Host,
            },
        ),
        Interrupt::CtrlC,
    )
}

#[derive(Clone, Copy, Debug)]
enum Interrupt {
    #[cfg(unix)]
    Signal(Signal),
    CtrlC,
}

impl Interrupt {
    fn send(self, pid: u32) -> Result<()> {
        match self {
            #[cfg(unix)]
            Self::Signal(signal) => send_signal(pid, signal),
            Self::CtrlC => send_ctrl_c(pid),
        }
    }

    fn status_problem(self, status: ExitStatus) -> Option<String> {
        match self {
            #[cfg(unix)]
            Self::Signal(signal) => {
                let expected_code = signal.exit_code();
                (status.code() != Some(expected_code)).then(|| {
                    format!("the script exited with {status}, expected exit code {expected_code}")
                })
            }
            Self::CtrlC => (!exited_after_handling_ctrl_c(status)).then(|| {
                format!(
                    "PowerShell exited with {status}, expected a nonzero exit code from its \
                     Ctrl-C handling"
                )
            }),
        }
    }
}

fn interrupt_mid_download(
    shell_path: &Utf8Path,
    launch: ShellLaunch,
    interrupt: Interrupt,
) -> Result<()> {
    // Pass in an explicit target so that this test doesn't depend on what the
    // host is.
    let (target, format) = match launch.kind() {
        ShellKind::Sh => (Target::X86_64_UNKNOWN_FREEBSD, ArchiveFormat::TarGz),
        ShellKind::PowerShell => (Target::X86_64_PC_WINDOWS_MSVC, ArchiveFormat::Zip),
    };
    let mut fixture = Fixture::new(
        &RELEASE,
        ManifestTweak::None,
        Some(ServedArchive {
            target,
            format,
            serving: ArchiveServing::Stall,
            content: ArchiveContent::Helper,
        }),
        PostInstallStep::Configured,
        DefaultBaseUrl::Unreachable,
    )?;
    let result = run_interrupted(&fixture, shell_path, launch, interrupt, target);
    fixture.finish(result)
}

fn run_interrupted(
    fixture: &Fixture,
    shell_path: &Utf8Path,
    launch: ShellLaunch,
    interrupt: Interrupt,
    target: Target,
) -> Result<CheckedRun> {
    fixture.expect_manifest("latest")?;

    let install_dir = fixture.install_dir(InstallDir::Plain);
    write_old_binary(&install_dir, Some(target))?;

    let child = fixture.spawn(
        shell_path,
        &Invocation {
            options: ScriptOptions::ARGS.with_target(Given::arg(Value::Target(target))),
            ..Invocation::new(launch)
        },
    )?;

    let interrupted = fixture
        .wait_for_stalled_request()
        .and_then(|connection| interrupt.send(child.id()).map(|()| connection));
    let connection = match interrupted {
        Ok(connection) => connection,
        Err(error) => return kill_and_fail(child, shell_path, error),
    };
    let output = match interrupt {
        // We can let the connection drop at this point.
        #[cfg(unix)]
        Interrupt::Signal(_) => {
            drop(connection);
            child.wait_with_output()
        }
        // Unlike `install.sh`'s curl, Invoke-WebRequest runs in the same process as
        // `pwsh`, and Ctrl-C would cancel the request in flight. So pwsh exits
        // while we still hold the stalled connection open. We only drop it
        // afterwards.
        Interrupt::CtrlC => {
            let output = child.wait_with_output();
            drop(connection);
            output
        }
    }
    .wrap_err_with(|| format!("failed to wait for {shell_path}"))?;
    let outcome = Outcome::from_output(output);

    let mut problems = Vec::new();
    problems.extend(interrupt.status_problem(outcome.status));
    // `install.ps1`'s temp directory is inside the install directory, so if the
    // cleanup `finally` block is skipped, it'll show up here as an extra entry.
    check_install_dir(
        &install_dir,
        Some(ExpectedBinary::Old),
        Some(target),
        &mut problems,
    )?;
    Ok(CheckedRun { problems, outcome })
}

/// Test that `install.ps1` removes a backup directory left by an earlier run
/// only if it holds nothing but the backup file.
pub(crate) fn powershell_removes_old_backups(
    shell: PowerShellName,
    shell_path: &Utf8Path,
) -> Result<()> {
    let target = Target::X86_64_PC_WINDOWS_MSVC;
    let mut fixture = replacement_fixture(target)?;
    let result = run_with_old_backups(&fixture, shell, shell_path, target);
    fixture.finish(result)
}

fn replacement_fixture(target: Target) -> Result<Fixture> {
    Fixture::new(
        &RELEASE,
        ManifestTweak::None,
        Some(ServedArchive {
            target,
            format: ArchiveFormat::Zip,
            serving: ArchiveServing::Normal,
            content: ArchiveContent::Helper,
        }),
        PostInstallStep::Configured,
        DefaultBaseUrl::Unreachable,
    )
}

fn replacement_invocation(shell: PowerShellName, target: Target) -> Invocation<'static> {
    Invocation {
        options: ScriptOptions::ARGS.with_target(Given::arg(Value::Target(target))),
        ..Invocation::new(ShellLaunch::PowerShell(shell, PowerShellRun::DEFAULT))
    }
}

fn run_with_old_backups(
    fixture: &Fixture,
    shell: PowerShellName,
    shell_path: &Utf8Path,
    target: Target,
) -> Result<CheckedRun> {
    fixture.expect_manifest("latest")?;
    fixture.expect_archive()?;

    let install_dir = fixture.install_dir(InstallDir::Plain);
    write_old_binary(&install_dir, Some(target))?;
    let backup_name = powershell_backup_prefix();
    let write = |path: Utf8PathBuf| {
        fs::write(&path, "old\n").wrap_err_with(|| format!("failed to write {path}"))
    };
    let make_dir = |path: Utf8PathBuf| {
        fs::create_dir(&path).wrap_err_with(|| format!("failed to create {path}"))
    };
    let only_backup = install_dir.join(format!("{backup_name}only"));
    make_dir(only_backup.clone())?;
    write(only_backup.join(&backup_name))?;
    let with_extra = install_dir.join(format!("{backup_name}extra"));
    make_dir(with_extra.clone())?;
    write(with_extra.join(&backup_name))?;
    write(with_extra.join("keep.txt"))?;
    let with_subdir = install_dir.join(format!("{backup_name}subdir"));
    make_dir(with_subdir.clone())?;
    write(with_subdir.join(&backup_name))?;
    make_dir(with_subdir.join("sub"))?;
    let not_a_dir = install_dir.join(format!("{backup_name}file"));
    write(not_a_dir.clone())?;

    let outcome = fixture.run(shell_path, &replacement_invocation(shell, target))?;
    let mut problems = Vec::new();
    if !outcome.status.success() {
        problems.push(format!("the script failed with {}", outcome.status));
    }
    let expected_entries: BTreeSet<String> = [
        BIN_NAME_EXE.to_owned(),
        format!("{backup_name}extra"),
        format!("{backup_name}subdir"),
        format!("{backup_name}file"),
    ]
    .into_iter()
    .collect();
    let entries = dir_entry_names(&install_dir)?;
    if entries != expected_entries {
        problems.push(format!(
            "the install directory contains {entries:?}, expected {expected_entries:?}"
        ));
    }
    for (dir, expected) in [
        (
            &with_extra,
            vec![backup_name.clone(), "keep.txt".to_owned()],
        ),
        (&with_subdir, vec![backup_name.clone(), "sub".to_owned()]),
    ] {
        let inside = dir_entry_names(dir)?;
        let expected: BTreeSet<String> = expected.into_iter().collect();
        if inside != expected {
            problems.push(format!("{dir} contains {inside:?}, expected {expected:?}"));
        }
    }
    check_new_binary(fixture, &install_dir, &mut problems)?;
    Ok(CheckedRun { problems, outcome })
}

fn check_new_binary(
    fixture: &Fixture,
    install_dir: &Utf8Path,
    problems: &mut Vec<String>,
) -> Result<()> {
    let expected = fixture
        .served_binary_sha256()
        .ok_or_else(|| eyre!("the fixture serves an archive with a binary"))?;
    check_binary_sha256(&install_dir.join(BIN_NAME_EXE), expected, problems)
}

/// On Windows, running executables can't be replaced, but they can be renamed.
/// Test that `install.ps1` can accomplish this as necessary.
#[cfg(windows)]
pub(crate) fn powershell_replaces_running_binary(
    shell: PowerShellName,
    shell_path: &Utf8Path,
) -> Result<()> {
    let target = Target::X86_64_PC_WINDOWS_MSVC;
    let mut fixture = replacement_fixture(target)?;
    let result = run_over_running_binary(&fixture, shell, shell_path, target);
    fixture.finish(result)
}

#[cfg(windows)]
fn run_over_running_binary(
    fixture: &Fixture,
    shell: PowerShellName,
    shell_path: &Utf8Path,
    target: Target,
) -> Result<CheckedRun> {
    fixture.expect_manifest("latest")?;
    fixture.expect_archive()?;

    let install_dir = fixture.install_dir(InstallDir::Plain);
    fs::create_dir_all(&install_dir).wrap_err("failed to create the install directory")?;
    // Windows ignores data after a PE image, so this still runs properly but
    // has a different hash.
    let mut old_binary =
        fs::read(HELPER).wrap_err_with(|| format!("failed to read the test helper at {HELPER}"))?;
    old_binary.extend_from_slice(b"old binary");
    let old_path = install_dir.join(BIN_NAME_EXE);
    fs::write(&old_path, &old_binary).wrap_err_with(|| format!("failed to write {old_path}"))?;
    let mut running = Command::new(&old_path)
        .arg("wait-for-stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .wrap_err_with(|| format!("failed to start {old_path}"))?;

    let outcome = fixture.run(shell_path, &replacement_invocation(shell, target));
    drop(running.stdin.take());
    let running_status = running
        .wait()
        .wrap_err_with(|| format!("failed to wait for {old_path}"))?;
    let outcome = outcome?;

    let mut problems = Vec::new();
    if !outcome.status.success() {
        problems.push(format!("the script failed with {}", outcome.status));
    }
    if !running_status.success() {
        problems.push(format!("the old binary exited with {running_status}"));
    }
    check_new_binary(fixture, &install_dir, &mut problems)?;
    let backup_name = powershell_backup_prefix();
    let backups: Vec<String> = dir_entry_names(&install_dir)?
        .into_iter()
        .filter(|name| name.as_str() != BIN_NAME_EXE)
        .collect();
    match backups.as_slice() {
        [backup] if backup.starts_with(&backup_name) => {
            let moved = install_dir.join(backup).join(&backup_name);
            match fs::read(&moved) {
                Ok(contents) if contents == old_binary => {}
                Ok(_) => problems.push(format!("{moved} is not the old binary")),
                Err(error) => problems.push(format!("failed to read {moved}: {error}")),
            }
        }
        _ => problems.push(format!(
            "besides {BIN_NAME_EXE}, the install directory contains {backups:?}, expected one \
             {backup_name}* directory holding the running binary"
        )),
    }
    Ok(CheckedRun { problems, outcome })
}

fn kill_and_fail<T>(mut child: Child, shell_path: &Utf8Path, error: Report) -> Result<T> {
    // The script most likely exited before making the request (its output says
    // why). Here, the kill is best-effort.
    let _ = child.kill();
    let output = child
        .wait_with_output()
        .wrap_err_with(|| format!("failed to wait for {shell_path} after this error: {error:?}"))?;
    bail!("{error:?}\n{}", Outcome::from_output(output));
}

#[cfg(unix)]
fn send_ctrl_c(pid: u32) -> Result<()> {
    send_signal(pid, Signal::Int)
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug)]
pub(crate) enum Signal {
    Hup,
    Int,
    Term,
}

#[cfg(unix)]
impl Signal {
    pub(crate) const ALL: [Self; 3] = [Self::Hup, Self::Int, Self::Term];

    fn name(self) -> &'static str {
        match self {
            Self::Hup => "HUP",
            Self::Int => "INT",
            Self::Term => "TERM",
        }
    }

    pub(crate) fn test_name(self) -> &'static str {
        match self {
            Self::Hup => "hup",
            Self::Int => "int",
            Self::Term => "term",
        }
    }

    fn number(self) -> libc::c_int {
        match self {
            Self::Hup => libc::SIGHUP,
            Self::Int => libc::SIGINT,
            Self::Term => libc::SIGTERM,
        }
    }

    /// Returns the expected exit code for this script.
    ///
    /// The script's traps exit with 128 plus the signal's number.
    fn exit_code(self) -> i32 {
        128 + self.number()
    }
}

#[cfg(unix)]
fn send_signal(pid: u32, signal: Signal) -> Result<()> {
    let pid = libc::pid_t::try_from(pid)
        .wrap_err_with(|| format!("process ID {pid} does not fit in a pid_t"))?;
    // SAFETY: Any pid and signal number are sound.
    if unsafe { libc::kill(pid, signal.number()) } != 0 {
        return Err(std::io::Error::last_os_error())
            .wrap_err_with(|| format!("failed to send SIG{} to process {pid}", signal.name()));
    }
    Ok(())
}

/// On Windows, Ctrl-C is scoped to a console. See the documentation on
/// `send_ctrl_c` in `test-helpers/mukti-test-helper.rs`.
#[cfg(windows)]
fn send_ctrl_c(pid: u32) -> Result<()> {
    run_checked(
        Command::new(HELPER).args(["send-ctrl-c", &pid.to_string()]),
        "the test helper sending Ctrl-C",
    )?;
    Ok(())
}

/// Returns true if PowerShell caught the Ctrl-C and exited by itself with a
/// nonzero code (1 for PowerShell 7.6.7 off Windows).
///
/// If `SIGINT` killed PowerShell instead, there is no exit code, and its Ctrl-C
/// handling (including the script's finally blocks) never ran.
#[cfg(unix)]
fn exited_after_handling_ctrl_c(status: ExitStatus) -> bool {
    match status.code() {
        Some(0) | None => false,
        Some(_) => true,
    }
}

/// STATUS_CONTROL_C_EXIT means the process ended on Ctrl-C without handling it.
#[cfg(windows)]
fn exited_after_handling_ctrl_c(status: ExitStatus) -> bool {
    use windows_sys::Win32::Foundation::STATUS_CONTROL_C_EXIT;
    match status.code() {
        Some(0) | None => false,
        Some(code) => code != STATUS_CONTROL_C_EXIT,
    }
}

/// Returns the prefix that install.ps1 uses for its backup directories.
fn powershell_backup_prefix() -> String {
    format!("__{PROJECT}_backup")
}

/// Tests that an HTTPS redirect to HTTP fails.
pub(crate) fn https_redirect_to_http(shell: ShellName, shell_path: &Utf8Path) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE)?;
    let base_url = fixture.expect_manifest_redirect_to_http();
    let manifest_url = format!("{}/latest/{MANIFEST_FILE_NAME}", fixture.resolve(base_url)?);
    let expected_error = match shell {
        ShellName::Sh(_) => {
            format!("failed to download the install manifest from {manifest_url}")
        }
        ShellName::PowerShell(PowerShellName::Pwsh) => format!("failed to download {manifest_url}"),
        #[cfg(windows)]
        ShellName::PowerShell(PowerShellName::WindowsPowerShell) => {
            // Unfortunately, Windows PowerShell 5.1 does follow the redirect --
            // there's no way to avoid this. But our script then refuses to
            // install the result.
            fixture.expect_manifest("latest")?;
            "refusing a redirect from https to plain http".to_owned()
        }
    };
    // PowerShell must be launched with `-Command` to be told about the test
    // certificate.
    let launch = match shell {
        ShellName::Sh(shell) => ShellLaunch::Sh(shell, ShRun::DEFAULT),
        ShellName::PowerShell(shell) => {
            fixture.expect_powershell_script()?;
            ShellLaunch::PowerShell(
                shell,
                PowerShellRun {
                    mode: PowerShellMode::ScriptBlock,
                    // The default PowerShellRun configuration configures a fake
                    // x86_64 Windows on non-Windows platforms. We use that here
                    // because this test needs to get to the point where it
                    // fetches a manifest, and that can only happen after it gets
                    // past platform detection.
                    ..PowerShellRun::DEFAULT
                },
            )
        }
    };

    let result = fixture
        .run(
            shell_path,
            &Invocation {
                options: ScriptOptions {
                    base_url: Given::arg(base_url),
                    ..ScriptOptions::ARGS
                },
                ..Invocation::new(launch)
            },
        )
        .and_then(|outcome| {
            let expected = ExpectedNoInstall {
                status: ExpectedStatus::Failure,
                stdout: match shell {
                    ShellName::Sh(_) => ExpectedStream::Empty,
                    ShellName::PowerShell(_) => powershell_stdout_before_manifest(),
                },
                stderr: ExpectedStream::Contains(&[&expected_error]),
                install_dir: ExpectedNoInstallDir::Empty,
            };
            let problems =
                check_no_install(&outcome, &fixture.install_dir(InstallDir::Plain), &expected)?;
            Ok(CheckedRun { problems, outcome })
        });
    fixture.finish(result)
}

fn powershell_stdout_before_manifest() -> ExpectedStream<'static> {
    #[cfg(windows)]
    if std::env::var_os(EMULATED_POWERSHELL_VAR).is_some() {
        return ExpectedStream::Contains(&["preferring ARM64 builds"]);
    }
    ExpectedStream::Empty
}

pub(crate) fn bad_values_are_rejected() -> Result<()> {
    let temp = camino_tempfile::tempdir().wrap_err("failed to create a temporary directory")?;
    let expect_rejected = |name: &str, project: &str, flags: &[&str], message: &str| {
        let out_dir = temp.path().join(name);
        let output = generate_scripts(project, &out_dir, flags)?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() {
            bail!("{name}: the value was accepted");
        }
        if !stderr.contains(message) {
            bail!("{name}: rejected without the expected message {message:?}:\n{stderr}");
        }
        if out_dir.exists() {
            bail!("{name}: rejected, but {out_dir} was still created");
        }
        Ok(())
    };

    let bad_flag_values = [
        ("--bin-name", "$(id)"),
        ("--base-url", "http://get.widget.example"),
        ("--docs-url", "https://widget.example/\"; exit"),
        ("--env-prefix", "widget_install"),
        ("--post-install-args", "self $(id)"),
        ("--post-install-min-version", "0.9.62-rc.1"),
    ];
    for (index, (flag, value)) in bad_flag_values.into_iter().enumerate() {
        let flags = PostInstallStep::Configured.script_flags_with(flag, value)?;
        expect_rejected(&format!("flag-{index}"), PROJECT, &flags, flag)?;
    }

    expect_rejected(
        "project",
        "x'; id; '",
        &PostInstallStep::Configured.script_flags(),
        "cannot be used in an install script",
    )
}
