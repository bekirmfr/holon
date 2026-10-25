//! The texts of files runs read, kept for their log (`docs/L4s/named-folders-L4.md` §5): what
//! `holon verify` runs a run again from when its log entry holds only a reference to one.
//!
//! - **By content.** A text is filed under the BLAKE3 of its bytes (hex), so the same file read
//!   by 500 runs, or by two deployments, is kept once.
//! - **In chunks.** A text is cut where its content says (content-defined chunking: a rolling
//!   gear hash over the bytes picks the cuts, 16 KiB to 256 KiB apart, 64 KiB on average), and
//!   each chunk is kept by its own hash. A file that grows, or changes in one place, adds only the
//!   chunks around the change: the cuts before and after it fall where they fell.
//! - **Swept.** A text no kept run refers to is removed, and with it every chunk no text kept uses
//!   ([`Blobs::sweep`]; folders.rs says which runs still refer to one). Its log entries keep its
//!   hash: the run stays provable, not re-runnable.
//!
//! On disk, under the content store (`<store>/blobs/`): a text is a small file naming its chunks,
//! and each chunk a file under `chunks/`, named by its hash. Every read is checked: each chunk
//! against its name, the text against its own.

use std::sync::Mutex;

use crate::*;

/// The fewest bytes between two cuts: a text this size or smaller is one chunk.
pub(crate) const CHUNK_MIN: usize = 16 * 1024;
/// Where cuts get likelier: they fall about this far apart.
pub(crate) const CHUNK_AVG: usize = 64 * 1024;
/// The most bytes between two cuts.
pub(crate) const CHUNK_MAX: usize = 256 * 1024;

/// Before the average, a cut needs these 18 top bits of the hash clear; after it, only 14: cuts
/// cluster around the average (FastCDC's normalized chunking).
const MASK_HARD: u64 = !0 << (64 - 18);
const MASK_EASY: u64 = !0 << (64 - 14);

/// A fixed random number per byte value: the gear the rolling hash turns by (splitmix64 from a
/// fixed seed, so every host cuts the same text at the same places).
const GEAR: [u64; 256] = gear();

const fn gear() -> [u64; 256] {
    let mut table = [0u64; 256];
    let mut seed: u64 = 0x686f_6c6f_6e00_0001;
    let mut i = 0;
    while i < 256 {
        seed = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        table[i] = z ^ (z >> 31);
        i += 1;
    }
    table
}

/// How long the first chunk of `bytes` is.
fn cut(bytes: &[u8]) -> usize {
    if bytes.len() <= CHUNK_MIN {
        return bytes.len();
    }
    let (normal, max) = (bytes.len().min(CHUNK_AVG), bytes.len().min(CHUNK_MAX));
    let mut hash = 0u64;
    for (i, &b) in bytes.iter().enumerate().take(max).skip(CHUNK_MIN) {
        hash = (hash << 1).wrapping_add(GEAR[b as usize]);
        if hash & if i < normal { MASK_HARD } else { MASK_EASY } == 0 {
            return i + 1;
        }
    }
    max
}

/// `bytes` cut into its chunks, in order.
pub(crate) fn chunks(mut bytes: &[u8]) -> Vec<&[u8]> {
    let mut all = Vec::new();
    while !bytes.is_empty() {
        let (chunk, rest) = bytes.split_at(cut(bytes));
        all.push(chunk);
        bytes = rest;
    }
    all
}

/// Whether `s` can name a text or a chunk: 64 lowercase hex digits, and so nothing else on disk.
fn is_id(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// What names a text: its size, and its chunks' hashes in order.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Manifest {
    size: u64,
    chunks: Vec<String>,
}

/// What a sweep removed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Swept {
    pub(crate) texts: usize,
    pub(crate) chunks: usize,
    /// The chunks' bytes, freed.
    pub(crate) bytes: u64,
}

/// What the store holds: how many texts, how many chunks, and the chunks' bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Held {
    pub(crate) texts: usize,
    pub(crate) chunks: usize,
    pub(crate) bytes: u64,
}

/// In memory: each text and chunk, with when it was last put.
#[derive(Default)]
struct Mem {
    texts: BTreeMap<String, (Manifest, u64)>,
    chunks: BTreeMap<String, (Vec<u8>, u64)>,
}

/// The texts runs read, kept by content, in chunks.
pub(crate) struct Blobs {
    /// Where they are kept, or none (in memory: the tests, an in-memory store).
    dir: Option<PathBuf>,
    mem: Mutex<Mem>,
    /// Held by a put and by a sweep: a sweep never removes a chunk a text being put is about to
    /// name.
    busy: Mutex<()>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// When the file at `path` was last changed, in milliseconds since 1970.
fn changed_ms(path: &Path) -> u64 {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64)
}

impl Blobs {
    pub(crate) fn memory() -> Blobs {
        Blobs { dir: None, mem: Mutex::new(Mem::default()), busy: Mutex::new(()) }
    }

    /// Kept in `dir` (made when the first text is put).
    pub(crate) fn at(dir: PathBuf) -> Blobs {
        Blobs { dir: Some(dir), ..Blobs::memory() }
    }

    /// Keep `bytes`: what names them (the BLAKE3 of the bytes, hex). Keeping them again only
    /// marks them as just put, so a sweep spares them as it spares a new text.
    pub(crate) fn put(&self, bytes: &[u8]) -> Result<String, String> {
        let id = blake3::hash(bytes).to_hex().to_string();
        let _busy = self.busy.lock().unwrap();
        let now = now_ms();
        let Some(dir) = &self.dir else {
            let mut mem = self.mem.lock().unwrap();
            let mut names = Vec::new();
            for chunk in chunks(bytes) {
                let name = blake3::hash(chunk).to_hex().to_string();
                mem.chunks.entry(name.clone()).or_insert_with(|| (chunk.to_vec(), now)).1 = now;
                names.push(name);
            }
            mem.texts.insert(id.clone(), (Manifest { size: bytes.len() as u64, chunks: names }, now));
            return Ok(id);
        };
        let text = dir.join(&id);
        if text.is_file() {
            std::fs::File::options()
                .write(true)
                .open(&text)
                .and_then(|f| f.set_modified(SystemTime::now()))
                .map_err(|e| format!("can't keep a text in {}: {e}", dir.display()))?;
            return Ok(id);
        }
        let dir_of_chunks = dir.join("chunks");
        std::fs::create_dir_all(&dir_of_chunks).map_err(|e| format!("can't keep texts in {}: {e}", dir.display()))?;
        let mut names = Vec::new();
        for chunk in chunks(bytes) {
            let name = blake3::hash(chunk).to_hex().to_string();
            let path = dir_of_chunks.join(&name);
            if !path.is_file() {
                holon_host::write_atomically(&path, chunk).map_err(|e| format!("can't keep a chunk in {}: {e}", dir.display()))?;
            }
            names.push(name);
        }
        let manifest = serde_json::to_vec(&Manifest { size: bytes.len() as u64, chunks: names }).map_err(|e| e.to_string())?;
        holon_host::write_atomically(&text, &manifest).map_err(|e| format!("can't keep a text in {}: {e}", dir.display()))?;
        Ok(id)
    }

    /// The bytes kept as `id`, checked against it: none if they aren't kept (never were, or were
    /// swept); an error if a chunk is missing or isn't what its name says.
    pub(crate) fn get(&self, id: &str) -> Result<Option<Vec<u8>>, String> {
        if !is_id(id) {
            return Ok(None);
        }
        let (manifest, parts) = match &self.dir {
            None => {
                let mem = self.mem.lock().unwrap();
                let Some((manifest, _)) = mem.texts.get(id) else { return Ok(None) };
                let parts: Vec<Option<Vec<u8>>> = manifest.chunks.iter().map(|c| mem.chunks.get(c).map(|(b, _)| b.clone())).collect();
                (manifest.clone(), parts)
            }
            Some(dir) => {
                let manifest: Manifest = match std::fs::read(dir.join(id)) {
                    Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("the kept text {id} is damaged: {e}"))?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(e) => return Err(format!("can't read the kept text {id}: {e}")),
                };
                let parts = manifest.chunks.iter().map(|c| if is_id(c) { std::fs::read(dir.join("chunks").join(c)).ok() } else { None }).collect();
                (manifest, parts)
            }
        };
        let mut bytes = Vec::with_capacity(manifest.size as usize);
        for (name, part) in manifest.chunks.iter().zip(parts) {
            let part = part.ok_or_else(|| format!("the kept text {id} is missing its chunk {name}"))?;
            if blake3::hash(&part).to_hex().as_str() != name {
                return Err(format!("the kept text {id} has a damaged chunk {name}"));
            }
            bytes.extend_from_slice(&part);
        }
        if blake3::hash(&bytes).to_hex().as_str() != id {
            return Err(format!("the kept text {id} isn't what its name says"));
        }
        Ok(Some(bytes))
    }

    /// Remove every text not in `live` put before `before_ms`, then every chunk put before it that
    /// no text left names. A text or chunk put since is spared: a run that just kept one may not
    /// have logged it yet.
    pub(crate) fn sweep(&self, live: &BTreeSet<String>, before_ms: u64) -> Result<Swept, String> {
        let _busy = self.busy.lock().unwrap();
        let mut swept = Swept::default();
        let Some(dir) = &self.dir else {
            let mut mem = self.mem.lock().unwrap();
            let before = mem.texts.len();
            mem.texts.retain(|id, (_, at)| live.contains(id) || *at >= before_ms);
            swept.texts = before - mem.texts.len();
            let used: BTreeSet<String> = mem.texts.values().flat_map(|(m, _)| m.chunks.clone()).collect();
            let gone: Vec<String> = mem.chunks.iter().filter(|(c, (_, at))| !used.contains(*c) && *at < before_ms).map(|(c, _)| c.clone()).collect();
            for c in gone {
                if let Some((bytes, _)) = mem.chunks.remove(&c) {
                    swept.chunks += 1;
                    swept.bytes += bytes.len() as u64;
                }
            }
            return Ok(swept);
        };
        let Ok(entries) = std::fs::read_dir(dir) else { return Ok(swept) };
        let mut used = BTreeSet::new();
        for entry in entries.flatten() {
            let Some(id) = entry.file_name().to_str().filter(|n| is_id(n)).map(str::to_string) else { continue };
            let path = entry.path();
            if !live.contains(&id) && changed_ms(&path) < before_ms {
                std::fs::remove_file(&path).map_err(|e| format!("can't sweep the kept text {id}: {e}"))?;
                swept.texts += 1;
                continue;
            }
            // A text that can't be read names chunks no one can tell: keep them all.
            match std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Manifest>(&b).ok()) {
                Some(m) => used.extend(m.chunks),
                None => return Ok(swept),
            }
        }
        let Ok(entries) = std::fs::read_dir(dir.join("chunks")) else { return Ok(swept) };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().filter(|n| is_id(n)).map(str::to_string) else { continue };
            let path = entry.path();
            if used.contains(&name) || changed_ms(&path) >= before_ms {
                continue;
            }
            let size = entry.metadata().map_or(0, |m| m.len());
            std::fs::remove_file(&path).map_err(|e| format!("can't sweep the chunk {name}: {e}"))?;
            swept.chunks += 1;
            swept.bytes += size;
        }
        Ok(swept)
    }

    /// What it holds now.
    pub(crate) fn held(&self) -> Held {
        let Some(dir) = &self.dir else {
            let mem = self.mem.lock().unwrap();
            return Held { texts: mem.texts.len(), chunks: mem.chunks.len(), bytes: mem.chunks.values().map(|(b, _)| b.len() as u64).sum() };
        };
        let ids = |d: &Path| -> Vec<std::fs::DirEntry> {
            std::fs::read_dir(d).map(|e| e.flatten().filter(|e| e.file_name().to_str().is_some_and(is_id)).collect()).unwrap_or_default()
        };
        let chunks = ids(&dir.join("chunks"));
        Held { texts: ids(dir).len(), chunks: chunks.len(), bytes: chunks.iter().map(|e| e.metadata().map_or(0, |m| m.len())).sum() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A text like a log: numbered lines, each different, `n` of them.
    fn log(n: usize) -> Vec<u8> {
        (0..n).map(|i| format!("{i:06} the request took {} ms and answered {}\n", (i * 7919) % 1000, 200 + (i % 3))).collect::<String>().into_bytes()
    }

    #[test]
    fn a_text_is_cut_where_its_content_says_so_a_change_adds_only_the_chunks_around_it() {
        let text = log(40_000);
        let parts = chunks(&text);
        assert_eq!(parts.concat(), text);
        assert!(parts.iter().all(|c| c.len() <= CHUNK_MAX));
        assert!(parts[..parts.len() - 1].iter().all(|c| c.len() >= CHUNK_MIN));
        let average = text.len() / parts.len();
        assert!((CHUNK_MIN * 2..CHUNK_MAX).contains(&average), "{average}");
        assert_eq!(chunks(b"small"), vec![&b"small"[..]]);
        assert!(chunks(b"").is_empty());

        // A line changed in the middle: the chunks before and after it are the same chunks.
        let mut changed = text.clone();
        let at = text.len() / 2;
        changed.splice(at..at + 5, *b"EDIT!");
        let before: BTreeSet<&[u8]> = parts.iter().copied().collect();
        let new = chunks(&changed).into_iter().filter(|c| !before.contains(c)).count();
        assert!(new <= 2, "{new} new chunks for one change");
    }

    #[test]
    fn a_text_is_kept_once_and_a_grown_one_adds_only_its_new_chunks() {
        let dir = std::env::temp_dir().join(format!("holon-blobs-{}", rand::random::<u64>()));
        for blobs in [Blobs::memory(), Blobs::at(dir.clone())] {
            let first = log(30_000);
            let id = blobs.put(&first).unwrap();
            assert_eq!(id, blake3::hash(&first).to_hex().to_string());
            assert_eq!(blobs.get(&id).unwrap().as_deref(), Some(&first[..]));
            let held = blobs.held();
            assert_eq!(held.bytes, first.len() as u64);

            // The same text again: nothing more.
            assert_eq!(blobs.put(&first).unwrap(), id);
            assert_eq!(blobs.held(), held);

            // The file grew by a few lines: one or two chunks more, not a second copy.
            let mut grown = first.clone();
            grown.extend_from_slice(b"one more line\nand another\n");
            let id2 = blobs.put(&grown).unwrap();
            assert_eq!(blobs.get(&id2).unwrap().as_deref(), Some(&grown[..]));
            let more = blobs.held().bytes - held.bytes;
            assert!(more <= 2 * CHUNK_MAX as u64 && more < first.len() as u64 / 4, "{more} bytes more");
            assert_eq!(blobs.get(&"0".repeat(64)).unwrap(), None);
            assert_eq!(blobs.get("../x").unwrap(), None);

            // Swept: what no run refers to goes, what one does stays, and so do its chunks.
            let live = BTreeSet::from([id2.clone()]);
            assert_eq!(blobs.sweep(&live, 0).unwrap(), Swept::default(), "nothing put before then");
            let swept = blobs.sweep(&live, now_ms() + 1_000).unwrap();
            assert_eq!(swept.texts, 1);
            assert_eq!(blobs.get(&id).unwrap(), None);
            assert_eq!(blobs.get(&id2).unwrap().as_deref(), Some(&grown[..]), "its chunks are all still there");
            assert!(swept.chunks <= 1, "the grown text uses every chunk the first did but its last");
            let all = blobs.sweep(&BTreeSet::new(), now_ms() + 1_000).unwrap();
            assert_eq!((all.texts, blobs.held()), (1, Held::default()));
        }
        // A damaged chunk is found, not answered.
        let blobs = Blobs::at(dir.clone());
        let id = blobs.put(&log(100)).unwrap();
        let chunk = std::fs::read_dir(dir.join("chunks")).unwrap().flatten().next().unwrap().path();
        std::fs::write(&chunk, b"something else").unwrap();
        assert!(blobs.get(&id).unwrap_err().contains("damaged"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
