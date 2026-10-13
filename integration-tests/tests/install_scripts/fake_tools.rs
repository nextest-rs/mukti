// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test cases for `install.sh` against a constructed PATH.

use crate::{
    cases::{
        windows_architecture_cases, Case, Expect, FakeWindowsArch, ShPlatform, ShRun, BASE,
        FAILS_ON_ARCHIVE, RELEASE,
    },
    fixture::{Fixture, InstallDirSource, Invocation, Launch, Mode, Release, MANIFEST_FILE_NAME},
    runner::report_problems,
    shells::{find_program, ShellSpec},
    targets::Target,
};
use camino::Utf8Path;
use color_eyre::eyre::{eyre, Context, Result};
use std::fs;

/// The SHA-256 tools `install.sh` is aware of, in the order they are tried.
#[derive(Clone, Copy, Debug)]
pub(crate) enum HashTool {
    Sha256sum,
    Shasum,
    Sha256,
    Digest,
    Openssl,
}

impl HashTool {
    const ALL: &'static [Self] = &[
        Self::Sha256sum,
        Self::Shasum,
        Self::Sha256,
        Self::Digest,
        Self::Openssl,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Sha256sum => "sha256sum",
            Self::Shasum => "shasum",
            Self::Sha256 => "sha256",
            Self::Digest => "digest",
            Self::Openssl => "openssl",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FakeTools {
    pub(crate) host: FakeHost,
    /// The target the script should detect for the host, or `None` if it should
    /// fail before choosing one.
    pub(crate) expected_target: Option<Target>,
    /// The hash tool to provide, or `None` for no hash tool at all.
    pub(crate) hash_tool: Option<HashTool>,
}

impl FakeTools {
    const fn new(host: FakeHost, expected_target: Option<Target>) -> Self {
        Self {
            host,
            expected_target,
            hash_tool: Some(HashTool::Sha256sum),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum FakeHost {
    Unix(FakeUnix),
    /// A MINGW or MSYS shell on Windows.
    ///
    /// In this situation, the script obtains the architecture from Windows and
    /// not from uname.
    Windows {
        uname_s: &'static str,
        arch: FakeWindowsArch,
    },
}

impl FakeHost {
    const LINUX_X86_64: Self = Self::linux("x86_64", Some("2.39"));

    const fn linux(uname_m: &'static str, glibc: Option<&'static str>) -> Self {
        Self::Unix(FakeUnix::linux(uname_m, glibc))
    }

    const fn mingw(arch: FakeWindowsArch) -> Self {
        Self::Windows {
            uname_s: "MINGW64_NT-10.0-26100",
            arch,
        }
    }

    const fn other(
        uname_s: &'static str,
        uname_m: &'static str,
        uname_o: Option<&'static str>,
    ) -> Self {
        Self::Unix(FakeUnix::other(uname_s, uname_m, uname_o))
    }

    pub(crate) fn env(&self) -> Vec<(&'static str, Option<String>)> {
        match self {
            Self::Unix(unix) => unix.env().to_vec(),
            Self::Windows { uname_s, arch } => {
                let mut env = FakeUnix::other(uname_s, "x86_64", Some("Msys"))
                    .env()
                    .to_vec();
                env.extend(arch.env());
                env
            }
        }
    }
}

/// Fake information returned by our test helper when standing in for the
/// `uname`, `getconf`, and `isainfo` tools.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FakeUnix {
    uname_s: &'static str,
    uname_m: &'static str,
    uname_o: Option<&'static str>,
    // None makes getconf fail, simulating a musl system.
    glibc: Option<&'static str>,
    long_bit: Option<&'static str>,
    isainfo: Option<&'static str>,
}

impl FakeUnix {
    const fn linux(uname_m: &'static str, glibc: Option<&'static str>) -> Self {
        Self {
            uname_s: "Linux",
            uname_m,
            uname_o: Some("GNU/Linux"),
            glibc,
            long_bit: Some("64"),
            isainfo: None,
        }
    }

    const fn other(
        uname_s: &'static str,
        uname_m: &'static str,
        uname_o: Option<&'static str>,
    ) -> Self {
        Self {
            uname_s,
            uname_m,
            uname_o,
            glibc: None,
            long_bit: None,
            isainfo: None,
        }
    }

    fn env(&self) -> [(&'static str, Option<String>); 6] {
        [
            ("FAKE_UNAME_S", Some(self.uname_s.to_owned())),
            ("FAKE_UNAME_M", Some(self.uname_m.to_owned())),
            ("FAKE_UNAME_O", self.uname_o.map(str::to_owned)),
            (
                "FAKE_GLIBC",
                self.glibc.map(|version| format!("glibc {version}")),
            ),
            ("FAKE_LONG_BIT", self.long_bit.map(str::to_owned)),
            ("FAKE_ISAINFO", self.isainfo.map(str::to_owned)),
        ]
    }
}

impl Case {
    const fn with_fake_tools(tools: FakeTools) -> Self {
        Self {
            sh: Some(ShRun {
                platform: ShPlatform::Fake(tools),
                ..ShRun::DEFAULT
            }),
            powershell: None,
            ..BASE
        }
    }

    const fn detects(
        name: &'static str,
        host: FakeHost,
        target: Target,
        progress_contains: &'static [&'static str],
    ) -> Self {
        Self {
            group: Some("detect"),
            name,
            progress_contains,
            ..Self::with_fake_tools(FakeTools::new(host, Some(target)))
        }
    }

    const fn detect_fails(
        name: &'static str,
        host: FakeHost,
        expect: Expect,
        error_contains: &'static [&'static str],
    ) -> Self {
        Self {
            group: Some("detect"),
            name,
            expect,
            error_contains,
            ..Self::with_fake_tools(FakeTools::new(host, None))
        }
    }
}

// TODO-RAINCLAUDE: what the test helper prints to stderr when it stands in for
// curl.
const FAKE_CURL_ERROR: &str = "fake curl: (7) could not connect";

const UNSUPPORTED: &[&str] = &["no widget-project build is known for this platform"];

pub(crate) fn fake_tool_cases() -> Vec<Case> {
    let mut cases = vec![
        Case::detects(
            "linux-x86_64-glibc-at-floor",
            FakeHost::linux("x86_64", Some("2.27")),
            Target::X86_64_UNKNOWN_LINUX_GNU,
            &[],
        ),
        Case::detects(
            "linux-x86_64-glibc-next-major",
            FakeHost::linux("x86_64", Some("3.0")),
            Target::X86_64_UNKNOWN_LINUX_GNU,
            &[],
        ),
        Case::detects(
            "linux-x86_64-glibc-below-floor",
            FakeHost::linux("x86_64", Some("2.26")),
            Target::X86_64_UNKNOWN_LINUX_MUSL,
            &["glibc 2.26 is older than the 2.27 that glibc builds need"],
        ),
        Case::detects(
            "linux-x86_64-no-glibc",
            FakeHost::linux("x86_64", None),
            Target::X86_64_UNKNOWN_LINUX_MUSL,
            &["glibc was not detected, so only statically linked musl builds were considered"],
        ),
        Case {
            release: Release {
                targets: &[Target::X86_64_UNKNOWN_LINUX_MUSL],
                ..RELEASE
            },
            ..Case::detects(
                "linux-x86_64-glibc-without-gnu-build",
                FakeHost::LINUX_X86_64,
                Target::X86_64_UNKNOWN_LINUX_MUSL,
                &[],
            )
        },
        // A 64-bit kernel with 32-bit programs -- glibc looks new enough, but
        // the platform only supports musl builds.
        Case::detects(
            "linux-x86_64-32-bit-userland",
            FakeHost::Unix(FakeUnix {
                long_bit: Some("32"),
                ..FakeUnix::linux("x86_64", Some("2.39"))
            }),
            Target::X86_64_UNKNOWN_LINUX_MUSL,
            &["64-bit kernel (x86_64) with a 32-bit userland"],
        ),
        Case::detects(
            "linux-aarch64-glibc",
            FakeHost::linux("aarch64", Some("2.31")),
            Target::AARCH64_UNKNOWN_LINUX_GNU,
            &[],
        ),
        Case::detects(
            "linux-riscv64-glibc",
            FakeHost::linux("riscv64", Some("2.35")),
            Target::RISCV64GC_UNKNOWN_LINUX_GNU,
            &[],
        ),
        // The release doesn't have a riscv64 musl build.
        Case::detect_fails(
            "linux-riscv64-no-glibc",
            FakeHost::linux("riscv64", None),
            Expect::FailsAfterManifest,
            &[
                "has no tar.gz archive for this platform",
                "looked for: riscv64gc-unknown-linux-musl",
                "glibc was not detected, so only statically linked musl builds were considered",
                "riscv64gc-unknown-linux-gnu",
            ],
        ),
        Case::detect_fails(
            "linux-android",
            FakeHost::other("Linux", "aarch64", Some("Android")),
            Expect::FailsBeforeManifest,
            UNSUPPORTED,
        ),
        Case::detects(
            "darwin-arm64",
            FakeHost::other("Darwin", "arm64", None),
            Target::UNIVERSAL_APPLE_DARWIN,
            &[],
        ),
        Case {
            release: Release {
                targets: &[
                    Target::UNIVERSAL_APPLE_DARWIN,
                    Target::AARCH64_APPLE_DARWIN,
                    Target::X86_64_APPLE_DARWIN,
                ],
                ..RELEASE
            },
            ..Case::detects(
                "darwin-prefers-native-over-universal",
                FakeHost::other("Darwin", "arm64", None),
                Target::AARCH64_APPLE_DARWIN,
                &[],
            )
        },
        Case::detects(
            "freebsd-amd64",
            FakeHost::other("FreeBSD", "amd64", Some("FreeBSD")),
            Target::X86_64_UNKNOWN_FREEBSD,
            &[],
        ),
        Case::detects(
            "illumos-x86_64",
            FakeHost::Unix(FakeUnix {
                isainfo: Some("amd64"),
                ..FakeUnix::other("SunOS", "i86pc", Some("illumos"))
            }),
            Target::X86_64_UNKNOWN_ILLUMOS,
            &[],
        ),
        Case::detect_fails(
            "illumos-32-bit",
            FakeHost::Unix(FakeUnix {
                isainfo: Some("i386"),
                ..FakeUnix::other("SunOS", "i86pc", Some("illumos"))
            }),
            Expect::FailsBeforeManifest,
            UNSUPPORTED,
        ),
        Case::detect_fails(
            "solaris",
            FakeHost::other("SunOS", "i86pc", Some("Solaris")),
            Expect::FailsBeforeManifest,
            UNSUPPORTED,
        ),
        Case::detect_fails(
            "mingw-no-architecture",
            FakeHost::mingw(FakeWindowsArch {
                machine: "PDP11",
                processor_architecture: None,
                processor_architew6432: None,
            }),
            Expect::FailsBeforeManifest,
            &["PROCESSOR_ARCHITECTURE: unset"],
        ),
        Case::detects(
            "msys",
            FakeHost::Windows {
                uname_s: "MSYS_NT-10.0-26100",
                arch: FakeWindowsArch::new("AMD64", "AMD64", None),
            },
            Target::X86_64_PC_WINDOWS_MSVC,
            &[],
        ),
        Case::detect_fails(
            "unknown-os",
            FakeHost::other("Plan9", "x86_64", None),
            Expect::FailsBeforeManifest,
            &["OS: Plan9, architecture: x86_64"],
        ),
    ];

    cases.extend(windows_architecture_cases(|arch, target| {
        Case::with_fake_tools(FakeTools::new(FakeHost::mingw(arch), Some(target)))
    }));

    const LINUX_TOOLS: FakeTools = FakeTools::new(
        FakeHost::LINUX_X86_64,
        Some(Target::X86_64_UNKNOWN_LINUX_GNU),
    );
    let linux_base = Case::with_fake_tools(LINUX_TOOLS);
    cases.push(Case {
        group: Some("post-install"),
        name: "binary-cannot-execute",
        // TODO-RAINCLAUDE: 126 is what a shell reports for a file it found
        // but could not execute. Here the helper runs and exits with 126
        // itself to stand in for that; binary-cannot-be-started covers a
        // status that really comes from the shell.
        expect: Expect::FailsPostInstall { helper_exit: 126 },
        error_contains: &[
            "widget-project 0.9.148 (x86_64-unknown-linux-gnu) was installed to",
            "but it cannot run on this system (exit code 126)",
            "pass --target <TRIPLE>",
        ],
        ..linux_base
    });
    cases.push(Case {
        group: Some("post-install"),
        name: "binary-not-found",
        // TODO-RAINCLAUDE: as above, for the 127 a shell reports for a file
        // it could not find.
        expect: Expect::FailsPostInstall { helper_exit: 127 },
        error_contains: &["but it cannot run on this system (exit code 127)"],
        ..linux_base
    });

    for &hash_tool in HashTool::ALL {
        match hash_tool {
            // TODO-RAINCLAUDE: every other fake-tools case already runs with
            // sha256sum, so it needs no case of its own.
            HashTool::Sha256sum => continue,
            HashTool::Shasum | HashTool::Sha256 | HashTool::Digest | HashTool::Openssl => {}
        }
        cases.push(Case {
            group: Some("hash"),
            name: hash_tool.name(),
            ..Case::with_fake_tools(FakeTools {
                hash_tool: Some(hash_tool),
                ..LINUX_TOOLS
            })
        });
    }
    cases.push(Case {
        group: Some("hash"),
        name: "no-hash-tool",
        expect: FAILS_ON_ARCHIVE,
        error_contains: &["no SHA-256 tool was found, so the download cannot be verified"],
        ..Case::with_fake_tools(FakeTools {
            hash_tool: None,
            ..LINUX_TOOLS
        })
    });

    cases
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ToolDirCurl {
    Real,
    Fake,
    Absent,
}

pub(crate) fn build_tool_dir(
    dir: &Utf8Path,
    curl: ToolDirCurl,
    hash_tool: Option<HashTool>,
) -> Result<()> {
    use crate::fixture::HELPER;
    use std::os::unix::fs::symlink;

    const FAKE_HOST_TOOLS: &[&str] = &["uname", "getconf", "isainfo"];
    const REAL_TOOLS: &[&str] = &[
        "tar", "gunzip", "gzip", "mktemp", "tr", "mkdir", "rm", "mv", "chmod", "cat",
    ];

    fs::create_dir_all(dir).wrap_err_with(|| format!("failed to create {dir}"))?;
    let link = |original: &Utf8Path, name: &str| {
        let link = dir.join(name);
        symlink(original, &link).wrap_err_with(|| format!("failed to link {link} to {original}"))
    };
    let link_real = |tool: &str| {
        let path = find_program(tool).ok_or_else(|| eyre!("{tool} was not found on PATH"))?;
        link(&path, tool)
    };
    for tool in REAL_TOOLS {
        link_real(tool)?;
    }
    let helper = Utf8Path::new(HELPER);
    for tool in FAKE_HOST_TOOLS {
        link(helper, tool)?;
    }
    match curl {
        ToolDirCurl::Real => link_real("curl")?,
        ToolDirCurl::Fake => link(helper, "curl")?,
        ToolDirCurl::Absent => {}
    }
    if let Some(hash_tool) = hash_tool {
        link(helper, hash_tool.name())?;
    }
    Ok(())
}

// TODO-RAINCLAUDE: check curl's arguments with a fake curl that records them
// and fails.
pub(crate) fn curl_arguments(shell: &ShellSpec, shell_path: &Utf8Path) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE, shell.kind)?;
    let tool_dir = fixture.tool_dir();
    build_tool_dir(&tool_dir, ToolDirCurl::Fake, None)?;
    let log = tool_dir.join("curl-log.txt");

    let mut problems = Vec::new();
    let base_url = "https://unreachable.example";
    let mut env = FakeHost::LINUX_X86_64.env();
    env.push(("FAKE_CURL_LOG", Some(log.to_string())));
    let outcome = fixture.run(
        shell,
        shell_path,
        &Invocation {
            env,
            path: Some(tool_dir.clone()),
            ..Invocation::latest(Some(base_url.to_owned()), Launch::Sh(Mode::Args))
        },
    )?;
    let arguments = fs::read_to_string(&log)
        .wrap_err_with(|| format!("curl was not run for {base_url}:\n{outcome}"))?;
    let arguments = format!("\n{arguments}");
    let manifest_url = format!("{base_url}/latest/{MANIFEST_FILE_NAME}");

    if outcome.status.success()
        || !outcome
            .stderr
            .contains("failed to download the install manifest")
    {
        problems.push(format!(
            "{base_url}: expected a download failure on stderr:\n{outcome}"
        ));
    }
    // TODO-RAINCLAUDE: curl's own error says why the download failed, so
    // the script must pass it on.
    if !outcome.stderr.contains(FAKE_CURL_ERROR) {
        problems.push(format!(
            "{base_url}: curl's own error is not on stderr:\n{outcome}"
        ));
    }
    for text in [
        "\n--proto\n=https\n--proto-redir\n=https\n",
        "\n-fsSL\n",
        "\n--retry\n3\n",
        "\n--connect-timeout\n30\n",
        "\n--speed-limit\n1\n",
        "\n--speed-time\n60\n",
    ] {
        if !arguments.contains(text) {
            problems.push(format!(
                "{base_url}: curl's arguments do not include {text:?}:{arguments}"
            ));
        }
    }
    if !arguments.ends_with(&format!("\n{manifest_url}\n")) {
        problems.push(format!("{base_url}: the URL is not curl's last argument"));
    }
    fs::remove_file(&log).wrap_err_with(|| format!("failed to remove {log}"))?;
    fixture.verify_requests(&mut problems);
    report_problems(problems)
}

pub(crate) fn curl_not_found(shell: &ShellSpec, shell_path: &Utf8Path) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE, shell.kind)?;
    let tool_dir = fixture.tool_dir();
    build_tool_dir(&tool_dir, ToolDirCurl::Absent, Some(HashTool::Sha256sum))?;
    let install_dir = fixture.install_dir(InstallDirSource::Explicit);

    let outcome = fixture.run(
        shell,
        shell_path,
        &Invocation {
            env: FakeHost::LINUX_X86_64.env(),
            path: Some(tool_dir),
            ..Invocation::latest(Some(fixture.base_url()), Launch::Sh(Mode::Args))
        },
    )?;

    let mut problems = Vec::new();
    if outcome.status.success() {
        problems.push("the script succeeded without curl".to_owned());
    }
    for text in ["required command not found: curl", "install curl"] {
        if !outcome.stderr.contains(text) {
            problems.push(format!("stderr does not contain {text:?}"));
        }
    }
    if install_dir.exists() {
        problems.push(format!("the script created {install_dir}"));
    }
    fixture.verify_requests(&mut problems);
    report_problems(problems).map_err(|error| eyre!("{error}\n{outcome}"))
}
