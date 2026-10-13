// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#[cfg(windows)]
use crate::shells::ShellSpec;
use crate::{shells::ShellKind, targets::Target};
#[cfg(windows)]
use camino::Utf8Path;
#[cfg(windows)]
use color_eyre::eyre::{bail, Context};
use color_eyre::eyre::{eyre, Result};
#[cfg(windows)]
use std::process::{Command, Stdio};

/// Set by the CI step that puts an x64 PowerShell on the PATH of an ARM64
/// Windows machine.
#[cfg(windows)]
pub(crate) const EMULATED_POWERSHELL_VAR: &str = "MUKTI_TEST_EMULATED_POWERSHELL";

#[cfg(windows)]
pub(crate) fn host_target(kind: ShellKind) -> Result<Option<Target>> {
    match kind {
        ShellKind::Sh | ShellKind::PowerShell => windows_host_target().map(Some),
    }
}

#[cfg(not(windows))]
pub(crate) fn host_target(kind: ShellKind) -> Result<Option<Target>> {
    use std::env::consts::{ARCH, OS};

    match kind {
        ShellKind::Sh => {
            // The test binary's libc is used to determine the system's. For
            // example, if `target_env` is musl, the test is taken to be running
            // on a musl system like Alpine.
            let target = match (OS, ARCH) {
                #[cfg(not(target_env = "musl"))]
                ("linux", "x86_64") => Target::X86_64_UNKNOWN_LINUX_GNU,
                #[cfg(target_env = "musl")]
                ("linux", "x86_64") => Target::X86_64_UNKNOWN_LINUX_MUSL,
                #[cfg(not(target_env = "musl"))]
                ("linux", "aarch64") => Target::AARCH64_UNKNOWN_LINUX_GNU,
                #[cfg(target_env = "musl")]
                ("linux", "aarch64") => Target::AARCH64_UNKNOWN_LINUX_MUSL,
                #[cfg(not(target_env = "musl"))]
                ("linux", "riscv64") => Target::RISCV64GC_UNKNOWN_LINUX_GNU,
                ("macos", _) => Target::UNIVERSAL_APPLE_DARWIN,
                ("freebsd", "x86_64") => Target::X86_64_UNKNOWN_FREEBSD,
                ("illumos", "x86_64") => Target::X86_64_UNKNOWN_ILLUMOS,
                (os, arch) => {
                    return Err(eyre!(
                        "no expected install target is known for a {os} {arch} machine (add one to host_target)"
                    ));
                }
            };
            Ok(Some(target))
        }
        ShellKind::PowerShell => Ok(None),
    }
}

#[cfg(windows)]
const MACHINE_ARCHITECTURE_COMMAND: &str =
    "[Environment]::GetEnvironmentVariable('PROCESSOR_ARCHITECTURE', 'Machine')";

#[cfg(windows)]
fn windows_host_target() -> Result<Target> {
    let output = Command::new("reg")
        .args([
            "query",
            r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            "/v",
            "PROCESSOR_ARCHITECTURE",
        ])
        .stdin(Stdio::null())
        .output()
        .wrap_err("failed to run reg query")?;
    if !output.status.success() {
        bail!(
            "reg query for the machine's PROCESSOR_ARCHITECTURE failed with {}:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);

    // reg query's output is of the form:
    //
    // HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Control\Session Manager\Environment
    //      PROCESSOR_ARCHITECTURE    REG_SZ    AMD64
    //
    // We care about the very last field here: AMD64.
    match stdout.split_whitespace().next_back().unwrap_or_default() {
        "AMD64" => Ok(Target::X86_64_PC_WINDOWS_MSVC),
        "ARM64" => Ok(Target::AARCH64_PC_WINDOWS_MSVC),
        "x86" => Ok(Target::I686_PC_WINDOWS_MSVC),
        other => Err(eyre!(
            "reg query reported an unknown processor architecture: {other:?} (full output: {stdout:?})"
        )),
    }
}

/// Ensures that the current environment has an x64 PowerShell on an ARM64 machine.
///
/// (We do this check to ensure that the corresponding CI configuration doesn't
/// start passing vacuously.)
#[cfg(windows)]
pub(crate) fn powershell_is_emulated(shell: &ShellSpec, shell_path: &Utf8Path) -> Result<()> {
    let output = Command::new(shell_path)
        .args(shell.args)
        .args([
            "-NoProfile",
            "-Command",
            &format!("\"$env:PROCESSOR_ARCHITECTURE on $({MACHINE_ARCHITECTURE_COMMAND})\""),
        ])
        .stdin(Stdio::null())
        .output()
        .wrap_err_with(|| format!("failed to run {shell_path}"))?;
    if !output.status.success() {
        bail!(
            "{shell_path} failed with {} when asked for its architecture:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
        );
    }
    let architectures = String::from_utf8_lossy(&output.stdout);
    if architectures.trim() != "AMD64 on ARM64" {
        bail!(
            "{EMULATED_POWERSHELL_VAR} is set, so {shell_path} must be an x64 PowerShell on an ARM64 machine, but it reported {:?}",
            architectures.trim(),
        );
    }
    let target = windows_host_target()?;
    if target != Target::AARCH64_PC_WINDOWS_MSVC {
        bail!("expected the tests to look for the ARM64 build under {shell_path}, not {target}");
    }
    Ok(())
}
