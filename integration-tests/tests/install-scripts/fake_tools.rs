// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test cases for `install.sh` against a constructed PATH.

use crate::{
    cases::{
        Case, CaseExpected, CaseInput, ExpectedAfterManifest, ExpectedResult, ExpectedTarget, BASE,
        RELEASE,
    },
    fixture::{
        CheckedRun, Fixture, Given, InstallDir, Invocation, Release, ScriptOptions, Value,
        MANIFEST_FILE_NAME,
    },
    runner::{
        check_no_install, ExpectedNoInstall, ExpectedNoInstallDir, ExpectedStatus, ExpectedStream,
    },
    shells::{find_program, FakeWindowsArch, ShName, ShPlatform, ShRun, ShellLaunch},
    targets::Target,
};
use camino::Utf8Path;
use color_eyre::eyre::{eyre, Context, Result};
use mukti_metadata::GlibcVersion;
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

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Sha256sum => "sha256sum",
            Self::Shasum => "shasum",
            Self::Sha256 => "sha256",
            Self::Digest => "digest",
            Self::Openssl => "openssl",
        }
    }

    const fn case_name(self) -> &'static str {
        match self {
            Self::Sha256sum => "hash::sha256sum",
            Self::Shasum => "hash::shasum",
            Self::Sha256 => "hash::sha256",
            Self::Digest => "hash::digest",
            Self::Openssl => "hash::openssl",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FakeTools {
    pub(crate) host: FakeHost,
    /// The hash tool to provide, or `None` for no hash tool at all.
    pub(crate) hash_tool: Option<HashTool>,
}

impl FakeTools {
    pub(crate) const fn new(host: FakeHost) -> Self {
        Self {
            host,
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

    pub(crate) const fn mingw(arch: FakeWindowsArch) -> Self {
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

impl ShRun {
    const fn with_fake_tools(tools: FakeTools) -> Self {
        Self {
            platform: ShPlatform::Fake(tools),
            ..Self::DEFAULT
        }
    }
}

impl Case {
    const fn with_fake_tools(tools: FakeTools) -> Self {
        Self {
            sh: Some(ShRun::with_fake_tools(tools)),
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
            name,
            expected: CaseExpected {
                target: ExpectedTarget::Exactly(target),
                progress_contains,
                ..BASE.expected
            },
            ..Self::with_fake_tools(FakeTools::new(host))
        }
    }

    const fn detect_fails(
        name: &'static str,
        host: FakeHost,
        result: ExpectedResult,
        error_contains: &'static [&'static str],
    ) -> Self {
        Self {
            name,
            expected: CaseExpected {
                result,
                target: ExpectedTarget::NotChecked,
                error_contains,
                ..BASE.expected
            },
            ..Self::with_fake_tools(FakeTools::new(host))
        }
    }
}

const FAKE_CURL_ERROR: &str = "fake curl: (7) could not connect";

const AARCH64_FLOOR_2_35: &[(Target, GlibcVersion)] = &[
    (Target::X86_64_UNKNOWN_LINUX_GNU, GlibcVersion::new(2, 27)),
    (Target::AARCH64_UNKNOWN_LINUX_GNU, GlibcVersion::new(2, 35)),
    (
        Target::RISCV64GC_UNKNOWN_LINUX_GNU,
        GlibcVersion::new(2, 27),
    ),
];
const RISCV64_FLOOR_2_40: &[(Target, GlibcVersion)] = &[
    (Target::X86_64_UNKNOWN_LINUX_GNU, GlibcVersion::new(2, 27)),
    (Target::AARCH64_UNKNOWN_LINUX_GNU, GlibcVersion::new(2, 27)),
    (
        Target::RISCV64GC_UNKNOWN_LINUX_GNU,
        GlibcVersion::new(2, 40),
    ),
];

const UNSUPPORTED: &[&str] = &["no widget-project build is known for this platform"];

pub(crate) fn fake_tool_cases() -> Vec<Case> {
    let below_floor_with_target = Case::detects(
        "detect::linux-x86_64-glibc-below-floor-with-target",
        FakeHost::linux("x86_64", Some("2.26")),
        Target::X86_64_UNKNOWN_LINUX_GNU,
        &[],
    );
    let mut cases = vec![
        Case::detects(
            "detect::linux-x86_64-glibc-at-floor",
            FakeHost::linux("x86_64", Some("2.27")),
            Target::X86_64_UNKNOWN_LINUX_GNU,
            &[],
        ),
        Case::detects(
            "detect::linux-x86_64-glibc-next-major",
            FakeHost::linux("x86_64", Some("3.0")),
            Target::X86_64_UNKNOWN_LINUX_GNU,
            &[],
        ),
        Case::detects(
            "detect::linux-x86_64-glibc-below-floor",
            FakeHost::linux("x86_64", Some("2.26")),
            Target::X86_64_UNKNOWN_LINUX_MUSL,
            &["glibc 2.26 is older than the 2.27 that the x86_64-unknown-linux-gnu build needs, so the x86_64-unknown-linux-musl build was chosen instead"],
        ),
        Case::detects(
            "detect::linux-x86_64-glibc-patch-version",
            FakeHost::linux("x86_64", Some("2.26.1")),
            Target::X86_64_UNKNOWN_LINUX_MUSL,
            &["glibc 2.26 is older than the 2.27 that the x86_64-unknown-linux-gnu build needs, so the x86_64-unknown-linux-musl build was chosen instead"],
        ),
        // An explicit --target skips glibc detection, so in that case the floor
        // must not apply.
        Case {
            input: CaseInput {
                options: ScriptOptions::ARGS.with_target(Given::arg(Value::Target(
                    Target::X86_64_UNKNOWN_LINUX_GNU,
                ))),
                ..BASE.input
            },
            expected: CaseExpected {
                output_excludes: &["is older than"],
                ..below_floor_with_target.expected
            },
            ..below_floor_with_target
        },
        Case::detects(
            "detect::linux-x86_64-no-glibc",
            FakeHost::linux("x86_64", None),
            Target::X86_64_UNKNOWN_LINUX_MUSL,
            &["glibc was not detected, so only statically linked musl builds were considered"],
        ),
        Case {
            input: CaseInput {
                release: Release {
                    targets: &[Target::X86_64_UNKNOWN_LINUX_MUSL],
                    ..RELEASE
                },
                ..BASE.input
            },
            ..Case::detects(
                "detect::linux-x86_64-glibc-without-gnu-build",
                FakeHost::LINUX_X86_64,
                Target::X86_64_UNKNOWN_LINUX_MUSL,
                &[],
            )
        },
        // A 64-bit kernel with 32-bit programs -- glibc looks new enough, but
        // the platform only supports musl builds.
        Case::detects(
            "detect::linux-x86_64-32-bit-userland",
            FakeHost::Unix(FakeUnix {
                long_bit: Some("32"),
                ..FakeUnix::linux("x86_64", Some("2.39"))
            }),
            Target::X86_64_UNKNOWN_LINUX_MUSL,
            &["64-bit kernel (x86_64) with a 32-bit userland"],
        ),
        Case::detects(
            "detect::linux-aarch64-glibc",
            FakeHost::linux("aarch64", Some("2.31")),
            Target::AARCH64_UNKNOWN_LINUX_GNU,
            &[],
        ),
        Case::detects(
            "detect::linux-riscv64-glibc",
            FakeHost::linux("riscv64", Some("2.35")),
            Target::RISCV64GC_UNKNOWN_LINUX_GNU,
            &[],
        ),
        // The release doesn't have a riscv64 musl build.
        Case::detect_fails(
            "detect::linux-riscv64-no-glibc",
            FakeHost::linux("riscv64", None),
            ExpectedResult::ReadsManifest {
                manifest: "latest",
                then: ExpectedAfterManifest::FailsBeforeArchive,
            },
            &[
                "has no tar.gz archive for this platform",
                "looked for: riscv64gc-unknown-linux-musl",
                "glibc was not detected, so only statically linked musl builds were considered",
                "riscv64gc-unknown-linux-gnu",
            ],
        ),
        // The glibc floor must come from the chosen archive's own line.
        Case {
            input: CaseInput {
                release: Release {
                    glibc_floors: AARCH64_FLOOR_2_35,
                    ..RELEASE
                },
                ..BASE.input
            },
            ..Case::detects(
                "detect::linux-aarch64-glibc-below-this-archives-floor",
                FakeHost::linux("aarch64", Some("2.31")),
                Target::AARCH64_UNKNOWN_LINUX_MUSL,
                &["glibc 2.31 is older than the 2.35 that the aarch64-unknown-linux-gnu build needs, so the aarch64-unknown-linux-musl build was chosen instead"],
            )
        },
        // There's no riscv64 musl build to fall back to, so if the glibc is
        // below the floor, we have to fail completely.
        Case {
            input: CaseInput {
                release: Release {
                    glibc_floors: RISCV64_FLOOR_2_40,
                    ..RELEASE
                },
                ..BASE.input
            },
            ..Case::detect_fails(
                "detect::linux-riscv64-glibc-below-floor",
                FakeHost::linux("riscv64", Some("2.35")),
                ExpectedResult::ReadsManifest {
                    manifest: "latest",
                    then: ExpectedAfterManifest::FailsBeforeArchive,
                },
                &[
                    "has no tar.gz archive for this platform",
                    "looked for: riscv64gc-unknown-linux-gnu riscv64gc-unknown-linux-musl",
                    "glibc 2.35 is older than the 2.40 that the riscv64gc-unknown-linux-gnu build needs",
                ],
            )
        },
        Case::detect_fails(
            "detect::linux-android",
            FakeHost::other("Linux", "aarch64", Some("Android")),
            ExpectedResult::FailsBeforeManifest,
            UNSUPPORTED,
        ),
        Case::detects(
            "detect::darwin-arm64",
            FakeHost::other("Darwin", "arm64", None),
            Target::UNIVERSAL_APPLE_DARWIN,
            &[],
        ),
        Case {
            input: CaseInput {
                release: Release {
                    targets: &[
                        Target::UNIVERSAL_APPLE_DARWIN,
                        Target::AARCH64_APPLE_DARWIN,
                        Target::X86_64_APPLE_DARWIN,
                    ],
                    ..RELEASE
                },
                ..BASE.input
            },
            ..Case::detects(
                "detect::darwin-prefers-native-over-universal",
                FakeHost::other("Darwin", "arm64", None),
                Target::AARCH64_APPLE_DARWIN,
                &[],
            )
        },
        Case::detects(
            "detect::freebsd-amd64",
            FakeHost::other("FreeBSD", "amd64", Some("FreeBSD")),
            Target::X86_64_UNKNOWN_FREEBSD,
            &[],
        ),
        Case::detects(
            "detect::illumos-x86_64",
            FakeHost::Unix(FakeUnix {
                isainfo: Some("amd64"),
                ..FakeUnix::other("SunOS", "i86pc", Some("illumos"))
            }),
            Target::X86_64_UNKNOWN_ILLUMOS,
            &[],
        ),
        Case::detect_fails(
            "detect::illumos-32-bit",
            FakeHost::Unix(FakeUnix {
                isainfo: Some("i386"),
                ..FakeUnix::other("SunOS", "i86pc", Some("illumos"))
            }),
            ExpectedResult::FailsBeforeManifest,
            UNSUPPORTED,
        ),
        Case::detect_fails(
            "detect::solaris",
            FakeHost::other("SunOS", "i86pc", Some("Solaris")),
            ExpectedResult::FailsBeforeManifest,
            UNSUPPORTED,
        ),
        Case::detect_fails(
            "detect::mingw-no-architecture",
            FakeHost::mingw(FakeWindowsArch {
                machine: "PDP11",
                processor_architecture: None,
                processor_architew6432: None,
            }),
            ExpectedResult::FailsBeforeManifest,
            &["PROCESSOR_ARCHITECTURE: unset"],
        ),
        Case::detects(
            "detect::msys",
            FakeHost::Windows {
                uname_s: "MSYS_NT-10.0-26100",
                arch: FakeWindowsArch::new("AMD64", "AMD64", None),
            },
            Target::X86_64_PC_WINDOWS_MSVC,
            &[],
        ),
        Case::detect_fails(
            "detect::unknown-os",
            FakeHost::other("Plan9", "x86_64", None),
            ExpectedResult::FailsBeforeManifest,
            &["OS: Plan9, architecture: x86_64"],
        ),
    ];

    const LINUX_TOOLS: FakeTools = FakeTools::new(FakeHost::LINUX_X86_64);
    let linux_base = Case {
        expected: CaseExpected {
            target: ExpectedTarget::Exactly(Target::X86_64_UNKNOWN_LINUX_GNU),
            ..BASE.expected
        },
        ..Case::with_fake_tools(LINUX_TOOLS)
    };
    cases.push(Case {
        // Simulate a file that was found but could not be executed by exiting
        // with code 126.
        //
        // Also see the `"binary-cannot-be-started"` test in `cases.rs`.
        name: "post-install::binary-cannot-execute",
        input: CaseInput {
            post_install_exit_code: Some(126),
            ..BASE.input
        },
        expected: CaseExpected {
            result: ExpectedResult::ReadsManifest {
                manifest: "latest",
                then: ExpectedAfterManifest::FailsPostInstall,
            },
            error_contains: &[
                "widget-project 0.9.148 (x86_64-unknown-linux-gnu) was installed to",
                "but it cannot run on this system (exit code 126)",
                "pass --target <TRIPLE>",
            ],
            ..linux_base.expected
        },
        ..linux_base
    });
    cases.push(Case {
        // Similar to `"binary-cannot-execute"`, but with exit code 127.
        name: "post-install::binary-not-found",
        input: CaseInput {
            post_install_exit_code: Some(127),
            ..BASE.input
        },
        expected: CaseExpected {
            result: ExpectedResult::ReadsManifest {
                manifest: "latest",
                then: ExpectedAfterManifest::FailsPostInstall,
            },
            error_contains: &["but it cannot run on this system (exit code 127)"],
            ..linux_base.expected
        },
        ..linux_base
    });

    for &hash_tool in HashTool::ALL {
        match hash_tool {
            // The other fake-tools cases cover `Sha256sum` as the default
            // value, so we don't need yet another case here.
            HashTool::Sha256sum => continue,
            HashTool::Shasum | HashTool::Sha256 | HashTool::Digest | HashTool::Openssl => {}
        }
        cases.push(Case {
            name: hash_tool.case_name(),
            sh: Some(ShRun::with_fake_tools(FakeTools {
                hash_tool: Some(hash_tool),
                ..LINUX_TOOLS
            })),
            ..linux_base
        });
    }
    cases.push(Case {
        name: "hash::no-hash-tool",
        sh: Some(ShRun::with_fake_tools(FakeTools {
            hash_tool: None,
            ..LINUX_TOOLS
        })),
        expected: CaseExpected {
            result: ExpectedResult::ReadsManifest {
                manifest: "latest",
                then: ExpectedAfterManifest::FailsOnArchive,
            },
            error_contains: &["no SHA-256 tool was found, so the download cannot be verified"],
            ..linux_base.expected
        },
        ..linux_base
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

/// Checks curl's arguments with a fake curl that records them and fails.
pub(crate) fn curl_arguments(shell: ShName, shell_path: &Utf8Path) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE)?;
    let tool_dir = fixture.tool_dir();
    build_tool_dir(&tool_dir, ToolDirCurl::Fake, None)?;
    let log = tool_dir.join("curl-log.txt");

    let base_url = "https://unreachable.example";
    let mut env = FakeHost::LINUX_X86_64.env();
    env.push(("FAKE_CURL_LOG", Some(log.to_string())));
    let result = fixture
        .run(
            shell_path,
            &Invocation {
                options: ScriptOptions {
                    base_url: Given::arg(Value::Literal(base_url)),
                    ..ScriptOptions::ARGS
                },
                env,
                path: Some(tool_dir.clone()),
                ..Invocation::new(ShellLaunch::Sh(shell, ShRun::DEFAULT))
            },
        )
        .and_then(|outcome| {
            let arguments = fs::read_to_string(&log)
                .wrap_err_with(|| format!("curl was not run for {base_url}:\n{outcome}"))?;
            let arguments = format!("\n{arguments}");
            let manifest_url = format!("{base_url}/latest/{MANIFEST_FILE_NAME}");

            let mut problems = Vec::new();
            if outcome.status.success()
                || !outcome
                    .stderr
                    .contains("failed to download the install manifest")
            {
                problems.push(format!("{base_url}: expected a download failure on stderr"));
            }
            // Real curl's error indicates why the download failed, so the script must
            // display it.
            if !outcome.stderr.contains(FAKE_CURL_ERROR) {
                problems.push(format!("{base_url}: curl's own error is not on stderr"));
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
            Ok(CheckedRun { problems, outcome })
        });
    fixture.finish(result)
}

pub(crate) fn curl_not_found(shell: ShName, shell_path: &Utf8Path) -> Result<()> {
    let mut fixture = Fixture::without_archive(&RELEASE)?;
    let tool_dir = fixture.tool_dir();
    build_tool_dir(&tool_dir, ToolDirCurl::Absent, Some(HashTool::Sha256sum))?;
    let install_dir = fixture.install_dir(InstallDir::Plain);

    let result = fixture
        .run(
            shell_path,
            &Invocation {
                env: FakeHost::LINUX_X86_64.env(),
                path: Some(tool_dir),
                ..Invocation::new(ShellLaunch::Sh(shell, ShRun::DEFAULT))
            },
        )
        .and_then(|outcome| {
            let expected = ExpectedNoInstall {
                status: ExpectedStatus::Failure,
                stdout: ExpectedStream::Empty,
                stderr: ExpectedStream::Contains(&[
                    "required command not found: curl",
                    "install curl",
                ]),
                install_dir: ExpectedNoInstallDir::NotCreated,
            };
            let problems = check_no_install(&outcome, &install_dir, &expected)?;
            Ok(CheckedRun { problems, outcome })
        });
    fixture.finish(result)
}
