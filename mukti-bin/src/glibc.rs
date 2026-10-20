// Copyright (c) The mukti Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use color_eyre::eyre::{eyre, Result, WrapErr};
use flate2::read::GzDecoder;
use mukti_metadata::GlibcVersion;
use object::{
    elf,
    read::elf::{FileHeader, SectionTable},
    Endianness,
};
use std::io::Read;

const ELF_MAGIC: &[u8; 4] = b"\x7fELF";
// e_ident[EI_CLASS] comes right after the magic bytes.
const ELF_CLASS_OFFSET: usize = 4;

/// Returns the minimum glibc symbol version required to run binaries from this
/// archive.
///
/// Returns the newest `GLIBC_x.y` symbol version that any ELF file in the
/// archive requires.
pub(crate) fn min_glibc_from_tar_gz(archive: &[u8]) -> Result<GlibcVersion> {
    let mut tar = tar::Archive::new(GzDecoder::new(archive));
    let entries = tar
        .entries()
        .wrap_err("failed to read the archive as a gzip-compressed tarball")?;

    let mut floor: Option<GlibcVersion> = None;
    let mut elf_count = 0;
    for entry in entries {
        let mut entry = entry.wrap_err("failed to read an entry in the archive")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        // The path is only used in error messages, so just treat it as a
        // string.
        let path = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
        let mut contents = Vec::new();
        entry
            .read_to_end(&mut contents)
            .wrap_err_with(|| format!("failed to read `{path}` from the archive"))?;
        if !contents.starts_with(ELF_MAGIC) {
            continue;
        }
        elf_count += 1;
        let required = required_glibc(&contents)
            .wrap_err_with(|| format!("failed to read `{path}` in the archive as an ELF file"))?;
        floor = floor.max(required);
    }

    match floor {
        Some(floor) => Ok(floor),
        None if elf_count == 0 => Err(eyre!("the archive contains no ELF files")),
        None => Err(eyre!(
            "none of the {elf_count} ELF file(s) in the archive require a glibc symbol version \
             (statically linked glibc builds are not supported)"
        )),
    }
}

fn required_glibc(data: &[u8]) -> Result<Option<GlibcVersion>> {
    match data.get(ELF_CLASS_OFFSET) {
        Some(&elf::ELFCLASS32) => required_glibc_for::<elf::FileHeader32<Endianness>>(data),
        Some(&elf::ELFCLASS64) => required_glibc_for::<elf::FileHeader64<Endianness>>(data),
        Some(class) => Err(eyre!("unknown ELF class {class}")),
        None => Err(eyre!("the file is too short to be an ELF file")),
    }
}

fn required_glibc_for<Elf>(data: &[u8]) -> Result<Option<GlibcVersion>>
where
    Elf: FileHeader<Endian = Endianness>,
{
    let header = Elf::parse(data)?;
    let endian = header.endian()?;
    let sections: SectionTable<'_, Elf> = header.sections(endian, data)?;
    let Some((mut verneeds, strings_index)) = sections.gnu_verneed(endian, data)? else {
        return Ok(None);
    };
    let strings = sections.strings(endian, data, strings_index)?;

    let mut floor = None;
    while let Some((_, mut vernauxes)) = verneeds.next()? {
        while let Some(vernaux) = vernauxes.next()? {
            let name = vernaux.name(endian, strings)?;
            floor = floor.max(parse_glibc_symbol_version(name)?);
        }
    }
    Ok(floor)
}

/// `GLIBC_ABI_DT_RELR` (packed relative relocations) was added in glibc 2.36
/// and needs to be detected explicitly here.
const GLIBC_ABI_DT_RELR: GlibcVersion = GlibcVersion::new(2, 36);

fn parse_glibc_symbol_version(name: &[u8]) -> Result<Option<GlibcVersion>> {
    let Some(rest) = name.strip_prefix(b"GLIBC_") else {
        return Ok(None);
    };
    if rest == b"PRIVATE" {
        return Ok(None);
    }
    if rest == b"ABI_DT_RELR" {
        return Ok(Some(GLIBC_ABI_DT_RELR));
    }
    // Very old versions of glibc produced three-part versions. Strip them down
    // to two parts.
    let numeric = std::str::from_utf8(rest).ok().and_then(|version| {
        let components: Vec<&str> = version.split('.').collect();
        let all_digits = components.iter().all(|component| {
            !component.is_empty() && component.bytes().all(|b| b.is_ascii_digit())
        });
        match components.as_slice() {
            [major, minor, ..] if all_digits => {
                Some(GlibcVersion::new(major.parse().ok()?, minor.parse().ok()?))
            }
            [] | [_] | [_, _, ..] => None,
        }
    });
    match numeric {
        Some(version) => Ok(Some(version)),
        None => Err(eyre!(
            "the binary requires the glibc symbol version `{}`, which mukti does not know; \
             add it to parse_glibc_symbol_version in mukti-bin with the glibc release that \
             introduced it",
            String::from_utf8_lossy(name),
        )),
    }
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use flate2::{write::GzEncoder, Compression};
    use object::{
        elf,
        write::elf::{FileHeader, Vernaux, Verneed, Writer},
        Endianness,
    };

    /// Returns a minimal ELF whose `.gnu.version_r` section needs the given
    /// versions from each library.
    pub(crate) fn elf_with_verneeds(is_64: bool, needs: &[(&str, &[&str])]) -> Vec<u8> {
        let mut buffer = Vec::new();
        let mut writer = Writer::new(Endianness::Little, is_64, &mut buffer);
        writer.reserve_file_header();

        let ids: Vec<_> = needs
            .iter()
            .map(|(file, versions)| {
                let file = writer.add_dynamic_string(file.as_bytes());
                let versions: Vec<_> = versions
                    .iter()
                    .map(|version| writer.add_dynamic_string(version.as_bytes()))
                    .collect();
                (file, versions)
            })
            .collect();
        let vernaux_count = needs.iter().map(|(_, versions)| versions.len()).sum();

        writer.reserve_shstrtab_section_index();
        writer.reserve_dynstr_section_index();
        writer.reserve_gnu_verneed_section_index();
        writer.reserve_dynstr();
        writer.reserve_gnu_verneed(needs.len(), vernaux_count);
        writer.reserve_shstrtab();
        writer.reserve_section_headers();

        writer
            .write_file_header(&FileHeader {
                os_abi: elf::ELFOSABI_NONE,
                abi_version: 0,
                e_type: elf::ET_DYN,
                e_machine: elf::EM_X86_64,
                e_entry: 0,
                e_flags: 0,
            })
            .expect("ELF file header written");
        writer.write_dynstr();
        writer.write_align_gnu_verneed();
        let mut index = 2;
        for (file, versions) in &ids {
            writer.write_gnu_verneed(&Verneed {
                version: elf::VER_NEED_CURRENT,
                aux_count: versions.len() as u16,
                file: *file,
            });
            for version in versions {
                writer.write_gnu_vernaux(&Vernaux {
                    flags: 0,
                    index,
                    name: *version,
                });
                index += 1;
            }
        }
        writer.write_shstrtab();
        writer.write_null_section_header();
        writer.write_shstrtab_section_header();
        writer.write_dynstr_section_header(0);
        writer.write_gnu_verneed_section_header(0);

        buffer
    }

    pub(crate) fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for (path, contents) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, path, *contents)
                .expect("tar entry appended");
        }
        builder
            .into_inner()
            .expect("tar archive finished")
            .finish()
            .expect("gzip stream finished")
    }

    /// Returns a typical set of verneeds for a glibc-linked Rust binary, with a
    /// 2.27 floor.
    pub(crate) fn glibc_binary() -> Vec<u8> {
        elf_with_verneeds(
            true,
            &[
                ("libgcc_s.so.1", &["GCC_3.0", "GCC_4.2.0"]),
                ("libc.so.6", &["GLIBC_2.2.5", "GLIBC_2.17", "GLIBC_2.27"]),
                ("libm.so.6", &["GLIBC_2.2.5"]),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{test_helpers::*, *};

    #[test]
    fn floor_is_the_newest_glibc_version_needed() {
        let archive = tar_gz(&[
            ("README.md", b"not an ELF file"),
            ("widget", &glibc_binary()),
            (
                "widget-helper",
                &elf_with_verneeds(true, &[("libc.so.6", &["GLIBC_2.2.5", "GLIBC_2.28"])]),
            ),
        ]);
        assert_eq!(
            min_glibc_from_tar_gz(&archive).expect("floor computed"),
            GlibcVersion::new(2, 28),
        );
    }

    #[test]
    fn floor_handles_32_bit_elf_and_odd_version_names() {
        let archive = tar_gz(&[(
            "widget",
            &elf_with_verneeds(
                false,
                &[(
                    "libc.so.6",
                    &["GLIBC_2.0", "GLIBC_PRIVATE", "GLIBC_2.10", "GLIBC_2.9"],
                )],
            ),
        )]);
        assert_eq!(
            min_glibc_from_tar_gz(&archive).expect("floor computed"),
            GlibcVersion::new(2, 10),
            "2.10 is newer than 2.9",
        );
    }

    #[test]
    fn packed_relative_relocations_need_glibc_2_36() {
        let archive = tar_gz(&[(
            "widget",
            &elf_with_verneeds(true, &[("libc.so.6", &["GLIBC_2.34", "GLIBC_ABI_DT_RELR"])]),
        )]);
        assert_eq!(
            min_glibc_from_tar_gz(&archive).expect("floor computed"),
            GlibcVersion::new(2, 36),
            "GLIBC_ABI_DT_RELR raises the floor above the newest numeric version",
        );
    }

    #[test]
    fn unknown_glibc_symbol_versions_are_errors() {
        for name in [
            "GLIBC_ABI_SOMETHING_NEW",
            "GLIBC_X.1",
            "GLIBC_2",
            "GLIBC_2.x",
            "GLIBC_.1",
        ] {
            let archive = tar_gz(&[(
                "widget",
                &elf_with_verneeds(true, &[("libc.so.6", &["GLIBC_2.17", name])]),
            )]);
            let error = min_glibc_from_tar_gz(&archive).expect_err("an unknown name is an error");
            let message = format!("{error:#}");
            assert!(
                message.contains("failed to read `widget` in the archive as an ELF file")
                    && message.contains(&format!("requires the glibc symbol version `{name}`")),
                "{name}: {message}",
            );
        }
    }

    #[test]
    fn floor_errors_say_what_is_wrong() {
        let cases: [(&str, Vec<u8>, &str); 4] = [
            (
                "not gzip",
                b"plain text".to_vec(),
                "failed to read an entry in the archive",
            ),
            (
                "no ELF files",
                tar_gz(&[("README.md", b"hello")]),
                "the archive contains no ELF files",
            ),
            (
                "no glibc versions",
                tar_gz(&[(
                    "widget",
                    &elf_with_verneeds(true, &[("libgcc_s.so.1", &["GCC_3.0"])]),
                )]),
                "none of the 1 ELF file(s) in the archive require a glibc symbol version",
            ),
            (
                "truncated ELF",
                tar_gz(&[("widget", &glibc_binary()[..20])]),
                "failed to read `widget` in the archive as an ELF file",
            ),
        ];
        for (description, archive, expected) in cases {
            let error = min_glibc_from_tar_gz(&archive).expect_err(description);
            let message = format!("{error:#}");
            assert!(
                message.contains(expected),
                "{description}: expected {expected:?} in {message:?}",
            );
        }
    }
}
