//! Part of a file, provably (`docs/L4s/named-folders-L4.md` §5): a range of its bytes, and the
//! BLAKE3 subtree hashes that tie that range to the whole file's hash.
//!
//! BLAKE3 hashes a file as a tree over 1 KiB chunks. To prove bytes `a..b` belong to a file with a
//! given hash, the range is widened to whole chunks, and every subtree of the file's tree that lies
//! wholly outside it is given by its chaining value (a few dozen, however big the file). Hashing the
//! range's own subtrees and merging them with those gives the root: the file's hash, if the bytes
//! are the file's. So a run that read 100 lines of a 10 GB log is checkable from those lines.
//!
//! Also here: where a file's lines are (`file_lines`), and what lines a span of text holds.

use std::io::{Read, Seek, SeekFrom};

use blake3::hazmat::{ChainingValue, HasherExt, Mode, left_subtree_len, merge_subtrees_non_root, merge_subtrees_root};

/// BLAKE3's chunk: the unit a range is widened to.
pub(crate) const CHUNK: u64 = 1024;

/// What a proof is read in.
const BLOCK: usize = 64 * 1024;

/// `start..end` widened to whole chunks of a file of `size` bytes: the whole of a file of one chunk.
pub(crate) fn widened(start: u64, end: u64, size: u64) -> (u64, u64) {
    if size <= CHUNK {
        return (0, size);
    }
    (start / CHUNK * CHUNK, end.div_ceil(CHUNK).saturating_mul(CHUNK).min(size))
}

/// The hash of the file `f` (`size` bytes), the bytes `a..b` (whole chunks, from [`widened`]), and
/// the chaining values of every subtree outside them, in order: what [`root_of`] checks. Reads the
/// whole file once.
pub(crate) fn prove(f: &mut (impl Read + Seek), size: u64, a: u64, b: u64) -> std::io::Result<(blake3::Hash, Vec<u8>, Vec<ChainingValue>)> {
    let mut bytes = Vec::new();
    let mut proof = Vec::new();
    if size <= CHUNK || (a == 0 && b == size) {
        let mut hasher = blake3::Hasher::new();
        f.seek(SeekFrom::Start(0))?;
        let mut whole = Vec::new();
        f.take(size).read_to_end(&mut whole)?;
        hasher.update(&whole);
        bytes.extend_from_slice(&whole[a as usize..b as usize]);
        return Ok((hasher.finalize(), bytes, proof));
    }
    let left = left_subtree_len(size);
    let l = subtree(f, 0, left, a, b, &mut bytes, &mut proof)?;
    let r = subtree(f, left, size - left, a, b, &mut bytes, &mut proof)?;
    Ok((merge_subtrees_root(&l, &r, Mode::Hash), bytes, proof))
}

/// The chaining value of the subtree `off..off + len` of `f`: its bytes kept in `bytes` where they
/// are in `a..b`, and its own value in `proof` where it is wholly outside.
fn subtree(f: &mut (impl Read + Seek), off: u64, len: u64, a: u64, b: u64, bytes: &mut Vec<u8>, proof: &mut Vec<ChainingValue>) -> std::io::Result<ChainingValue> {
    let inside = off >= a && off + len <= b;
    if inside || off + len <= a || off >= b {
        let mut hasher = blake3::Hasher::new();
        hasher.set_input_offset(off);
        f.seek(SeekFrom::Start(off))?;
        let mut left = len;
        let mut block = vec![0u8; BLOCK];
        while left > 0 {
            let n = left.min(BLOCK as u64) as usize;
            f.read_exact(&mut block[..n])?;
            hasher.update(&block[..n]);
            if inside {
                bytes.extend_from_slice(&block[..n]);
            }
            left -= n as u64;
        }
        let cv = hasher.finalize_non_root();
        if !inside {
            proof.push(cv);
        }
        return Ok(cv);
    }
    let left = left_subtree_len(len);
    let l = subtree(f, off, left, a, b, bytes, proof)?;
    let r = subtree(f, off + left, len - left, a, b, bytes, proof)?;
    Ok(merge_subtrees_non_root(&l, &r, Mode::Hash))
}

/// The hash of a file of `size` bytes whose bytes from `a` are `bytes`, given the chaining values
/// of the subtrees outside them ([`prove`]): none if they don't fit together.
pub(crate) fn root_of(size: u64, a: u64, bytes: &[u8], proof: &[ChainingValue]) -> Option<blake3::Hash> {
    let b = a.checked_add(bytes.len() as u64)?;
    if !a.is_multiple_of(CHUNK) || (!b.is_multiple_of(CHUNK) && b != size) || b > size {
        return None;
    }
    if size <= CHUNK || (a == 0 && b == size) {
        return (a == 0 && b == size && proof.is_empty()).then(|| blake3::hash(bytes));
    }
    let mut given = proof.iter();
    let left = left_subtree_len(size);
    let l = check(0, left, a, bytes, &mut given)?;
    let r = check(left, size - left, a, bytes, &mut given)?;
    given.next().is_none().then(|| merge_subtrees_root(&l, &r, Mode::Hash))
}

fn check<'p>(off: u64, len: u64, a: u64, bytes: &[u8], given: &mut impl Iterator<Item = &'p ChainingValue>) -> Option<ChainingValue> {
    let b = a + bytes.len() as u64;
    if off >= a && off + len <= b {
        let mut hasher = blake3::Hasher::new();
        hasher.set_input_offset(off);
        hasher.update(&bytes[(off - a) as usize..(off + len - a) as usize]);
        return Some(hasher.finalize_non_root());
    }
    if off + len <= a || off >= b {
        return given.next().copied();
    }
    if len <= CHUNK {
        return None;
    }
    let left = left_subtree_len(len);
    let l = check(off, left, a, bytes, given)?;
    let r = check(off + left, len - left, a, bytes, given)?;
    Some(merge_subtrees_non_root(&l, &r, Mode::Hash))
}

/// Where `limit` lines from line `from` are in `f` (`size` bytes): `start..end`, the last one's
/// line ending included. `from` counts from 0, or from the end when negative (-1: the last line).
/// A file's last line needs no line ending; past the end is an empty span at the end.
pub(crate) fn line_span(f: &mut (impl Read + Seek), size: u64, from: i64, limit: u64) -> std::io::Result<(u64, u64)> {
    let start = if from >= 0 { after_newlines(f, 0, size, from as u64)? } else { start_from_end(f, size, from.unsigned_abs())? };
    let end = after_newlines(f, start, size, limit)?;
    Ok((start, end))
}

/// Where the text from `at` is once `n` line endings are passed: the end, if fewer.
fn after_newlines(f: &mut (impl Read + Seek), at: u64, size: u64, mut n: u64) -> std::io::Result<u64> {
    if n == 0 {
        return Ok(at);
    }
    f.seek(SeekFrom::Start(at))?;
    let mut pos = at;
    let mut block = vec![0u8; BLOCK];
    while pos < size {
        let got = f.read(&mut block[..(size - pos).min(BLOCK as u64) as usize])?;
        if got == 0 {
            break;
        }
        for (i, b) in block[..got].iter().enumerate() {
            if *b == b'\n' {
                n -= 1;
                if n == 0 {
                    return Ok(pos + i as u64 + 1);
                }
            }
        }
        pos += got as u64;
    }
    Ok(size)
}

/// Where the `m`th line from the end starts (the first line's start, if there are fewer).
fn start_from_end(f: &mut (impl Read + Seek), size: u64, m: u64) -> std::io::Result<u64> {
    // A line ending at the very end closes the last line; it starts none.
    let mut end = size;
    if size > 0 {
        let mut last = [0u8];
        f.seek(SeekFrom::Start(size - 1))?;
        f.read_exact(&mut last)?;
        if last[0] == b'\n' {
            end -= 1;
        }
    }
    let mut seen = 0;
    let mut block = vec![0u8; BLOCK];
    while end > 0 {
        let n = end.min(BLOCK as u64);
        f.seek(SeekFrom::Start(end - n))?;
        f.read_exact(&mut block[..n as usize])?;
        for i in (0..n as usize).rev() {
            if block[i] == b'\n' {
                seen += 1;
                if seen == m {
                    return Ok(end - n + i as u64 + 1);
                }
            }
        }
        end -= n;
    }
    Ok(0)
}

/// The lines of a span of text, without their line endings (`\n`, or `\r\n`).
pub(crate) fn lines_of(text: &str) -> Vec<String> {
    text.split_inclusive('\n').map(|l| l.strip_suffix('\n').map_or(l, |l| l.strip_suffix('\r').unwrap_or(l)).to_string()).collect()
}

/// Where `length` bytes from `offset` are in a file of `size` bytes (from the end when `offset` is
/// negative), within it.
pub(crate) fn slice_span(size: u64, offset: i64, length: u64) -> (u64, u64) {
    let start = if offset >= 0 { (offset as u64).min(size) } else { size.saturating_sub(offset.unsigned_abs()) };
    (start, start.saturating_add(length).min(size))
}

/// Bytes as lowercase hex, as the log writes them.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The bytes hex text names: none if it isn't hex.
pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 % 251) as u8).collect()
    }

    #[test]
    fn a_range_proves_itself_part_of_the_file_and_nothing_else_does() {
        for size in [0usize, 1, 1000, 1024, 1025, 3000, 4096, 10_000, 65_536 + 17, 300_000] {
            let bytes = data(size);
            let hash = blake3::hash(&bytes);
            for (s, e) in [(0, size), (0, 1.min(size)), (size / 3, size / 2), (size.saturating_sub(5), size), (size / 2, size / 2)] {
                let (a, b) = widened(s as u64, e as u64, size as u64);
                let (root, kept, proof) = prove(&mut Cursor::new(&bytes), size as u64, a, b).unwrap();
                assert_eq!(root, hash, "{size} {s}..{e}");
                assert_eq!(kept, &bytes[a as usize..b as usize]);
                assert!(proof.len() <= 2 * 64, "a few hashes, not the file");
                assert_eq!(root_of(size as u64, a, &kept, &proof), Some(hash), "{size} {s}..{e}");
                if !kept.is_empty() {
                    let mut changed = kept.clone();
                    changed[0] ^= 1;
                    assert_ne!(root_of(size as u64, a, &changed, &proof), Some(hash), "a changed byte doesn't fit");
                }
                if a >= CHUNK {
                    assert_ne!(root_of(size as u64, a - CHUNK, &kept, &proof), Some(hash), "nor bytes said to be elsewhere");
                }
            }
        }
    }

    #[test]
    fn lines_are_found_from_the_start_and_from_the_end() {
        let text = "a\nbb\r\nccc\ndddd";
        let size = text.len() as u64;
        let span = |from, limit| {
            let (s, e) = line_span(&mut Cursor::new(text.as_bytes()), size, from, limit).unwrap();
            lines_of(&text[s as usize..e as usize])
        };
        assert_eq!(span(0, 2), ["a", "bb"]);
        assert_eq!(span(1, 100), ["bb", "ccc", "dddd"]);
        assert_eq!(span(-1, 5), ["dddd"]);
        assert_eq!(span(-2, 1), ["ccc"]);
        assert_eq!(span(-10, 2), ["a", "bb"], "fewer lines than asked: from the first");
        assert_eq!(span(9, 2), Vec::<String>::new(), "past the end");
        let closed = "x\ny\n";
        let (s, e) = line_span(&mut Cursor::new(closed.as_bytes()), 4, -1, 1).unwrap();
        assert_eq!(&closed[s as usize..e as usize], "y\n", "a final line ending starts no line");
        assert_eq!(slice_span(10, -3, 100), (7, 10));
        assert_eq!(slice_span(10, 20, 5), (10, 10));
    }
}
