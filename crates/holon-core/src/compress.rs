//! Compression (host-capabilities L4 §5.1.7): gzip and zip, both ways, the same bytes on every
//! platform. Deflating and inflating are `miniz_oxide`'s; the two containers and CRC-32 are
//! written here. Nothing reads the clock: a gzip's time is 0 and a zip's entries are dated
//! 1980-01-01. Inflating is bounded, as core can't see the run's limits: past `MAX_INFLATED` bytes
//! or `MAX_FILES` files, nothing.

use miniz_oxide::inflate::stream::{InflateState, inflate};
use miniz_oxide::{DataFormat, MZFlush, MZStatus};
use std::collections::BTreeMap;

/// The most a `gunzip` or an `unzip` inflates, its files together: a 10 MiB file that would
/// inflate to 10 GiB stops here.
pub(crate) const MAX_INFLATED: usize = 64 << 20;

/// The most files a zip holds, written or read.
pub(crate) const MAX_FILES: usize = 10_000;

/// Deflate's level: zlib's default.
const LEVEL: u8 = 6;

/// 1980-01-01 at midnight, as a zip writes a date and a time (MS-DOS's): the first it can hold.
const DOS_DATE: u16 = 1 << 5 | 1;
const DOS_TIME: u16 = 0;

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;

/// A zip's flag for a name written in UTF-8; without it, a name is in the reader's code page.
const UTF8_NAME: u16 = 1 << 11;

/// CRC-32 (IEEE 802.3, reflected), the checksum gzip and zip both carry.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |c, &b| CRC_TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8))
}

/// The raw deflate stream at the start of `input`, inflated, and how many bytes of `input` it
/// took; nothing for a stream that is broken, cut short, or would pass what is left of `budget`,
/// which it spends.
fn inflated(input: &[u8], budget: &mut usize) -> Option<(Vec<u8>, usize)> {
    let mut state = InflateState::new_boxed(DataFormat::Raw);
    let mut out = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    let mut at = 0;
    loop {
        let step = inflate(&mut state, &input[at..], &mut chunk, MZFlush::None);
        at += step.bytes_consumed;
        *budget = budget.checked_sub(step.bytes_written)?;
        out.extend_from_slice(&chunk[..step.bytes_written]);
        match step.status {
            Ok(MZStatus::StreamEnd) => return Some((out, at)),
            Ok(MZStatus::Ok) if step.bytes_consumed > 0 || step.bytes_written > 0 => {}
            _ => return None,
        }
    }
}

/// A number a zip's field holds in 32 bits, short of the 64-bit form's marker.
fn u32_of(n: usize) -> Option<u32> {
    u32::try_from(n).ok().filter(|n| *n != u32::MAX)
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// `data` gzipped: one member, deflated at level 6, with no name, time 0 and system 255 (unknown),
/// so the same data gives the same bytes wherever it is gzipped.
pub(crate) fn gzip(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
    out.extend(miniz_oxide::deflate::compress_to_vec(data, LEVEL));
    out.extend(crc32(data).to_le_bytes());
    out.extend((data.len() as u32).to_le_bytes());
    out
}

/// The bytes inside a gzip, each member's joined, as `gunzip` does; nothing for a header, a
/// checksum or a length that is wrong, bytes after the last member that aren't another, or past
/// `MAX_INFLATED`.
pub(crate) fn gunzip(bytes: &[u8]) -> Option<Vec<u8>> {
    gunzip_within(bytes, MAX_INFLATED)
}

fn gunzip_within(bytes: &[u8], mut budget: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut rest = bytes;
    loop {
        let body = member_header(rest)?;
        let (data, used) = inflated(&rest[body..], &mut budget)?;
        let end = body + used;
        if u32_at(rest, end)? != crc32(&data) || u32_at(rest, end + 4)? != data.len() as u32 {
            return None;
        }
        out.extend(data);
        rest = &rest[end + 8..];
        if rest.is_empty() {
            return Some(out);
        }
    }
}

/// Where a gzip member's deflate stream starts, past its header (RFC 1952 §2.3): its extra field,
/// name and comment when its flags say so, and the header's own checksum, checked; nothing for
/// what isn't one, or a flag RFC 1952 reserves.
fn member_header(b: &[u8]) -> Option<usize> {
    let &[0x1f, 0x8b, 8, flags, _, _, _, _, _, _, ..] = b else { return None };
    if flags & 0xe0 != 0 {
        return None;
    }
    let mut at = 10;
    if flags & 4 != 0 {
        at += 2 + usize::from(u16_at(b, at)?);
    }
    // The name, then the comment: each ends with a zero byte.
    for flag in [8, 16] {
        if flags & flag != 0 {
            at += b.get(at..)?.iter().position(|&c| c == 0)? + 1;
        }
    }
    if flags & 2 != 0 {
        if u16_at(b, at)? != crc32(b.get(..at)?) as u16 {
            return None;
        }
        at += 2;
    }
    (at <= b.len()).then_some(at)
}

/// Whether a file's name in a zip lands inside the folder it is unzipped into: not empty, not a
/// directory's, not starting with `/`, without a `..` part or a `\`.
fn lands_inside(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('/') && !name.ends_with('/') && !name.contains('\\') && !name.split('/').any(|part| part == "..")
}

/// A zip of `files`, each deflated, in the order given, dated 1980-01-01; nothing for a name
/// [`lands_inside`] refuses, more than `MAX_FILES` files, or more than a zip without its 64-bit
/// form holds.
pub(crate) fn zip<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut directory = Vec::new();
    let mut count = 0usize;
    for (name, data) in files {
        count += 1;
        if !lands_inside(name) || count > MAX_FILES {
            return None;
        }
        let deflated = miniz_oxide::deflate::compress_to_vec(data, LEVEL);
        let offset = u32_of(out.len())?;
        let flags = if name.is_ascii() { 0 } else { UTF8_NAME };
        // What the file's local header and its entry in the directory both hold, from the version
        // needed to read it (2.0, for deflate) to the length of its extra field (none).
        let mut common = Vec::with_capacity(26);
        for half in [20, flags, 8, DOS_TIME, DOS_DATE] {
            common.extend(half.to_le_bytes());
        }
        for word in [crc32(data), u32_of(deflated.len())?, u32_of(data.len())?] {
            common.extend(word.to_le_bytes());
        }
        common.extend(u16::try_from(name.len()).ok()?.to_le_bytes());
        common.extend(0u16.to_le_bytes());

        out.extend(LOCAL.to_le_bytes());
        out.extend(&common);
        out.extend(name.as_bytes());
        out.extend(&deflated);

        directory.extend(CENTRAL.to_le_bytes());
        // Made by MS-DOS's rules (no permissions), version 2.0.
        directory.extend(20u16.to_le_bytes());
        directory.extend(&common);
        // No comment, the first disk, no attributes; then where its local header is.
        directory.extend([0u8; 10]);
        directory.extend(offset.to_le_bytes());
        directory.extend(name.as_bytes());
    }
    let (at, size, count) = (u32_of(out.len())?, u32_of(directory.len())?, u16::try_from(count).ok()?);
    out.extend(directory);
    out.extend(END.to_le_bytes());
    // This disk, and the one the directory starts on: the first.
    out.extend([0u8; 4]);
    for half in [count, count] {
        out.extend(half.to_le_bytes());
    }
    for word in [size, at] {
        out.extend(word.to_le_bytes());
    }
    out.extend(0u16.to_le_bytes());
    Some(out)
}

/// A zip's files, name to bytes, its directories left out; nothing for what isn't a zip, an entry
/// encrypted or compressed other than stored or deflated, a checksum or a size that doesn't match,
/// two entries of one name, the 64-bit form, more than `MAX_FILES` files, or past `MAX_INFLATED`.
/// A name without the UTF-8 flag that isn't UTF-8 is read as Latin-1, as fflate reads one.
pub(crate) fn unzip(bytes: &[u8]) -> Option<BTreeMap<String, Vec<u8>>> {
    unzip_within(bytes, MAX_INFLATED)
}

fn unzip_within(b: &[u8], mut budget: usize) -> Option<BTreeMap<String, Vec<u8>>> {
    let end = end_record(b)?;
    let (disk, first, here, count) = (u16_at(b, end + 4)?, u16_at(b, end + 6)?, u16_at(b, end + 8)?, u16_at(b, end + 10)?);
    let offset = u32_at(b, end + 16)?;
    // One disk, and no 64-bit form, whose markers are all ones.
    if disk != 0 || first != 0 || here != count || count == u16::MAX || offset == u32::MAX || usize::from(count) > MAX_FILES {
        return None;
    }
    let mut files = BTreeMap::new();
    let mut at = offset as usize;
    for _ in 0..count {
        if u32_at(b, at)? != CENTRAL {
            return None;
        }
        let (flags, method, crc) = (u16_at(b, at + 8)?, u16_at(b, at + 10)?, u32_at(b, at + 16)?);
        let (packed, size, local) = (u32_at(b, at + 20)?, u32_at(b, at + 24)?, u32_at(b, at + 42)?);
        let (named, extra, comment) = (usize::from(u16_at(b, at + 28)?), usize::from(u16_at(b, at + 30)?), usize::from(u16_at(b, at + 32)?));
        let raw = b.get(at + 46..at + 46 + named)?;
        at += 46 + named + extra + comment;
        // Encrypted (strong encryption sets this bit too), or the 64-bit form.
        if flags & 1 != 0 || [packed, size, local].contains(&u32::MAX) {
            return None;
        }
        let name = match std::str::from_utf8(raw) {
            Ok(name) => name.to_string(),
            Err(_) if flags & UTF8_NAME == 0 => raw.iter().map(|&c| char::from(c)).collect(),
            Err(_) => return None,
        };
        if name.ends_with('/') {
            continue;
        }
        // The local header has its own name and extra field; the data follows them.
        let local = local as usize;
        if u32_at(b, local)? != LOCAL {
            return None;
        }
        let start = local + 30 + usize::from(u16_at(b, local + 26)?) + usize::from(u16_at(b, local + 28)?);
        let stored = b.get(start..start.checked_add(packed as usize)?)?;
        let data = match method {
            0 => {
                budget = budget.checked_sub(stored.len())?;
                stored.to_vec()
            }
            8 => inflated(stored, &mut budget)?.0,
            _ => return None,
        };
        if data.len() != size as usize || crc32(&data) != crc || files.insert(name, data).is_some() {
            return None;
        }
    }
    Some(files)
}

/// Where a zip's end record starts: the last 22 bytes, or before a comment of up to 65,535 that
/// runs to the end.
fn end_record(b: &[u8]) -> Option<usize> {
    let last = b.len().checked_sub(22)?;
    (last.saturating_sub(0xffff)..=last).rev().find(|&at| u32_at(b, at) == Some(END) && u16_at(b, at + 20).is_some_and(|c| at + 22 + usize::from(c) == b.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// Python's gzip: a member with a file name and a time, then a second member.
    const PYTHON_GZIP: &str = "1f8b080800f1536502ff612e74787400cb48cdc9c957c840905c003b7c8adf120000001f8b08000000000002ff2bcf2fca49e10200a86138dd06000000";

    /// Python's zipfile: a stored file, a directory, a deflated file in it, a name in UTF-8.
    const PYTHON_ZIP: &str = "504b0304140000000000831822501a10845e05000000050000000a00000073746f7265642e7478746173206973504b030414000000000083182250000000000000000000000000040000006469722f504b030414000000080083182250fe9505bc0c0000006b000000100000006469722f6465666c617465642e786d6cb349b4aba003b0d14fb40300504b0304140000080800831822507170d3fb090000000700000009000000c3bc6265722e747874732f3abce7f0fc5400504b01021400140000000000831822501a10845e05000000050000000a000000000000000000000080010000000073746f7265642e747874504b010214001400000000008318225000000000000000000000000004000000000000000000000080012d0000006469722f504b0102140014000000080083182250fe9505bc0c0000006b00000010000000000000000000000080014f0000006469722f6465666c617465642e786d6c504b01021400140000080800831822507170d3fb0900000007000000090000000000000000000000800189000000c3bc6265722e747874504b05060000000004000400df000000b90000000000";

    #[test]
    fn crc32_is_ieees() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn what_is_gzipped_gunzips_back() {
        let long: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
        for data in [&b""[..], b"hello", "Grüße".as_bytes(), &long] {
            let gz = gzip(data);
            assert_eq!(gz[..10], [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255], "no name, time 0, system unknown");
            assert_eq!(gunzip(&gz).as_deref(), Some(data));
        }
        assert!(gzip(&long).len() < long.len());
    }

    #[test]
    fn pythons_gzip_reads_with_its_name_and_both_members() {
        assert_eq!(gunzip(&hex(PYTHON_GZIP)).as_deref(), Some(&b"hello hello hello\nworld\n"[..]));
    }

    #[test]
    fn what_isnt_a_whole_gzip_is_nothing() {
        let gz = gzip(b"hello hello hello");
        let n = gz.len();
        let changed = |at: usize, to: u8| {
            let mut g = gz.clone();
            g[at] = to;
            g
        };
        for (bad, why) in [
            (Vec::new(), "nothing at all"),
            (b"hello".to_vec(), "not gzip"),
            (gz[..n - 1].to_vec(), "its length cut short"),
            (gz[..12].to_vec(), "its stream cut short"),
            (changed(n - 8, gz[n - 8] ^ 1), "a checksum that doesn't match"),
            (changed(n - 1, gz[n - 1] ^ 1), "a length that doesn't match"),
            (changed(3, 0x20), "a reserved flag"),
            ([gz.clone(), b"junk".to_vec()].concat(), "bytes after it that aren't a member"),
        ] {
            assert_eq!(gunzip(&bad), None, "{why}");
        }
        // A header's own checksum (FHCRC) is checked.
        let mut header = vec![0x1f, 0x8b, 8, 2, 0, 0, 0, 0, 0, 255];
        let crc = crc32(&header) as u16;
        let body = &gz[10..];
        header.extend(crc.to_le_bytes());
        assert_eq!(gunzip(&[header.clone(), body.to_vec()].concat()).as_deref(), Some(&b"hello hello hello"[..]));
        header[10] ^= 1;
        assert_eq!(gunzip(&[header, body.to_vec()].concat()), None);
    }

    #[test]
    fn inflating_stops_at_its_budget() {
        let zeros = gzip(&vec![0u8; 100_000]);
        assert_eq!(gunzip_within(&zeros, 100_000).map(|b| b.len()), Some(100_000));
        assert_eq!(gunzip_within(&zeros, 99_999), None);
        let two = zip([("a", &[0u8; 600][..]), ("b", &[0u8; 600][..])]).unwrap();
        assert!(unzip_within(&two, 1200).is_some());
        assert_eq!(unzip_within(&two, 1199), None, "the files together");
        assert_eq!(MAX_INFLATED, 64 << 20);
    }

    #[test]
    fn what_is_zipped_unzips_back_in_order() {
        let files = [("a.xml", &b"<a/>"[..]), ("b/c.csv", b"x,y\n1,2\n"), ("empty", b""), ("\u{fc}ber.txt", "Grüße".as_bytes())];
        let z = zip(files).unwrap();
        let back = unzip(&z).unwrap();
        assert_eq!(back.len(), 4);
        for (name, data) in files {
            assert_eq!(back[name], data, "{name}");
        }
        assert_eq!(zip(files), Some(z.clone()), "the same bytes again");
        assert_eq!(z[..4], LOCAL.to_le_bytes());
        assert_eq!(z[10..14], [0, 0, 0x21, 0], "dated 1980-01-01, midnight");
        assert_eq!(unzip(&zip([]).unwrap()), Some(BTreeMap::new()), "an empty zip");
    }

    #[test]
    fn pythons_zip_reads_without_its_directory() {
        let files = unzip(&hex(PYTHON_ZIP)).unwrap();
        let names: Vec<&str> = files.keys().map(String::as_str).collect();
        assert_eq!(names, ["dir/deflated.xml", "stored.txt", "\u{fc}ber.txt"]);
        assert_eq!(files["stored.txt"], b"as is");
        assert_eq!(files["dir/deflated.xml"], [&b"<a>"[..], &[b'x'; 100], b"</a>"].concat());
        assert_eq!(files["\u{fc}ber.txt"], "Grüße".as_bytes());
    }

    #[test]
    fn what_isnt_a_whole_zip_is_nothing() {
        let z = hex(PYTHON_ZIP);
        let central = z.windows(4).position(|w| w == CENTRAL.to_le_bytes()).unwrap();
        let changed = |at: usize, to: u8| {
            let mut c = z.clone();
            c[at] = to;
            c
        };
        let data = z.windows(5).position(|w| w == b"as is").unwrap();
        for (bad, why) in [
            (Vec::new(), "nothing at all"),
            (b"PK not a zip".to_vec(), "no end record"),
            (z[..z.len() - 1].to_vec(), "its end record cut short"),
            (z[..central].to_vec(), "no directory"),
            (changed(data, b'A'), "a checksum that doesn't match"),
            (changed(central + 8, 1), "encrypted"),
            (changed(central + 10, 12), "compressed by bzip2"),
            (changed(central + 24, 9), "a size that doesn't match"),
        ] {
            assert_eq!(unzip(&bad), None, "{why}");
        }
        // Two entries of one name: the second's directory entry pointing at the first's data.
        let one = zip([("a", &b"x"[..])]).unwrap();
        let dir = one.windows(4).position(|w| w == CENTRAL.to_le_bytes()).unwrap();
        let entry = &one[dir..one.len() - 22];
        let mut end = one[one.len() - 22..].to_vec();
        end[8] = 2;
        end[10] = 2;
        end[12..16].copy_from_slice(&((entry.len() * 2) as u32).to_le_bytes());
        let twice = [&one[..dir], entry, entry, &end].concat();
        assert_eq!(unzip(&twice), None, "two entries of one name");
    }

    #[test]
    fn a_name_that_would_land_outside_its_folder_isnt_zipped() {
        for bad in ["", "/etc/passwd", "a/../../b", "..", "a\\b", "dir/"] {
            assert_eq!(zip([(bad, &b"x"[..])]), None, "{bad:?}");
        }
        for ok in ["a", "a/b/c.txt", "..a", "a..b", "über/x"] {
            assert!(zip([(ok, &b"x"[..])]).is_some(), "{ok:?}");
        }
        let many: Vec<String> = (0..=MAX_FILES).map(|i| format!("f{i}")).collect();
        assert_eq!(zip(many.iter().map(|n| (n.as_str(), &b""[..]))), None, "more than 10,000 files");
    }

    /// What gzip and zip write is pinned: a logged run must give the same bytes when verify runs
    /// it again, so a new `miniz_oxide` that deflates differently fails here first.
    #[test]
    fn the_bytes_written_are_pinned() {
        let data = "id,name,iban,amount\nP1,Jane Roe,FR1420041010050500013M02606,100.00\n".repeat(20);
        let hash = |b: &[u8]| crate::crypto::sha256(b, "hex").unwrap();
        assert_eq!(hash(&gzip(data.as_bytes())), "22e79a75a476b9a4406a7899c4722626e2017fbe2ffbf16dd8223585dcb1eae8");
        assert_eq!(hash(&zip([("payouts.csv", data.as_bytes())]).unwrap()), "83555fe493d577f72804eca25187537290c18814311c7d44c37c33fb409b58ee");
    }
}
