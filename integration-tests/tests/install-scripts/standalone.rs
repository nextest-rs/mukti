// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#[cfg(windows)]
use crate::fixture::HELPER;
use crate::{
    archive::ArchiveFormat,
    cases::{PowerShellRun, RELEASE},
    fixture::{
        generate_scripts, ArchiveContent, ArchiveServing, CheckedRun, DefaultBaseUrl, Fixture,
        Given, InstallDir, Invocation, ManifestTweak, Outcome, PostInstallStep, PowerShellMode,
        ScriptOptions, ServedArchive, ShMode, ShellLaunch, Value, MANIFEST_FILE_NAME, PROJECT,
    },
    runner::{
        check_install_dir, normalize_output, report_problems, write_old_binary, ExpectedBinary,
    },
    shells::{PowerShellName, ShName, ShellKind, ShellName},
    targets::Target,
};
use camino::Utf8Path;
use color_eyre::eyre::{bail, eyre, Context, Report, Result};
use std::{
    fs,
    process::{Child, Command, ExitStatus, Stdio},
};

pub(crate) fn sh_truncated_script(shell: ShName, shell_path: &Utf8Path) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE, ShellKind::Sh)?;
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
    let mut problems = Vec::new();
    for cut in cuts {
        let outcome = fixture.run(
            shell_path,
            &Invocation {
                options: ScriptOptions::ENV,
                stdin: Some(script[..cut].to_vec()),
                ..Invocation::new(ShellLaunch::Sh(shell, ShMode::Piped))
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
    fixture.verify_requests(&mut problems);
    report_problems(problems)
}

/// Ensures that unknown or extra arguments are rejected before the install
/// directory is created and before any request is made.
pub(crate) fn sh_rejected_arguments(shell: ShName, shell_path: &Utf8Path) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE, ShellKind::Sh)?;
    let install_dir = fixture.install_dir(InstallDir::Plain);
    let rows: [(&[&str], &[&str]); 6] = [
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
            &["--target", "not a target"],
            &["invalid target: not a target", "expected a target triple"],
        ),
    ];

    let mut problems = Vec::new();
    for (extra_args, expected) in rows {
        let outcome = fixture.run(
            shell_path,
            &Invocation {
                extra_args,
                ..Invocation::new(ShellLaunch::Sh(shell, ShMode::File))
            },
        )?;
        if outcome.status.success() {
            problems.push(format!("{extra_args:?}: the script succeeded:\n{outcome}"));
        }
        for text in expected {
            if !outcome.stderr.contains(text) {
                problems.push(format!(
                    "{extra_args:?}: stderr does not contain {text:?}:\n{outcome}"
                ));
            }
        }
        if !outcome.stdout.is_empty() {
            problems.push(format!(
                "{extra_args:?}: the script wrote to stdout:\n{outcome}"
            ));
        }
        if install_dir.exists() {
            problems.push(format!("{extra_args:?}: the script created {install_dir}"));
            break;
        }
    }
    fixture.verify_requests(&mut problems);
    report_problems(problems)
}

/// `install.ps1` must refuse an unknown or stray argument at the beginning of
/// the script.
pub(crate) fn powershell_rejected_arguments(
    shell: PowerShellName,
    shell_path: &Utf8Path,
) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE, ShellKind::PowerShell)?;
    let install_dir = fixture.install_dir(InstallDir::Plain);
    let rows: [(PowerShellMode, &[&str], &str); 5] = [
        (PowerShellMode::File, &["-Bogus", "x"], "'Bogus'"),
        (PowerShellMode::CallOperator, &["-Bogus", "x"], "'Bogus'"),
        (PowerShellMode::File, &["stray"], "'stray'"),
        (PowerShellMode::CallOperator, &["stray"], "'stray'"),
        // Why no File rejection for --version? Because PowerShell in file mode
        // actually translates `--version` to `-Version`. Pretty wild, right?
        (
            PowerShellMode::CallOperator,
            &["--version", "1.2.3"],
            "'--version'",
        ),
    ];

    let mut problems = Vec::new();
    for (mode, extra_args, named) in rows {
        let outcome = fixture.run(
            shell_path,
            &Invocation {
                extra_args,
                ..Invocation::new(ShellLaunch::PowerShell(shell, mode))
            },
        )?;
        if outcome.status.success() {
            problems.push(format!(
                "{mode:?} {extra_args:?}: the script succeeded:\n{outcome}"
            ));
        }
        let stderr = normalize_output(&outcome.stderr);
        for text in [
            named,
            "the options are -Version, -InstallDir, -Target, and -BaseUrl",
        ] {
            if !stderr.contains(text) {
                problems.push(format!(
                    "{mode:?} {extra_args:?}: stderr does not contain {text:?}:\n{outcome}"
                ));
            }
        }
        if install_dir.exists() {
            problems.push(format!(
                "{mode:?} {extra_args:?}: the script created {install_dir}"
            ));
            break;
        }
    }
    fixture.verify_requests(&mut problems);
    report_problems(problems)
}

pub(crate) fn sh_help(shell: ShName, shell_path: &Utf8Path) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE, ShellKind::Sh)?;
    let install_dir = fixture.install_dir(InstallDir::Plain);

    let mut problems = Vec::new();
    for flag in ["--help", "-h"] {
        let outcome = fixture.run(
            shell_path,
            &Invocation {
                extra_args: &[flag],
                ..Invocation::new(ShellLaunch::Sh(shell, ShMode::File))
            },
        )?;
        if !outcome.status.success() {
            problems.push(format!("{flag}: the script failed:\n{outcome}"));
        }
        for text in [
            "Install a pre-built widget-project binary.",
            "Usage: install.sh [OPTIONS]",
            "--version <VERSION>",
            "[env: WIDGET_INSTALL_VERSION]",
            "Directory to install widget into",
            "[default: $CARGO_HOME/bin, or ~/.cargo/bin]",
            "[default: https://get.widget.example]",
            "For more information, see https://widget.example/docs/install/",
        ] {
            if !outcome.stdout.contains(text) {
                problems.push(format!(
                    "{flag}: stdout does not contain {text:?}:\n{outcome}"
                ));
            }
        }
        if !outcome.stderr.is_empty() {
            problems.push(format!("{flag}: the script wrote to stderr:\n{outcome}"));
        }
        if install_dir.exists() {
            problems.push(format!("{flag}: the script created {install_dir}"));
            break;
        }
    }
    fixture.verify_requests(&mut problems);
    report_problems(problems)
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
    // Pass in an explicit target so that this test doesn't depend on what the
    // host is.
    let target = Target::X86_64_UNKNOWN_FREEBSD;
    let mut fixture = Fixture::new(
        &RELEASE,
        ManifestTweak::None,
        Some(ServedArchive {
            target,
            format: ArchiveFormat::TarGz,
            serving: ArchiveServing::Stall,
            content: ArchiveContent::Helper,
        }),
        ShellKind::Sh.post_install_step(),
        DefaultBaseUrl::Unreachable,
    )?;
    let result = sh_signal_mid_download(&fixture, shell, shell_path, target, signal);
    fixture.finish(result)
}

#[cfg(unix)]
fn sh_signal_mid_download(
    fixture: &Fixture,
    shell: ShName,
    shell_path: &Utf8Path,
    target: Target,
    signal: Signal,
) -> Result<CheckedRun> {
    fixture.expect_manifest("latest")?;

    let install_dir = fixture.install_dir(InstallDir::Plain);
    write_old_binary(&install_dir, Some(target))?;

    let child = fixture.spawn(
        shell_path,
        &Invocation {
            options: ScriptOptions {
                target: Given::arg(Value::Target(target)),
                ..ScriptOptions::ARGS
            },
            ..Invocation::new(ShellLaunch::Sh(shell, ShMode::File))
        },
    )?;

    let signalled = fixture
        .wait_for_stalled_request()
        .and_then(|connection| send_signal(child.id(), signal).map(|()| connection));
    let connection = match signalled {
        Ok(connection) => connection,
        Err(error) => return kill_and_fail(child, shell_path, error),
    };
    // We can let the connection drop at this point.
    drop(connection);

    let output = child
        .wait_with_output()
        .wrap_err_with(|| format!("failed to wait for {shell_path}"))?;
    let outcome = Outcome::from_output(output);

    let mut problems = Vec::new();
    let expected_code = signal.exit_code();
    if outcome.status.code() != Some(expected_code) {
        problems.push(format!(
            "the script exited with {}, expected exit code {expected_code}",
            outcome.status
        ));
    }
    check_install_dir(
        &install_dir,
        Some(ExpectedBinary::Old),
        Some(target),
        &mut problems,
    )?;
    Ok(CheckedRun { problems, outcome })
}

/// We send Ctrl-C  TODO-RAINCLAUDE: we send Ctrl-C (SIGINT off Windows) mid-download, then check for cleanup.
// TODO-RAINCLAUDE: not SIGTERM off Windows: on SIGTERM, pwsh exits at once without finally blocks.
pub(crate) fn powershell_interrupt_cleans_up(
    shell: PowerShellName,
    shell_path: &Utf8Path,
) -> Result<()> {
    let target = Target::X86_64_PC_WINDOWS_MSVC;
    let mut fixture = Fixture::new(
        &RELEASE,
        ManifestTweak::None,
        Some(ServedArchive {
            target,
            format: ArchiveFormat::Zip,
            serving: ArchiveServing::Stall,
            content: ArchiveContent::Helper,
        }),
        ShellKind::PowerShell.post_install_step(),
        DefaultBaseUrl::Unreachable,
    )?;
    let result = powershell_interrupt_mid_download(&fixture, shell, shell_path, target);
    fixture.finish(result)
}

fn powershell_interrupt_mid_download(
    fixture: &Fixture,
    shell: PowerShellName,
    shell_path: &Utf8Path,
    target: Target,
) -> Result<CheckedRun> {
    fixture.expect_manifest("latest")?;

    let install_dir = fixture.install_dir(InstallDir::Plain);
    write_old_binary(&install_dir, Some(target))?;

    let child = fixture.spawn(
        shell_path,
        &Invocation {
            options: ScriptOptions {
                target: Given::arg(Value::Target(target)),
                ..ScriptOptions::ARGS
            },
            ..Invocation::new(ShellLaunch::PowerShell(shell, PowerShellMode::CallOperator))
        },
    )?;

    let interrupted = fixture
        .wait_for_stalled_request()
        .and_then(|connection| send_ctrl_c(child.id()).map(|()| connection));
    // TODO-RAINCLAUDE: unlike sh, pwsh exits without the stalled request being released.
    let connection = match interrupted {
        Ok(connection) => connection,
        Err(error) => return kill_and_fail(child, shell_path, error),
    };
    let output = child
        .wait_with_output()
        .wrap_err_with(|| format!("failed to wait for {shell_path}"))?;
    drop(connection);
    let outcome = Outcome::from_output(output);

    let mut problems = Vec::new();
    if !exited_after_handling_ctrl_c(outcome.status) {
        problems.push(format!(
            "PowerShell exited with {}, expected a nonzero exit code from its Ctrl-C handling",
            outcome.status
        ));
    }
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

fn kill_and_fail<T>(mut child: Child, shell_path: &Utf8Path, error: Report) -> Result<T> {
    // TODO-RAINCLAUDE: the script most likely exited before making
    // the request; its output says why. The kill is best-effort.
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
    // kill is a builtin in every shell, while some minimal systems do not
    // have a kill program.
    let name = signal.name();
    let status = Command::new("sh")
        .args([
            "-c",
            &format!("kill -{name} \"$1\""),
            "kill",
            &pid.to_string(),
        ])
        .stdin(Stdio::null())
        .status()
        .wrap_err_with(|| format!("failed to run sh to send SIG{name}"))?;
    if !status.success() {
        bail!("kill -{name} failed with {status}");
    }
    Ok(())
}

/// On Windows, Ctrl-C is scoped to a console. See the documentation on
/// `send_ctrl_c` in `test-helpers/mukti-test-helper.rs`.
#[cfg(windows)]
fn send_ctrl_c(pid: u32) -> Result<()> {
    let output = Command::new(HELPER)
        .args(["send-ctrl-c", &pid.to_string()])
        .stdin(Stdio::null())
        .output()
        .wrap_err("failed to run the test helper to send Ctrl-C")?;
    if !output.status.success() {
        bail!(
            "sending Ctrl-C with the test helper failed with {}:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
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

/// Tests that an HTTPS redirect to HTTP fails.
pub(crate) fn https_redirect_to_http(shell: ShellName, shell_path: &Utf8Path) -> Result<()> {
    let mut fixture = Fixture::new(
        &RELEASE,
        ManifestTweak::None,
        None,
        shell.kind().post_install_step(),
        DefaultBaseUrl::Unreachable,
    )?;
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
    // TODO-RAINCLAUDE: PowerShell needs a -Command launch to be told about
    // the test certificate, and a platform it can detect, as the cases have.
    let (launch, env) = match shell {
        ShellName::Sh(shell) => (ShellLaunch::Sh(shell, ShMode::File), Vec::new()),
        ShellName::PowerShell(shell) => {
            fixture.expect_powershell_script()?;
            (
                ShellLaunch::PowerShell(shell, PowerShellMode::ScriptBlock),
                PowerShellRun::DEFAULT.platform.env(),
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
                env,
                ..Invocation::new(launch)
            },
        )
        .and_then(|outcome| {
            let mut problems = Vec::new();
            if outcome.status.success() {
                problems.push("the script succeeded after a redirect to plain http".to_owned());
            }
            if !outcome.stderr.contains(&expected_error) {
                problems.push(format!("stderr does not contain {expected_error:?}"));
            }
            check_install_dir(
                &fixture.install_dir(InstallDir::Plain),
                None,
                None,
                &mut problems,
            )?;
            Ok(CheckedRun { problems, outcome })
        });
    fixture.finish(result)
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
        ("--min-glibc", "2.27; id"),
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
