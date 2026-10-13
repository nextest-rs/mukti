// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#[cfg(unix)]
use crate::{
    archive::ArchiveFormat,
    fixture::{ArchiveContent, ArchiveServing, Outcome, ServedArchive, OLD_BINARY},
    runner::ExpectedBinary,
};
use crate::{
    cases::RELEASE,
    fixture::{
        generate_scripts, DefaultBaseUrl, Fixture, InstallDirSource, Invocation, Launch,
        ManifestTweak, Mode, PostInstallStep, PowerShellMode, MANIFEST_FILE_NAME, PROJECT,
    },
    runner::{check_install_dir, report_problems},
    shells::{ShellKind, ShellName, ShellSpec},
    targets::Target,
};
use camino::Utf8Path;
use color_eyre::eyre::{bail, eyre, Context, Result};
use std::fs;
#[cfg(unix)]
use std::process::{Command, Stdio};

pub(crate) fn truncated_script(shell: &ShellSpec, shell_path: &Utf8Path) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE, shell.kind)?;
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

    // TODO-RAINCLAUDE: sample the body to keep this fast, but try every cut in
    // the bytes before the closing brace, where the call to main is.
    let tail_start = longest.saturating_sub(32);
    let cuts = (0..tail_start).step_by(499).chain(tail_start..=longest);
    let install_dir = fixture.install_dir(InstallDirSource::Explicit);
    let mut problems = Vec::new();
    for cut in cuts {
        let outcome = fixture.run(
            shell,
            shell_path,
            &Invocation {
                stdin: Some(script[..cut].to_vec()),
                ..Invocation::latest(Some(fixture.base_url()), Launch::Sh(Mode::Piped))
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

// TODO-RAINCLAUDE: each row is rejected while parsing arguments: before the
// install directory is created, and before any request is made.
pub(crate) fn rejected_arguments(shell: &ShellSpec, shell_path: &Utf8Path) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE, shell.kind)?;
    let install_dir = fixture.install_dir(InstallDirSource::Explicit);
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
            shell,
            shell_path,
            &Invocation {
                extra_args,
                ..Invocation::latest(Some(fixture.base_url()), Launch::Sh(Mode::Args))
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

// TODO-RAINCLAUDE: PowerShell 7 prints an error's message with a bar before
// each line, and breaks a line that is wider than the console wherever that
// falls. PowerShell 7.4 also colours each line, even with NO_COLOR set. For
// example, on a GitHub runner (colour codes shown as <...>):
//
//     <red>| <red>... argument 'stray'. the options are -Version, -InstallDir,<reset>
//     <red>| <red>-Target, and -BaseUrl<reset>
//
// This returns the text with the colour codes, bars and line breaks taken
// out, so that a phrase can be looked for whatever the console's width was.
fn unwrapped_error(text: &str) -> String {
    let mut plain = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            plain.push(c);
        } else if chars.next() == Some('[') {
            // TODO-RAINCLAUDE: a colour code runs from ESC [ to its first
            // letter.
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    plain
        .split_whitespace()
        .filter(|word| *word != "|")
        .collect::<Vec<_>>()
        .join(" ")
}

// TODO-RAINCLAUDE: install.ps1 must refuse an argument that is not one of its
// options, before the install directory is created and before any request is
// made, and must exit with a failure. Both ways of running the script file
// are checked.
pub(crate) fn powershell_rejected_arguments(
    shell: &ShellSpec,
    shell_path: &Utf8Path,
) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE, shell.kind)?;
    let install_dir = fixture.install_dir(InstallDirSource::Explicit);
    // TODO-RAINCLAUDE: the mode, the arguments, and a word from PowerShell's
    // own message. The sh spelling of an option is only refused through the
    // call operator: `powershell -File` reads --version as -Version.
    let rows: [(PowerShellMode, &[&str], &str); 5] = [
        (PowerShellMode::File, &["-Bogus", "x"], "'Bogus'"),
        (PowerShellMode::Args, &["-Bogus", "x"], "'Bogus'"),
        (PowerShellMode::File, &["stray"], "'stray'"),
        (PowerShellMode::Args, &["stray"], "'stray'"),
        (PowerShellMode::Args, &["--version", "1.2.3"], "'--version'"),
    ];

    let mut problems = Vec::new();
    for (mode, extra_args, named) in rows {
        let outcome = fixture.run(
            shell,
            shell_path,
            &Invocation {
                extra_args,
                ..Invocation::latest(Some(fixture.base_url()), Launch::PowerShell(mode))
            },
        )?;
        if outcome.status.success() {
            problems.push(format!(
                "{mode:?} {extra_args:?}: the script succeeded:\n{outcome}"
            ));
        }
        let stderr = unwrapped_error(&outcome.stderr);
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

pub(crate) fn help(shell: &ShellSpec, shell_path: &Utf8Path) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE, shell.kind)?;
    let install_dir = fixture.install_dir(InstallDirSource::Explicit);

    let mut problems = Vec::new();
    for flag in ["--help", "-h"] {
        let outcome = fixture.run(
            shell,
            shell_path,
            &Invocation {
                extra_args: &[flag],
                ..Invocation::latest(Some(fixture.base_url()), Launch::Sh(Mode::Args))
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

// TODO-RAINCLAUDE: a script signalled in the middle of a download exits with
// the signal's status and leaves nothing behind in the install directory. dash
// does not run its EXIT trap when a signal kills it, so this depends on the
// script's signal traps.
//
// Only TERM is sent. A shell cannot trap a signal that it inherited as
// ignored, and HUP (under nohup) and INT (in a background job) often are.
#[cfg(unix)]
pub(crate) fn signal_cleans_up(shell: &ShellSpec, shell_path: &Utf8Path) -> Result<()> {
    // TODO-RAINCLAUDE: pass a target so that this doesn't depend on what the
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
        shell.kind.post_install_step(),
        DefaultBaseUrl::Elsewhere,
    )?;
    let result = signal_mid_download(&fixture, shell, shell_path, target);
    let (problems, outcome) = fixture.verify_requests_after(result)?;
    report_problems(problems).map_err(|error| eyre!("{error}\n{outcome}"))
}

#[cfg(unix)]
fn signal_mid_download(
    fixture: &Fixture,
    shell: &ShellSpec,
    shell_path: &Utf8Path,
    target: Target,
) -> Result<(Vec<String>, Outcome)> {
    fixture.expect_manifest("latest")?;

    let install_dir = fixture.install_dir(InstallDirSource::Explicit);
    let old_binary = install_dir.join(target.bin_file_name());
    fs::create_dir_all(&install_dir).wrap_err("failed to create the install directory")?;
    fs::write(&old_binary, OLD_BINARY).wrap_err("failed to write the old binary")?;

    let mut child = fixture.spawn(
        shell,
        shell_path,
        &Invocation {
            target: Some(target),
            ..Invocation::latest(Some(fixture.base_url()), Launch::Sh(Mode::Args))
        },
    )?;

    let signalled = fixture.wait_for_stalled_request().and_then(|connection| {
        // TODO-RAINCLAUDE: kill is a builtin in every shell, while some
        // minimal systems have no kill program.
        let status = Command::new(shell_path)
            .args(shell.args)
            .args(["-c", "kill -TERM \"$1\"", "kill", &child.id().to_string()])
            .stdin(Stdio::null())
            .status()
            .wrap_err_with(|| format!("failed to run {shell_path} to send the signal"))?;
        if !status.success() {
            bail!("kill -TERM failed with {status}");
        }
        Ok(connection)
    });
    let connection = match signalled {
        Ok(connection) => connection,
        Err(error) => {
            // TODO-RAINCLAUDE: the script most likely exited before making
            // the request; its output says why. The kill is best-effort.
            let _ = child.kill();
            let output = child.wait_with_output().wrap_err_with(|| {
                format!("failed to wait for {shell_path} after this error: {error:?}")
            })?;
            bail!("{error:?}\n{}", Outcome::from_output(output));
        }
    };
    // TODO-RAINCLAUDE: a shell runs a trap once the command it is waiting on
    // returns, so let curl go.
    drop(connection);

    let output = child
        .wait_with_output()
        .wrap_err_with(|| format!("failed to wait for {shell_path}"))?;
    let outcome = Outcome::from_output(output);

    let mut problems = Vec::new();
    // 143 is 128 plus SIGTERM's number.
    if outcome.status.code() != Some(143) {
        problems.push(format!(
            "the script exited with {}, expected exit code 143",
            outcome.status
        ));
    }
    check_install_dir(
        &install_dir,
        Some(ExpectedBinary::Old),
        Some(target),
        &mut problems,
    )?;
    Ok((problems, outcome))
}

// TODO-RAINCLAUDE: a base URL that is https, where the server answers the
// manifest request with a redirect to plain http. The script must fail and
// install nothing. curl and PowerShell 7 refuse to follow, so the plain-http
// request is never made; an unexpected request fails the test.
pub(crate) fn https_redirect_to_http(shell: &ShellSpec, shell_path: &Utf8Path) -> Result<()> {
    let mut fixture = Fixture::new(
        &RELEASE,
        ManifestTweak::None,
        None,
        shell.kind.post_install_step(),
        DefaultBaseUrl::Elsewhere,
    )?;
    let base_url = fixture.expect_manifest_redirect_to_http();
    let manifest_url = format!("{base_url}/latest/{MANIFEST_FILE_NAME}");
    let expected_error = match shell.name {
        ShellName::Sh
        | ShellName::Dash
        | ShellName::Bash
        | ShellName::Busybox
        | ShellName::Ksh93 => {
            format!("failed to download the install manifest from {manifest_url}")
        }
        ShellName::Pwsh => format!("failed to download {manifest_url}"),
        #[cfg(windows)]
        ShellName::WindowsPowerShell => {
            // TODO-RAINCLAUDE: Windows PowerShell follows the redirect before
            // install.ps1 gets to see where the response came from, so here
            // the plain-http request is made. The script must then refuse
            // what came back.
            fixture.expect_manifest("latest")?;
            "refusing a redirect from https to plain http".to_owned()
        }
    };
    // TODO-RAINCLAUDE: PowerShell needs a -Command launch to be told about
    // the test certificate, and a target off Windows.
    let (launch, target) = match shell.kind {
        ShellKind::Sh => (Launch::Sh(Mode::Args), None),
        ShellKind::PowerShell => {
            fixture.expect_powershell_script()?;
            (
                Launch::PowerShell(PowerShellMode::ScriptBlock),
                Some(Target::X86_64_PC_WINDOWS_MSVC),
            )
        }
    };

    let result = fixture
        .run(
            shell,
            shell_path,
            &Invocation {
                target,
                ..Invocation::latest(Some(base_url), launch)
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
                &fixture.install_dir(InstallDirSource::Explicit),
                None,
                None,
                &mut problems,
            )?;
            Ok((problems, outcome))
        });
    let (problems, outcome) = fixture.verify_requests_after(result)?;
    report_problems(problems).map_err(|error| eyre!("{error}\n{outcome}"))
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
        let mut flags = PostInstallStep::Configured.script_flags();
        let position = flags
            .iter()
            .position(|existing| *existing == flag)
            .ok_or_else(|| eyre!("{flag} is not one of the script flags"))?;
        flags[position + 1] = value;
        expect_rejected(&format!("flag-{index}"), PROJECT, &flags, flag)?;
    }

    expect_rejected(
        "project",
        "x'; id; '",
        &PostInstallStep::Configured.script_flags(),
        "cannot be used in an install script",
    )
}
