use std::io;

const LOCAL_SIGNATURE: u32 = 0x0403_4B50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4B50;
const END_SIGNATURE: u32 = 0x0605_4B50;
const END64_SIGNATURE: u32 = 0x0606_4B50;
const END64_LOCATOR_SIGNATURE: u32 = 0x0706_4B50;
const ZIP64_EXTRA_ID: u16 = 0x0001;
const UTF8_FLAG: u16 = 0x0800;
const MAX_U32: u64 = 0xFFFF_FFFF;
const LOCAL_HEADER: usize = 30;
const CENTRAL_HEADER: usize = 46;
const END_RECORD: usize = 22;
const END64_RECORD: usize = 56;
const END64_LOCATOR: usize = 20;

pub struct Archive {
    data: Vec<u8>,
    members: Vec<Member>,
}

pub struct Member {
    pub filename: String,
    pub orig_filename: String,
    pub is_dir: bool,
    pub create_system: u8,
    pub external_attr: u32,
    pub method: u16,
    pub flags: u16,
    pub crc32: u32,
    pub offset: u64,
    pub end_offset: u64,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
}

impl Archive {
    pub fn open(data: Vec<u8>) -> Result<Self, io::Error> {
        let end = EndRecord::find(&data)?;
        let (count, offset) = if end.zip64 {
            let record = End64::read(&data, find_locator(&data, end.offset_absolute)?)?;
            (record.count, record.offset)
        } else {
            (u64::from(end.count), end.offset)
        };
        let mut members = Vec::new();
        let mut cursor = usize::try_from(offset).map_err(|_| bad("central directory offset"))?;
        for _ in 0..count {
            members.push(CentralEntry::read(&data, &mut cursor)?.into_member()?);
        }
        let mut ordered: Vec<&mut Member> = members.iter_mut().collect();
        ordered.sort_by(|left, right| right.offset.cmp(&left.offset));
        let mut end_offset = offset;
        for member in ordered {
            member.end_offset = end_offset;
            end_offset = member.offset;
        }
        Ok(Self { data, members })
    }

    pub fn members(&self) -> &[Member] {
        &self.members
    }

    pub fn read(&self, member: &Member) -> Result<Vec<u8>, io::Error> {
        let at = usize::try_from(member.offset).map_err(|_| bad("local header offset"))?;
        if u32_at(&self.data, at)? != LOCAL_SIGNATURE {
            return Err(bad("bad local file header magic"));
        }
        if member.flags & 0x0020 != 0 {
            return Err(bad("compressed patched data (flag bit 5)"));
        }
        if member.flags & 0x0040 != 0 {
            return Err(bad("strong encryption (flag bit 6)"));
        }
        let flags = u16_at(&self.data, at + 6)?;
        let name_length = usize::from(u16_at(&self.data, at + 26)?);
        let extra_length = usize::from(u16_at(&self.data, at + 28)?);
        let name = slice(&self.data, at + LOCAL_HEADER, name_length)
            .ok_or_else(|| bad("truncated local file name"))?;
        if decode_name(name, flags & UTF8_FLAG != 0)?.as_bytes() != member.orig_filename.as_bytes()
        {
            return Err(bad(
                "file name in local header and central directory differ",
            ));
        }
        let start = at + LOCAL_HEADER + name_length + extra_length;
        if member.end_offset != member.offset
            && u64::try_from(start)
                .ok()
                .and_then(|start| start.checked_add(member.compressed_size))
                .is_some_and(|end| end > member.end_offset)
        {
            return Err(bad("overlapped entries"));
        }
        if member.flags & 0x0001 != 0 {
            return Err(bad("archive member is encrypted"));
        }
        let length = usize::try_from(member.compressed_size).map_err(|_| bad("member size"))?;
        let payload = slice(&self.data, start, length).ok_or_else(|| bad("truncated member"))?;
        let output = inflate(member.method, payload)?;
        if u64::try_from(output.len()).unwrap_or(u64::MAX) != member.uncompressed_size {
            return Err(bad("member size mismatch"));
        }
        if crc32(&output) != member.crc32 {
            return Err(bad("bad member CRC"));
        }
        Ok(output)
    }
}

struct EndRecord {
    count: u16,
    offset: u64,
    offset_absolute: usize,
    zip64: bool,
}

impl EndRecord {
    fn find(data: &[u8]) -> Result<Self, io::Error> {
        let mut at = data
            .len()
            .checked_sub(END_RECORD)
            .ok_or_else(|| bad("archive is not a zip file"))?;
        let minimum = data.len().saturating_sub(65_535 + END_RECORD);
        loop {
            if u32_at(data, at)? == END_SIGNATURE
                && usize::from(u16_at(data, at + 20)?) == data.len() - at - END_RECORD
            {
                let count = u16_at(data, at + 10)?;
                let offset = u64::from(u32_at(data, at + 16)?);
                return Ok(Self {
                    count,
                    offset,
                    offset_absolute: at,
                    zip64: count == 0xFFFF || offset == MAX_U32,
                });
            }
            if at == minimum {
                return Err(bad("archive is not a zip file"));
            }
            at -= 1;
        }
    }
}

struct End64 {
    count: u64,
    offset: u64,
}

impl End64 {
    fn read(data: &[u8], at: usize) -> Result<Self, io::Error> {
        if slice(data, at, END64_RECORD).is_none() {
            return Err(bad("truncated zip64 end record"));
        }
        if u32_at(data, at)? != END64_SIGNATURE {
            return Err(bad("bad zip64 end record magic"));
        }
        Ok(Self {
            count: u64_at(data, at + 32)?,
            offset: u64_at(data, at + 48)?,
        })
    }
}

fn find_locator(data: &[u8], end_at: usize) -> Result<usize, io::Error> {
    let at = end_at
        .checked_sub(END64_LOCATOR)
        .ok_or_else(|| bad("truncated zip64 locator"))?;
    if u32_at(data, at)? != END64_LOCATOR_SIGNATURE {
        return Err(bad("bad zip64 locator magic"));
    }
    usize::try_from(u64_at(data, at + 8)?).map_err(|_| bad("zip64 end record offset"))
}

struct CentralEntry {
    create_system: u8,
    flags: u16,
    method: u16,
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
    external_attr: u32,
    offset: u64,
    name: Vec<u8>,
}

impl CentralEntry {
    fn read(data: &[u8], cursor: &mut usize) -> Result<Self, io::Error> {
        let at = *cursor;
        if slice(data, at, CENTRAL_HEADER).is_none() {
            return Err(bad("truncated central directory entry"));
        }
        if u32_at(data, at)? != CENTRAL_SIGNATURE {
            return Err(bad("bad central directory magic"));
        }
        let create_system = *data.get(at + 5).ok_or_else(|| bad("field"))?;
        let flags = u16_at(data, at + 8)?;
        let method = u16_at(data, at + 10)?;
        let crc32 = u32_at(data, at + 16)?;
        let compressed = u32_at(data, at + 20)?;
        let uncompressed = u32_at(data, at + 24)?;
        let name_length = usize::from(u16_at(data, at + 28)?);
        let extra_length = usize::from(u16_at(data, at + 30)?);
        let comment_length = usize::from(u16_at(data, at + 32)?);
        let external_attr = u32_at(data, at + 38)?;
        let offset = u32_at(data, at + 42)?;
        let body = at + CENTRAL_HEADER;
        let name = slice(data, body, name_length)
            .ok_or_else(|| bad("truncated central directory entry"))?
            .to_vec();
        let extra = slice(data, body + name_length, extra_length)
            .ok_or_else(|| bad("truncated central directory entry"))?;
        let sizes = expand_zip64(compressed, uncompressed, offset, extra);
        *cursor = body + name_length + extra_length + comment_length;
        if *cursor > data.len() {
            return Err(bad("truncated central directory entry"));
        }
        Ok(Self {
            create_system,
            flags,
            method,
            crc32,
            compressed_size: sizes.0,
            uncompressed_size: sizes.1,
            external_attr,
            offset: sizes.2,
            name,
        })
    }

    fn into_member(self) -> Result<Member, io::Error> {
        let orig_filename = decode_name(&self.name, self.flags & UTF8_FLAG != 0)?;
        let filename = orig_filename
            .split('\0')
            .next()
            .unwrap_or_default()
            .to_owned();
        Ok(Member {
            is_dir: filename.ends_with('/'),
            filename,
            orig_filename,
            create_system: self.create_system,
            external_attr: self.external_attr,
            method: self.method,
            flags: self.flags,
            crc32: self.crc32,
            offset: self.offset,
            end_offset: 0,
            compressed_size: self.compressed_size,
            uncompressed_size: self.uncompressed_size,
        })
    }
}

fn decode_name(raw: &[u8], utf8: bool) -> Result<String, io::Error> {
    if utf8 {
        String::from_utf8(raw.to_vec()).map_err(|_| bad("archive entry name is not valid UTF-8"))
    } else {
        Ok(raw.iter().map(|&byte| cp437(byte)).collect())
    }
}

fn expand_zip64(compressed: u32, uncompressed: u32, offset: u32, extra: &[u8]) -> (u64, u64, u64) {
    let mut uncompressed = u64::from(uncompressed);
    let mut compressed = u64::from(compressed);
    let mut offset = u64::from(offset);
    let mut cursor = 0;
    while let (Ok(id), Ok(size)) = (u16_at(extra, cursor), u16_at(extra, cursor + 2)) {
        let body = cursor + 4;
        let end = body + usize::from(size);
        cursor = end;
        if id != ZIP64_EXTRA_ID {
            continue;
        }
        let mut field = body;
        for value in [&mut uncompressed, &mut compressed, &mut offset] {
            if *value == MAX_U32 && field + 8 <= end {
                if let Ok(replacement) = u64_at(extra, field) {
                    *value = replacement;
                }
                field += 8;
            }
        }
        break;
    }
    (compressed, uncompressed, offset)
}

fn slice(data: &[u8], at: usize, length: usize) -> Option<&[u8]> {
    data.get(at..at.checked_add(length)?)
}

fn u16_at(data: &[u8], at: usize) -> Result<u16, io::Error> {
    let field: [u8; 2] = slice(data, at, 2)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| bad("truncated field"))?;
    Ok(u16::from_le_bytes(field))
}

fn u32_at(data: &[u8], at: usize) -> Result<u32, io::Error> {
    let field: [u8; 4] = slice(data, at, 4)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| bad("truncated field"))?;
    Ok(u32::from_le_bytes(field))
}

fn u64_at(data: &[u8], at: usize) -> Result<u64, io::Error> {
    let field: [u8; 8] = slice(data, at, 8)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| bad("truncated field"))?;
    Ok(u64::from_le_bytes(field))
}

fn inflate(method: u16, payload: &[u8]) -> Result<Vec<u8>, io::Error> {
    match method {
        0 => Ok(payload.to_vec()),
        8 => miniz_oxide::inflate::decompress_to_vec(payload)
            .map_err(|_| bad("member uses invalid deflate data")),
        _ => Err(bad("unsupported archive compression method")),
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ 0xEDB8_8320
            };
        }
    }
    !crc
}

#[allow(clippy::too_many_lines)]
fn cp437(byte: u8) -> char {
    match byte {
        0x00..=0x7F => char::from(byte),
        0x80 => '\u{00C7}',
        0x81 => '\u{00FC}',
        0x82 => '\u{00E9}',
        0x83 => '\u{00E2}',
        0x84 => '\u{00E4}',
        0x85 => '\u{00E0}',
        0x86 => '\u{00E5}',
        0x87 => '\u{00E7}',
        0x88 => '\u{00EA}',
        0x89 => '\u{00EB}',
        0x8A => '\u{00E8}',
        0x8B => '\u{00EF}',
        0x8C => '\u{00EE}',
        0x8D => '\u{00EC}',
        0x8E => '\u{00C4}',
        0x8F => '\u{00C5}',
        0x90 => '\u{00C9}',
        0x91 => '\u{00E6}',
        0x92 => '\u{00C6}',
        0x93 => '\u{00F4}',
        0x94 => '\u{00F6}',
        0x95 => '\u{00F2}',
        0x96 => '\u{00FB}',
        0x97 => '\u{00F9}',
        0x98 => '\u{00FF}',
        0x99 => '\u{00D6}',
        0x9A => '\u{00DC}',
        0x9B => '\u{00A2}',
        0x9C => '\u{00A3}',
        0x9D => '\u{00A5}',
        0x9E => '\u{20A7}',
        0x9F => '\u{0192}',
        0xA0 => '\u{00E1}',
        0xA1 => '\u{00ED}',
        0xA2 => '\u{00F3}',
        0xA3 => '\u{00FA}',
        0xA4 => '\u{00F1}',
        0xA5 => '\u{00D1}',
        0xA6 => '\u{00AA}',
        0xA7 => '\u{00BA}',
        0xA8 => '\u{00BF}',
        0xA9 => '\u{2310}',
        0xAA => '\u{00AC}',
        0xAB => '\u{00BD}',
        0xAC => '\u{00BC}',
        0xAD => '\u{00A1}',
        0xAE => '\u{00AB}',
        0xAF => '\u{00BB}',
        0xB0 => '\u{2591}',
        0xB1 => '\u{2592}',
        0xB2 => '\u{2593}',
        0xB3 => '\u{2502}',
        0xB4 => '\u{2524}',
        0xB5 => '\u{2561}',
        0xB6 => '\u{2562}',
        0xB7 => '\u{2556}',
        0xB8 => '\u{2555}',
        0xB9 => '\u{2563}',
        0xBA => '\u{2551}',
        0xBB => '\u{2557}',
        0xBC => '\u{255D}',
        0xBD => '\u{255C}',
        0xBE => '\u{255B}',
        0xBF => '\u{2510}',
        0xC0 => '\u{2514}',
        0xC1 => '\u{2534}',
        0xC2 => '\u{252C}',
        0xC3 => '\u{251C}',
        0xC4 => '\u{2500}',
        0xC5 => '\u{253C}',
        0xC6 => '\u{255E}',
        0xC7 => '\u{255F}',
        0xC8 => '\u{255A}',
        0xC9 => '\u{2554}',
        0xCA => '\u{2569}',
        0xCB => '\u{2566}',
        0xCC => '\u{2560}',
        0xCD => '\u{2550}',
        0xCE => '\u{256C}',
        0xCF => '\u{2567}',
        0xD0 => '\u{2568}',
        0xD1 => '\u{2564}',
        0xD2 => '\u{2565}',
        0xD3 => '\u{2559}',
        0xD4 => '\u{2558}',
        0xD5 => '\u{2552}',
        0xD6 => '\u{2553}',
        0xD7 => '\u{256B}',
        0xD8 => '\u{256A}',
        0xD9 => '\u{2518}',
        0xDA => '\u{250C}',
        0xDB => '\u{2588}',
        0xDC => '\u{2584}',
        0xDD => '\u{258C}',
        0xDE => '\u{2590}',
        0xDF => '\u{2580}',
        0xE0 => '\u{03B1}',
        0xE1 => '\u{00DF}',
        0xE2 => '\u{0393}',
        0xE3 => '\u{03C0}',
        0xE4 => '\u{03A3}',
        0xE5 => '\u{03C3}',
        0xE6 => '\u{00B5}',
        0xE7 => '\u{03C4}',
        0xE8 => '\u{03A6}',
        0xE9 => '\u{0398}',
        0xEA => '\u{03A9}',
        0xEB => '\u{03B4}',
        0xEC => '\u{221E}',
        0xED => '\u{03C6}',
        0xEE => '\u{03B5}',
        0xEF => '\u{2229}',
        0xF0 => '\u{2261}',
        0xF1 => '\u{00B1}',
        0xF2 => '\u{2265}',
        0xF3 => '\u{2264}',
        0xF4 => '\u{2320}',
        0xF5 => '\u{2321}',
        0xF6 => '\u{00F7}',
        0xF7 => '\u{2248}',
        0xF8 => '\u{00B0}',
        0xF9 => '\u{2219}',
        0xFA => '\u{00B7}',
        0xFB => '\u{221A}',
        0xFC => '\u{207F}',
        0xFD => '\u{00B2}',
        0xFE => '\u{25A0}',
        0xFF => '\u{00A0}',
    }
}

fn bad(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]
pub mod test_archive {
    use super::{CENTRAL_SIGNATURE, LOCAL_SIGNATURE, UTF8_FLAG, crc32};

    const VERSION_NEEDED: u16 = 20;

    #[derive(Clone)]
    pub struct Spec {
        pub name: String,
        pub raw_name: Option<Vec<u8>>,
        pub data: Vec<u8>,
        pub method: u16,
        pub flag_bits: u16,
        pub create_system: u8,
        pub external_attr: u32,
    }

    impl Spec {
        fn name_bytes(&self) -> Vec<u8> {
            self.raw_name
                .clone()
                .unwrap_or_else(|| self.name.clone().into_bytes())
        }

        fn version_made_by(&self) -> u16 {
            u16::from(self.create_system) << 8
        }

        fn flags(&self) -> u16 {
            if self.raw_name.is_none() && !self.name.is_ascii() {
                self.flag_bits | UTF8_FLAG
            } else {
                self.flag_bits
            }
        }
    }

    pub fn blank() -> Spec {
        Spec {
            name: String::new(),
            raw_name: None,
            data: Vec::new(),
            method: 0,
            flag_bits: 0,
            create_system: 3,
            external_attr: 0,
        }
    }

    pub fn file(name: impl Into<String>, data: impl Into<Vec<u8>>) -> Spec {
        Spec {
            name: name.into(),
            data: data.into(),
            ..blank()
        }
    }

    pub fn directory(name: impl Into<String>) -> Spec {
        let name = name.into();
        let name = if name.ends_with('/') {
            name
        } else {
            format!("{name}/")
        };
        file(name, Vec::new())
    }

    pub fn write_archive(specs: &[Spec]) -> Vec<u8> {
        let mut body = Vec::new();
        let mut central = Vec::new();
        for spec in specs {
            let payload = if spec.method == 8 {
                miniz_oxide::deflate::compress_to_vec(&spec.data, 6)
            } else {
                spec.data.clone()
            };
            let crc = crc32(&spec.data);
            let offset = body.len() as u32;
            let name = spec.name_bytes();
            let stored = payload.len() as u32;
            let size = spec.data.len() as u32;
            body.extend_from_slice(&LOCAL_SIGNATURE.to_le_bytes());
            body.extend_from_slice(&VERSION_NEEDED.to_le_bytes());
            body.extend_from_slice(&spec.flags().to_le_bytes());
            body.extend_from_slice(&spec.method.to_le_bytes());
            body.extend_from_slice(&0u16.to_le_bytes());
            body.extend_from_slice(&0x21u16.to_le_bytes());
            body.extend_from_slice(&crc.to_le_bytes());
            body.extend_from_slice(&stored.to_le_bytes());
            body.extend_from_slice(&size.to_le_bytes());
            body.extend_from_slice(&(name.len() as u16).to_le_bytes());
            body.extend_from_slice(&0u16.to_le_bytes());
            body.extend_from_slice(&name);
            body.extend_from_slice(&payload);
            central.extend_from_slice(&CENTRAL_SIGNATURE.to_le_bytes());
            central.extend_from_slice(&spec.version_made_by().to_le_bytes());
            central.extend_from_slice(&VERSION_NEEDED.to_le_bytes());
            central.extend_from_slice(&spec.flags().to_le_bytes());
            central.extend_from_slice(&spec.method.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0x21u16.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&stored.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&spec.external_attr.to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(&name);
        }
        let mut out = body;
        let offset = out.len() as u32;
        out.extend_from_slice(&central);
        let count = specs.len() as u16;
        out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]
mod tests {
    use super::test_archive::{Spec, blank, directory, file, write_archive};
    use super::{Archive, CENTRAL_SIGNATURE, LOCAL_SIGNATURE, ZIP64_EXTRA_ID, crc32};

    #[test]
    fn round_trips_stored_members_with_directory_entries() {
        let specs = [
            directory("outer"),
            directory("outer/a"),
            file("outer/a/SKILL.md", b"markdown".to_vec()),
        ];
        let archive = Archive::open(write_archive(&specs)).unwrap();
        assert_eq!(archive.members().len(), 3);
        assert_eq!(archive.members()[0].filename, "outer/");
        assert!(archive.members()[0].is_dir);
        assert!(!archive.members()[2].is_dir);
        assert_eq!(archive.read(&archive.members()[2]).unwrap(), b"markdown");
    }

    #[test]
    fn keeps_utf8_names_and_decodes_cp437_without_utf8_flag() {
        let utf8 = Spec {
            name: "caf\u{e9}.txt".to_owned(),
            flag_bits: 0x0800,
            ..blank()
        };
        let high_byte = Spec {
            raw_name: Some(b"caf\xe9.txt".to_vec()),
            ..blank()
        };
        let utf8_bytes_without_flag = Spec {
            raw_name: Some("caf\u{e9}.txt".as_bytes().to_vec()),
            ..blank()
        };
        let archive =
            Archive::open(write_archive(&[utf8, high_byte, utf8_bytes_without_flag])).unwrap();
        assert_eq!(archive.members()[0].filename, "caf\u{e9}.txt");
        assert_eq!(archive.members()[1].filename, "caf\u{398}.txt");
        assert_eq!(archive.members()[2].filename, "caf\u{251c}\u{2310}.txt");
    }

    #[test]
    fn truncates_filenames_at_the_first_nul_byte() {
        let member = file("a/name\u{0}hidden", Vec::new());
        let archive = Archive::open(write_archive(&[member])).unwrap();
        assert_eq!(archive.members()[0].filename, "a/name");
        assert_eq!(archive.members()[0].orig_filename, "a/name\u{0}hidden");
    }

    #[test]
    fn inflates_deflated_payloads() {
        let member = Spec {
            name: "text.txt".to_owned(),
            data: b"hello hello hello".to_vec(),
            method: 8,
            ..blank()
        };
        let archive = Archive::open(write_archive(&[member])).unwrap();
        assert!(archive.members()[0].compressed_size < 17);
        assert_eq!(
            archive.read(&archive.members()[0]).unwrap(),
            b"hello hello hello"
        );
    }

    #[test]
    fn rejects_corrupt_and_truncated_archives() {
        assert!(Archive::open(b"not a zip".to_vec()).is_err());
        assert!(Archive::open(b"PK\x05\x06".to_vec()).is_err());
        let mut truncated = write_archive(&[file("a.txt", b"data".to_vec())]);
        truncated.pop();
        assert!(Archive::open(truncated).is_err());
        let mut bad_crc = write_archive(&[file("a.txt", b"data".to_vec())]);
        let central = bad_crc
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .unwrap();
        bad_crc[central + 16..central + 20].copy_from_slice(&0u32.to_le_bytes());
        let archive = Archive::open(bad_crc).unwrap();
        assert!(archive.read(&archive.members()[0]).is_err());
    }

    #[test]
    fn rejects_encrypted_and_unsupported_method_members() {
        let encrypted = Spec {
            name: "a.txt".to_owned(),
            flag_bits: 1,
            ..blank()
        };
        let archive = Archive::open(write_archive(&[encrypted])).unwrap();
        assert_eq!(
            archive.read(&archive.members()[0]).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        let bzip2 = Spec {
            name: "b.txt".to_owned(),
            data: b"payload".to_vec(),
            method: 12,
            ..blank()
        };
        let archive = Archive::open(write_archive(&[bzip2])).unwrap();
        assert!(archive.read(&archive.members()[0]).is_err());
    }

    #[test]
    fn reads_zip64_sized_members_via_the_central_directory_extra() {
        let mut extra = Vec::new();
        extra.extend_from_slice(&ZIP64_EXTRA_ID.to_le_bytes());
        extra.extend_from_slice(&24u16.to_le_bytes());
        extra.extend_from_slice(&7u64.to_le_bytes());
        extra.extend_from_slice(&7u64.to_le_bytes());
        extra.extend_from_slice(&0u64.to_le_bytes());
        let mut local_extra = Vec::new();
        local_extra.extend_from_slice(&ZIP64_EXTRA_ID.to_le_bytes());
        local_extra.extend_from_slice(&16u16.to_le_bytes());
        local_extra.extend_from_slice(&7u64.to_le_bytes());
        local_extra.extend_from_slice(&7u64.to_le_bytes());
        let crc = crc32(b"payload");
        let mut body = Vec::new();
        body.extend_from_slice(&LOCAL_SIGNATURE.to_le_bytes());
        body.extend_from_slice(&45u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0x21u16.to_le_bytes());
        body.extend_from_slice(&crc.to_le_bytes());
        body.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        body.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        body.extend_from_slice(&3u16.to_le_bytes());
        body.extend_from_slice(&(local_extra.len() as u16).to_le_bytes());
        body.extend_from_slice(b"big");
        body.extend_from_slice(&local_extra);
        body.extend_from_slice(b"payload");
        let mut central = Vec::new();
        central.extend_from_slice(&CENTRAL_SIGNATURE.to_le_bytes());
        central.extend_from_slice(&45u16.to_le_bytes());
        central.extend_from_slice(&45u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0x21u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        central.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        central.extend_from_slice(&3u16.to_le_bytes());
        central.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(b"big");
        central.extend_from_slice(&extra);
        let mut bytes = body;
        let directory_offset = bytes.len() as u64;
        bytes.extend_from_slice(&central);
        let end64_offset = bytes.len() as u64;
        bytes.extend_from_slice(&0x0606_4B50u32.to_le_bytes());
        bytes.extend_from_slice(&44u64.to_le_bytes());
        bytes.extend_from_slice(&45u16.to_le_bytes());
        bytes.extend_from_slice(&45u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&(central.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&directory_offset.to_le_bytes());
        bytes.extend_from_slice(&0x0706_4B50u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&end64_offset.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0xFFFFu16.to_le_bytes());
        bytes.extend_from_slice(&0xFFFFu16.to_le_bytes());
        bytes.extend_from_slice(&(central.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());

        let archive = Archive::open(bytes).unwrap();
        assert_eq!(archive.members().len(), 1);
        assert_eq!(archive.members()[0].compressed_size, 7);
        assert_eq!(archive.read(&archive.members()[0]).unwrap(), b"payload");
    }

    #[test]
    fn detects_local_and_central_name_mismatches() {
        let mut mismatched = write_archive(&[file("a.txt", b"data".to_vec())]);
        let at = mismatched
            .windows(4)
            .position(|window| window == b"PK\x03\x04")
            .unwrap();
        mismatched[at + 30] = b'x';
        let archive = Archive::open(mismatched).unwrap();
        assert_eq!(
            archive.read(&archive.members()[0]).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn rejects_overlapped_entries() {
        let mut bytes = write_archive(&[
            file("a.txt", b"data".to_vec()),
            file("b.txt", b"more".to_vec()),
        ]);
        let central = bytes
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .unwrap();
        bytes[central + 20..central + 24].copy_from_slice(&0x7FFF_u32.to_le_bytes());
        let archive = Archive::open(bytes).unwrap();

        assert_eq!(
            archive.read(&archive.members()[0]).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        assert_eq!(archive.read(&archive.members()[1]).unwrap(), b"more");
    }

    #[test]
    fn rejects_invalid_utf8_names_when_the_utf8_flag_is_set() {
        let invalid = Spec {
            raw_name: Some(vec![b'a', 0xFF]),
            flag_bits: super::UTF8_FLAG,
            ..blank()
        };
        assert!(Archive::open(write_archive(&[invalid])).is_err());
        let unflagged = Spec {
            raw_name: Some(vec![b'a', 0xFF]),
            ..blank()
        };
        let archive = Archive::open(write_archive(&[unflagged])).unwrap();
        assert_eq!(archive.members()[0].filename, "a\u{a0}");
    }
}
