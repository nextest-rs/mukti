// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tests for determining colored output.

use crate::{
    archive::ArchiveFormat,
    cases::RELEASE,
    fixture::{
        ArchiveContent, ArchiveServing, CheckedRun, DefaultBaseUrl, Fixture, InstallDir,
        Invocation, ManifestTweak, Outcome, PostInstallStep, ScriptOptions, ServedArchive,
    },
    runner::{
        check_no_install, normalize_output, ExpectedNoInstall, ExpectedNoInstallDir,
        ExpectedStatus, ExpectedStream,
    },
    shells::{PowerShellName, ShellKind, ShellLaunch},
};
#[cfg(unix)]
use crate::{
    fixture::{ensure_success, run_command, PASSED_THROUGH_VARS},
    shells::ShName,
};
use camino::Utf8Path;
use color_eyre::eyre::Result;
#[cfg(unix)]
use color_eyre::eyre::{bail, eyre, Context};
#[cfg(unix)]
use std::{env, process::Command};

#[derive(Clone, Copy, Debug)]
pub(crate) struct ColorCase {
    pub(crate) name: &'static str,
    input: ColorInput,
    expected: CaseExpected,
}

#[derive(Clone, Copy, Debug)]
struct ColorInput {
    // No other color-related variable is set.
    env: &'static [(&'static str, &'static str)],
}

#[derive(Clone, Copy, Debug)]
enum CaseExpected {
    Always(ColorExpected),
    /// install.sh with TERM unset in the environment (bash sets TERM=dumb
    /// itself).
    ///
    /// These tests are Unix only, because on Windows the MSYS runtime sets
    /// TERM=xterm-256color for unix shells.
    #[cfg(unix)]
    ByShellTerm {
        term_unset: ColorExpected,
        term_dumb: ColorExpected,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ColorExpected {
    Colored,
    Plain,
    Rejected,
}

const fn case(
    name: &'static str,
    env: &'static [(&'static str, &'static str)],
    expected: ColorExpected,
) -> ColorCase {
    ColorCase {
        name,
        input: ColorInput { env },
        expected: CaseExpected::Always(expected),
    }
}

#[cfg(unix)]
const fn case_term_unset(
    name: &'static str,
    env: &'static [(&'static str, &'static str)],
    term_unset: ColorExpected,
    term_dumb: ColorExpected,
) -> ColorCase {
    ColorCase {
        name,
        input: ColorInput { env },
        expected: CaseExpected::ByShellTerm {
            term_unset,
            term_dumb,
        },
    }
}

const TTY: (&str, &str) = ("IGNORE_IS_TERMINAL", "1");
const XTERM: (&str, &str) = ("TERM", "xterm");

/// Covers cases for for install.sh and pwsh, though not Windows PowerShell 5.1.
const COMMON_CASES: &[ColorCase] = {
    use ColorExpected::{Colored, Plain, Rejected};
    &[
        case("not-a-terminal", &[XTERM], Plain),
        case("terminal", &[TTY, XTERM], Colored),
        case(
            "terminal-ignore-zero",
            &[("IGNORE_IS_TERMINAL", "0"), XTERM],
            Plain,
        ),
        case(
            "terminal-ignore-empty",
            &[("IGNORE_IS_TERMINAL", ""), XTERM],
            Colored,
        ),
        case("term-dumb", &[TTY, ("TERM", "dumb")], Plain),
        case("term-empty", &[TTY, ("TERM", "")], Colored),
        case("no-color", &[TTY, XTERM, ("NO_COLOR", "1")], Plain),
        case("no-color-empty", &[TTY, XTERM, ("NO_COLOR", "")], Plain),
        case("force-color", &[("FORCE_COLOR", "1")], Colored),
        case("force-color-empty", &[("FORCE_COLOR", "")], Colored),
        case("force-color-true", &[("FORCE_COLOR", "true")], Colored),
        case("force-color-word", &[("FORCE_COLOR", "yes")], Colored),
        case("force-color-plus", &[("FORCE_COLOR", "+")], Colored),
        case("force-color-zero", &[("FORCE_COLOR", "0")], Plain),
        case("force-color-zeros", &[("FORCE_COLOR", "+00")], Plain),
        case("force-color-false", &[("FORCE_COLOR", "false")], Plain),
        // FORCE_COLOR=0 doesn't force color off.
        case(
            "force-color-zero-terminal",
            &[("FORCE_COLOR", "0"), TTY, XTERM],
            Colored,
        ),
        // FORCE_COLOR, when set, overrides CLICOLOR_FORCE.
        case(
            "force-color-zero-hides-clicolor-force",
            &[("FORCE_COLOR", "0"), ("CLICOLOR_FORCE", "1")],
            Plain,
        ),
        case("clicolor-force", &[("CLICOLOR_FORCE", "1")], Colored),
        case("clicolor-force-empty", &[("CLICOLOR_FORCE", "")], Colored),
        case("clicolor-force-zero", &[("CLICOLOR_FORCE", "0")], Plain),
        case("cargo-always", &[("CARGO_TERM_COLOR", "always")], Colored),
        case(
            "cargo-never-over-force-color",
            &[
                ("CARGO_TERM_COLOR", "never"),
                ("FORCE_COLOR", "1"),
                TTY,
                XTERM,
            ],
            Plain,
        ),
        case(
            "cargo-auto",
            &[("CARGO_TERM_COLOR", "auto"), TTY, XTERM],
            Colored,
        ),
        case(
            "cargo-auto-not-a-terminal",
            &[("CARGO_TERM_COLOR", "auto"), XTERM],
            Plain,
        ),
        case(
            "cargo-invalid",
            &[("CARGO_TERM_COLOR", "sometimes")],
            Rejected,
        ),
        case("cargo-empty", &[("CARGO_TERM_COLOR", "")], Rejected),
        case(
            "term-dumb-colorterm",
            &[TTY, ("TERM", "dumb"), ("COLORTERM", "truecolor")],
            Plain,
        ),
        case(
            "cargo-wrong-case",
            &[("CARGO_TERM_COLOR", "Always")],
            Rejected,
        ),
    ]
};

/// pwsh 7 honors NO_COLOR (any value) and TERM=dumb itself by stripping escapes
/// from host output, so these cases differ.
const SH_NO_COLOR_CASES: &[ColorCase] = {
    use ColorExpected::Colored;
    &[
        case("no-color-zero", &[TTY, XTERM, ("NO_COLOR", "0")], Colored),
        case(
            "force-color-over-no-color",
            &[("FORCE_COLOR", "1"), ("NO_COLOR", "1")],
            Colored,
        ),
        case(
            "force-color-over-term-dumb",
            &[("FORCE_COLOR", "1"), ("TERM", "dumb")],
            Colored,
        ),
        case(
            "cargo-always-over-no-color",
            &[
                ("CARGO_TERM_COLOR", "always"),
                ("NO_COLOR", "1"),
                ("TERM", "dumb"),
            ],
            Colored,
        ),
    ]
};

const POWERSHELL_NO_COLOR_CASES: &[ColorCase] = {
    use ColorExpected::Plain;
    &[
        case("no-color-zero", &[TTY, XTERM, ("NO_COLOR", "0")], Plain),
        case(
            "force-color-over-no-color",
            &[("FORCE_COLOR", "1"), ("NO_COLOR", "1")],
            Plain,
        ),
        case(
            "force-color-over-term-dumb",
            &[("FORCE_COLOR", "1"), ("TERM", "dumb")],
            Plain,
        ),
        case(
            "cargo-always-over-no-color",
            &[
                ("CARGO_TERM_COLOR", "always"),
                ("NO_COLOR", "1"),
                ("TERM", "dumb"),
            ],
            Plain,
        ),
    ]
};

/// When TERM is unset, the result differs by platform in supports-color:
/// Windows assumes color, but Unix doesn't.
#[cfg(unix)]
const SH_TERM_UNSET_CASES: &[ColorCase] = {
    use ColorExpected::{Colored, Plain};
    &[
        case_term_unset("term-unset", &[TTY], Plain, Plain),
        case_term_unset(
            "term-unset-colorterm",
            &[TTY, ("COLORTERM", "")],
            Colored,
            Plain,
        ),
        case_term_unset(
            "term-unset-clicolor",
            &[TTY, ("CLICOLOR", "1")],
            Colored,
            Plain,
        ),
        case_term_unset(
            "term-unset-clicolor-zero",
            &[TTY, ("CLICOLOR", "0")],
            Plain,
            Plain,
        ),
        case_term_unset(
            "term-unset-apple-terminal",
            &[TTY, ("TERM_PROGRAM", "Apple_Terminal")],
            Colored,
            Plain,
        ),
        case_term_unset(
            "term-unset-iterm",
            &[TTY, ("TERM_PROGRAM", "iTerm.app")],
            Colored,
            Plain,
        ),
    ]
};

const POWERSHELL_TERM_UNSET_CASES: &[ColorCase] = {
    use ColorExpected::{Colored, Plain};
    &[
        case("term-unset", &[TTY], Colored),
        case("term-cygwin", &[TTY, ("TERM", "cygwin")], Plain),
        case(
            "term-cygwin-colorterm",
            &[TTY, ("TERM", "cygwin"), ("COLORTERM", "1")],
            Colored,
        ),
        case(
            "term-cygwin-clicolor",
            &[TTY, ("TERM", "cygwin"), ("CLICOLOR", "1")],
            Colored,
        ),
    ]
};

/// Windows PowerShell 5.1 doesn't produce colored output, but still rejects an
/// invalid CARGO_TERM_COLOR.
#[cfg(windows)]
const WINDOWS_POWERSHELL_CASES: &[ColorCase] = {
    use ColorExpected::{Plain, Rejected};
    &[
        case("terminal", &[TTY, XTERM], Plain),
        case("force-color", &[("FORCE_COLOR", "1")], Plain),
        case("cargo-always", &[("CARGO_TERM_COLOR", "always")], Plain),
        case(
            "cargo-invalid",
            &[("CARGO_TERM_COLOR", "sometimes")],
            Rejected,
        ),
        // .NET Framework returns $null for an empty variable, so 5.1 treats an
        // empty CARGO_TERM_COLOR as unset.
        case("cargo-empty", &[("CARGO_TERM_COLOR", "")], Plain),
    ]
};

fn color_env(input: &ColorInput) -> Vec<(&'static str, Option<String>)> {
    // By default, we set NO_COLOR=1. For these tests we want to unset it again.
    std::iter::once(("NO_COLOR", None))
        .chain(
            input
                .env
                .iter()
                .map(|(name, value)| (*name, Some((*value).to_owned()))),
        )
        .collect()
}

const COLORED_INFO: &str =
    "\u{1b}[1minfo:\u{1b}[0m installing \u{1b}[1mwidget-project 0.9.148\u{1b}[0m (";
const COLORED_INSTALL_DIR: &str = ") to \u{1b}[1m";
const COLORED_ERROR: &str = "\u{1b}[1;31merror:\u{1b}[0m the archive for";
const PLAIN_INFO: &str = "info: installing widget-project 0.9.148 (";
const PLAIN_ERROR: &str = "error: the archive for";

pub(crate) fn sh_color_cases() -> Vec<&'static ColorCase> {
    let cases = COMMON_CASES.iter().chain(SH_NO_COLOR_CASES);
    #[cfg(unix)]
    let cases = cases.chain(SH_TERM_UNSET_CASES);
    cases.collect()
}

pub(crate) fn powershell_color_cases(shell: PowerShellName) -> Vec<&'static ColorCase> {
    match shell {
        PowerShellName::Pwsh => COMMON_CASES
            .iter()
            .chain(POWERSHELL_NO_COLOR_CASES)
            .chain(POWERSHELL_TERM_UNSET_CASES)
            .collect(),
        #[cfg(windows)]
        PowerShellName::WindowsPowerShell => WINDOWS_POWERSHELL_CASES.iter().collect(),
    }
}

pub(crate) fn run_color_case(
    shell_path: &Utf8Path,
    launch: ShellLaunch,
    case: &ColorCase,
) -> Result<()> {
    let expected = match (case.expected, launch) {
        (CaseExpected::Always(expected), ShellLaunch::Sh(..) | ShellLaunch::PowerShell(..)) => {
            expected
        }
        #[cfg(unix)]
        (
            CaseExpected::ByShellTerm {
                term_unset,
                term_dumb,
            },
            ShellLaunch::Sh(shell, _),
        ) => match shell_term(shell, shell_path)?.as_deref() {
            None => term_unset,
            Some("dumb") => term_dumb,
            Some(other) => {
                bail!("{shell} sets TERM={other:?} when it is unset, so this case can't run there");
            }
        },
        #[cfg(unix)]
        (CaseExpected::ByShellTerm { .. }, ShellLaunch::PowerShell(..)) => {
            bail!("case {} is only meaningful for install.sh", case.name);
        }
    };
    let format = match launch.kind() {
        ShellKind::Sh => ArchiveFormat::TarGz,
        ShellKind::PowerShell => ArchiveFormat::Zip,
    };
    let served = match expected {
        ColorExpected::Colored | ColorExpected::Plain => Some(ServedArchive {
            target: launch.host_target()?,
            format,
            serving: ArchiveServing::NotFound,
            content: ArchiveContent::NotAnArchive,
        }),
        ColorExpected::Rejected => None,
    };
    let mut fixture = Fixture::new(
        &RELEASE,
        ManifestTweak::None,
        served,
        PostInstallStep::Configured,
        DefaultBaseUrl::for_options(ScriptOptions::ARGS),
    )?;
    let result = (|| -> Result<CheckedRun> {
        match expected {
            ColorExpected::Colored | ColorExpected::Plain => {
                fixture.expect_manifest("latest")?;
                fixture.expect_archive()?;
            }
            ColorExpected::Rejected => {}
        }
        let outcome = fixture.run(
            shell_path,
            &Invocation {
                env: color_env(&case.input),
                ..Invocation::new(launch)
            },
        )?;
        let problems = check_outcome(&fixture, launch.kind(), expected, &outcome)?;
        Ok(CheckedRun { problems, outcome })
    })();
    fixture.finish(result)
}

fn check_outcome(
    fixture: &Fixture,
    kind: ShellKind,
    expected: ColorExpected,
    outcome: &Outcome,
) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    // install.sh writes everything to stderr. install.ps1 writes info lines to
    // stdout, and its errors are host-rendered, so only stdout is checked.
    let (stream, stream_name, colored, plain): (_, _, &[&str], &[&str]) = match kind {
        ShellKind::Sh => (
            &outcome.stderr,
            "stderr",
            &[COLORED_INFO, COLORED_INSTALL_DIR, COLORED_ERROR],
            &[PLAIN_INFO, PLAIN_ERROR],
        ),
        ShellKind::PowerShell => (
            &outcome.stdout,
            "stdout",
            &[COLORED_INFO, COLORED_INSTALL_DIR],
            &[PLAIN_INFO],
        ),
    };
    match (expected, kind) {
        (ColorExpected::Colored | ColorExpected::Plain, ShellKind::PowerShell) => {
            if !normalize_output(&outcome.stderr).contains("the archive for") {
                problems.push("stderr does not contain \"the archive for\"".to_owned());
            }
        }
        (ColorExpected::Colored | ColorExpected::Plain, ShellKind::Sh)
        | (ColorExpected::Rejected, ShellKind::Sh | ShellKind::PowerShell) => {}
    }
    if outcome.status.success() {
        match expected {
            ColorExpected::Colored | ColorExpected::Plain => {
                problems.push("the script succeeded".to_owned());
            }
            ColorExpected::Rejected => {}
        }
    }
    let wanted = match expected {
        ColorExpected::Colored => colored,
        ColorExpected::Plain => plain,
        ColorExpected::Rejected => {
            let expected = ExpectedNoInstall {
                status: ExpectedStatus::Failure,
                stdout: ExpectedStream::Empty,
                stderr: ExpectedStream::Contains(&[
                    "invalid CARGO_TERM_COLOR",
                    "expected \"auto\", \"always\", or \"never\"",
                ]),
                install_dir: ExpectedNoInstallDir::NotCreated,
            };
            problems.extend(check_no_install(
                outcome,
                &fixture.install_dir(InstallDir::Plain),
                &expected,
            )?);
            &[]
        }
    };
    for text in wanted {
        if !stream.contains(text) {
            problems.push(format!("{stream_name} does not contain {text:?}"));
        }
    }
    match (expected, kind) {
        (ColorExpected::Colored, ShellKind::Sh | ShellKind::PowerShell)
        | (ColorExpected::Rejected, ShellKind::PowerShell) => {}
        (ColorExpected::Plain, ShellKind::Sh | ShellKind::PowerShell)
        | (ColorExpected::Rejected, ShellKind::Sh) => {
            if stream.contains('\u{1b}') {
                problems.push(format!("{stream_name} contains an escape character"));
            }
        }
    }
    Ok(problems)
}

#[cfg(unix)]
fn shell_term(shell: ShName, shell_path: &Utf8Path) -> Result<Option<String>> {
    let mut command = Command::new(shell_path);
    command.args(shell.args()).args([
        "-c",
        "if [ -n \"${TERM+set}\" ]; then printf 'set:%s' \"$TERM\"; else printf unset; fi",
    ]);
    command.env_clear();
    for name in PASSED_THROUGH_VARS {
        if let Some(value) = env::var_os(name) {
            command.env(name, value);
        }
    }
    let what = format!("{shell_path} to check how it handles an unset TERM");
    let output = run_command(&mut command, &what)?;
    ensure_success(&what, &output)?;
    let stdout = String::from_utf8(output.stdout)
        .wrap_err_with(|| format!("{what}: output is not UTF-8"))?;
    match stdout.strip_prefix("set:") {
        Some(value) => Ok(Some(value.to_owned())),
        None if stdout == "unset" => Ok(None),
        None => Err(eyre!("{what}: unexpected output {stdout:?}")),
    }
}
