//! The content store's long-lived values, packed (docs/L4s/storage-L4.md, step 10).
//!
//! A value is written as a file of its own (`values/<hash>`): small, so one file each costs a
//! whole disk block (a 358-byte value takes 4 KiB), and a store writing state leaves thousands.
//! The values that outlive a sweep (still named an hour after they were put) are moved here:
//! many to a pack file, deflated together in blocks of about `BLOCK` bytes, so values alike
//! compress against each other (measured on the soaks' store: 2,000 values, 717 KB, took about
//! 8 MB of disk as files and 128 KB deflated together), each with a 52-byte line of the index.
//!
//! A pack is one file, `values/packs/<id>.pack`, written whole aside, synced, then renamed in:
//! `MAGIC`, the blocks (raw deflate, each), the index (each value's hash, its block's offset and
//! length, and its place in the inflated block), then the index's offset and `MAGIC` again. The
//! loose files are removed only once their pack is in, so a crash leaves a value in both places,
//! never in neither. A read looks for the loose file first, then here (`get`): the index is in
//! memory (about 60 bytes a value), and a few blocks inflated lately are kept.
//!
//! What the server's sweep no longer names is removed by rewriting a pack without it (`rewrite`),
//! the new pack in before the old one goes.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};

use holon_core::Hash;

const MAGIC: &[u8; 8] = b"HOLONPK1";
/// About how many bytes of values a block holds before it is deflated.
pub const BLOCK: usize = 64 << 10;
/// How many bytes of inflated blocks are kept for the reads that come next: a read of a packed
/// value inflates its block, so a store whose values read lately fit here inflates each once.
/// Kept by 16 blocks, a listing reading values across 35 of them inflated one at most reads, and
/// answered 74–82 requests a second against 636–672 from loose files (no value cache).
pub const KEPT_BYTES: usize = 8 << 20;
/// The bytes of one index entry: hash, block offset, block length, offset in the block, length.
const ENTRY: usize = 32 + 8 + 4 + 4 + 4;

/// Where a packed value is.
#[derive(Clone, Copy, Debug, PartialEq)]
struct At {
    pack: u64,
    block: u64,
    block_len: u32,
    offset: u32,
    len: u32,
}

/// What the packs hold.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Held {
    pub packs: usize,
    pub values: usize,
    /// The values' bytes, and the packs' on disk.
    pub bytes: u64,
    pub on_disk: u64,
}

struct Pack {
    path: PathBuf,
    size: u64,
}

#[derive(Default)]
struct Kept {
    index: HashMap<Hash, At>,
    packs: BTreeMap<u64, Pack>,
    next: u64,
}

/// A block inflated lately: its pack and offset, and its bytes.
type Inflated = ((u64, u64), Arc<Vec<u8>>);

/// A pack's bytes, and where each value is in it.
type Written = (Vec<u8>, Vec<(Hash, At)>);

/// A pack written and synced, not yet in the index.
struct NewPack {
    n: u64,
    path: PathBuf,
    size: u64,
    entries: Vec<(Hash, At)>,
}

/// A store's packs: its index, and the blocks read lately, the least recently read first. Its locks
/// are taken back from a panic that held one, as the server's are (holon's sync.rs): a panic in one
/// run would otherwise fail every read of a packed value after it.
pub struct Packs {
    dir: PathBuf,
    kept: RwLock<Kept>,
    blocks: Mutex<VecDeque<Inflated>>,
    /// The packs found that couldn't be read as they opened, and why: left as they are, their
    /// values missing from the store until one is restored.
    unreadable: Vec<(PathBuf, String)>,
}

/// The packs of each values folder this process opened, so the store reading values and the
/// server packing them share one index.
static OPEN: Mutex<Vec<(PathBuf, Weak<Packs>)>> = Mutex::new(Vec::new());

impl Packs {
    /// The packs under `values` (`values/packs/`), opened once a process: each pack's index read.
    /// A pack that can't be read is an error: the values in it would be missing.
    pub fn shared(values: &Path) -> io::Result<Arc<Packs>> {
        let mut open = OPEN.lock().unwrap_or_else(PoisonError::into_inner);
        open.retain(|(_, w)| w.strong_count() > 0);
        if let Some(p) = open.iter().find(|(dir, _)| dir == values).and_then(|(_, w)| w.upgrade()) {
            return Ok(p);
        }
        let packs = Arc::new(Packs::open(values)?);
        open.push((values.to_path_buf(), Arc::downgrade(&packs)));
        Ok(packs)
    }

    fn open(values: &Path) -> io::Result<Packs> {
        let dir = values.join("packs");
        let mut kept = Kept::default();
        let mut unreadable = Vec::new();
        if let Ok(entries) = fs::read_dir(&dir) {
            let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "pack")).collect();
            paths.sort();
            for path in paths {
                // One that can't be read (cut short, or changed on disk) is said and left as it
                // is: the others, and every value of its own file, are read all the same, and a
                // server whose disk lost a pack still starts.
                let (entries, size) = match read_index(&path) {
                    Ok(read) => read,
                    Err(e) => {
                        eprintln!("holon: the pack {} can't be read, its values are missing from the store until it is restored: {e}", path.display());
                        unreadable.push((path, e.to_string()));
                        continue;
                    }
                };
                let n = kept.next;
                kept.next += 1;
                for (hash, mut at) in entries {
                    at.pack = n;
                    kept.index.insert(hash, at);
                }
                kept.packs.insert(n, Pack { path, size });
            }
        }
        Ok(Packs { dir, kept: RwLock::new(kept), blocks: Mutex::new(VecDeque::new()), unreadable })
    }

    /// The packs that couldn't be read as they opened, and why.
    pub fn unreadable(&self) -> &[(PathBuf, String)] {
        &self.unreadable
    }

    /// Whether `key` is packed.
    pub fn contains(&self, key: &Hash) -> bool {
        self.kept.read().unwrap_or_else(PoisonError::into_inner).index.contains_key(key)
    }

    /// The packed value's bytes, as they were put, if it is packed.
    pub fn get(&self, key: &Hash) -> io::Result<Option<Vec<u8>>> {
        // A pack rewritten between the lookup and the read: looked up again.
        for _ in 0..3 {
            let Some((at, path)) = ({
                let kept = self.kept.read().unwrap_or_else(PoisonError::into_inner);
                kept.index.get(key).and_then(|at| kept.packs.get(&at.pack).map(|p| (*at, p.path.clone())))
            }) else {
                return Ok(None);
            };
            match self.block(&path, at) {
                Ok(block) => {
                    let (from, to) = (at.offset as usize, at.offset as usize + at.len as usize);
                    return block.get(from..to).map(|b| Some(b.to_vec())).ok_or_else(|| io::Error::other("a packed value past its block's end"));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }

    fn block(&self, path: &Path, at: At) -> io::Result<Arc<Vec<u8>>> {
        let key = (at.pack, at.block);
        {
            let mut blocks = self.blocks.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(i) = blocks.iter().position(|(k, _)| *k == key) {
                let hit = blocks.remove(i).expect("just found");
                let b = Arc::clone(&hit.1);
                blocks.push_back(hit);
                return Ok(b);
            }
        }
        let mut file = fs::File::open(path)?;
        file.seek(SeekFrom::Start(at.block))?;
        let mut deflated = vec![0; at.block_len as usize];
        file.read_exact(&mut deflated)?;
        let mut block = Vec::new();
        flate2::read::DeflateDecoder::new(&deflated[..]).read_to_end(&mut block)?;
        let block = Arc::new(block);
        let mut blocks = self.blocks.lock().unwrap_or_else(PoisonError::into_inner);
        blocks.push_back((key, Arc::clone(&block)));
        let mut held: usize = blocks.iter().map(|(_, b)| b.len()).sum();
        while held > KEPT_BYTES && blocks.len() > 1 {
            held -= blocks.pop_front().map_or(0, |(_, b)| b.len());
        }
        Ok(block)
    }

    /// Pack `values` (each hash and its bytes as put) into a new pack, in once it is on disk
    /// whole: its loose files may then go. None packed: nothing written. The pack is written and
    /// synced with no lock held: reads of packed values go on meanwhile.
    pub fn pack(&self, values: &[(Hash, Vec<u8>)]) -> io::Result<usize> {
        let fresh: Vec<&(Hash, Vec<u8>)> = values.iter().filter(|(h, _)| !self.contains(h)).collect();
        let Some(written) = self.write(&fresh)? else { return Ok(0) };
        let count = written.entries.len();
        let mut kept = self.kept.write().unwrap_or_else(PoisonError::into_inner);
        for (hash, at) in &written.entries {
            kept.index.insert(*hash, *at);
        }
        kept.packs.insert(written.n, Pack { path: written.path, size: written.size });
        Ok(count)
    }

    /// `values` written into a pack file of its own, synced, not yet in the index: its number,
    /// where it is, its size and where each value is in it. None for no values.
    fn write(&self, values: &[&(Hash, Vec<u8>)]) -> io::Result<Option<NewPack>> {
        if values.is_empty() {
            return Ok(None);
        }
        let (bytes, mut entries) = write_pack(values)?;
        fs::create_dir_all(&self.dir)?;
        let n = {
            let mut kept = self.kept.write().unwrap_or_else(PoisonError::into_inner);
            kept.next += 1;
            kept.next - 1
        };
        let path = self.dir.join(format!("{:016x}-{:08x}.pack", now_ns(), n));
        crate::write_atomically(&path, &bytes)?;
        for (_, at) in entries.iter_mut() {
            at.pack = n;
        }
        Ok(Some(NewPack { n, path, size: bytes.len() as u64, entries }))
    }

    /// Every packed value's hash and length (as put).
    pub fn hashes(&self) -> Vec<(Hash, u64)> {
        self.kept.read().unwrap_or_else(PoisonError::into_inner).index.iter().map(|(h, at)| (*h, at.len as u64)).collect()
    }

    /// Remove `gone` from the packs: each pack holding any of them rewritten with the rest (a
    /// pack left with none, removed), the new one in before the old goes. How many values went,
    /// and their bytes.
    ///
    /// Each new pack is written with no lock held; then `settle` is called, for what must hold
    /// while the index changes (the store's lock, so no put comes between) and which of `gone`
    /// are to stay after all (put again since they were found gone: a put of a packed value
    /// writes nothing). Those are written into a pack of their own, and the index moves to the
    /// new packs at once, so a read finds every value kept throughout.
    pub fn rewrite<G>(&self, gone: &std::collections::HashSet<Hash>, mut settle: impl FnMut() -> (G, std::collections::HashSet<Hash>)) -> io::Result<(usize, u64)> {
        let touched: Vec<(u64, PathBuf)> = {
            let kept = self.kept.read().unwrap_or_else(PoisonError::into_inner);
            let mut packs: Vec<u64> = gone.iter().filter_map(|h| kept.index.get(h).map(|at| at.pack)).collect();
            packs.sort_unstable();
            packs.dedup();
            packs.into_iter().filter_map(|n| kept.packs.get(&n).map(|p| (n, p.path.clone()))).collect()
        };
        let (mut removed, mut bytes) = (0, 0u64);
        for (n, path) in touched {
            let ats: Vec<(Hash, At)> = self.kept.read().unwrap_or_else(PoisonError::into_inner).index.iter().filter(|(_, at)| at.pack == n).map(|(h, at)| (*h, *at)).collect();
            let value = |at: &At| -> io::Result<Vec<u8>> {
                let block = self.block(&path, *at)?;
                block.get(at.offset as usize..(at.offset + at.len) as usize).map(<[u8]>::to_vec).ok_or_else(|| io::Error::other("a packed value past its block's end"))
            };
            let mut live = Vec::new();
            for (hash, at) in ats.iter().filter(|(h, _)| !gone.contains(h)) {
                live.push((*hash, value(at)?));
            }
            // The live ones into a pack of their own, with no lock held.
            let refs: Vec<&(Hash, Vec<u8>)> = live.iter().collect();
            let fresh = self.write(&refs)?;
            let (_held, again) = settle();
            // Put again meanwhile: kept, in a pack of their own (few, if any).
            let mut back = Vec::new();
            for (hash, at) in ats.iter().filter(|(h, _)| gone.contains(h) && again.contains(h)) {
                back.push((*hash, value(at)?));
            }
            let back_refs: Vec<&(Hash, Vec<u8>)> = back.iter().collect();
            let kept_back = self.write(&back_refs)?;
            let mut kept = self.kept.write().unwrap_or_else(PoisonError::into_inner);
            for new in [fresh, kept_back].into_iter().flatten() {
                // Only the values still in the old pack move (another rewrite may have moved some).
                let mut moved = 0;
                for (hash, at) in &new.entries {
                    if kept.index.get(hash).is_some_and(|old| old.pack == n) {
                        kept.index.insert(*hash, *at);
                        moved += 1;
                    }
                }
                if moved > 0 {
                    kept.packs.insert(new.n, Pack { path: new.path, size: new.size });
                } else {
                    let _ = fs::remove_file(&new.path);
                }
            }
            for (hash, at) in &ats {
                if kept.index.get(hash).is_some_and(|old| old.pack == n) {
                    kept.index.remove(hash);
                    removed += 1;
                    bytes += at.len as u64;
                }
            }
            kept.packs.remove(&n);
            drop(kept);
            self.blocks.lock().unwrap_or_else(PoisonError::into_inner).retain(|((p, _), _)| *p != n);
            let _ = fs::remove_file(&path);
        }
        Ok((removed, bytes))
    }

    /// What the packs hold.
    pub fn held(&self) -> Held {
        let kept = self.kept.read().unwrap_or_else(PoisonError::into_inner);
        Held { packs: kept.packs.len(), values: kept.index.len(), bytes: kept.index.values().map(|a| a.len as u64).sum(), on_disk: kept.packs.values().map(|p| p.size).sum() }
    }
}

fn now_ns() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
}

/// A pack's bytes for `values`, and where each is in it (its `pack` number left 0).
fn write_pack(values: &[&(Hash, Vec<u8>)]) -> io::Result<Written> {
    let mut out = MAGIC.to_vec();
    let mut entries = Vec::with_capacity(values.len());
    let mut block: Vec<u8> = Vec::new();
    let mut in_block: Vec<(Hash, u32, u32)> = Vec::new();
    let flush = |out: &mut Vec<u8>, block: &mut Vec<u8>, in_block: &mut Vec<(Hash, u32, u32)>, entries: &mut Vec<(Hash, At)>| -> io::Result<()> {
        if in_block.is_empty() {
            return Ok(());
        }
        let mut z = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(block)?;
        let deflated = z.finish()?;
        let at = out.len() as u64;
        out.extend_from_slice(&deflated);
        for (hash, offset, len) in in_block.drain(..) {
            entries.push((hash, At { pack: 0, block: at, block_len: deflated.len() as u32, offset, len }));
        }
        block.clear();
        Ok(())
    };
    for (hash, bytes) in values {
        in_block.push((*hash, block.len() as u32, bytes.len() as u32));
        block.extend_from_slice(bytes);
        if block.len() >= BLOCK {
            flush(&mut out, &mut block, &mut in_block, &mut entries)?;
        }
    }
    flush(&mut out, &mut block, &mut in_block, &mut entries)?;
    let index_at = out.len() as u64;
    out.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for (hash, at) in &entries {
        out.extend_from_slice(hash);
        out.extend_from_slice(&at.block.to_le_bytes());
        out.extend_from_slice(&at.block_len.to_le_bytes());
        out.extend_from_slice(&at.offset.to_le_bytes());
        out.extend_from_slice(&at.len.to_le_bytes());
    }
    out.extend_from_slice(&index_at.to_le_bytes());
    out.extend_from_slice(MAGIC);
    Ok((out, entries))
}

/// A pack's index, and the pack's size.
fn read_index(path: &Path) -> io::Result<(Vec<(Hash, At)>, u64)> {
    let bytes = fs::read(path)?;
    let bad = || io::Error::other("not a whole pack");
    if bytes.len() < 8 + 8 + 8 + 8 || &bytes[..8] != MAGIC || &bytes[bytes.len() - 8..] != MAGIC {
        return Err(bad());
    }
    let u64_at = |i: usize| bytes.get(i..i + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()));
    let u32_at = |i: usize| bytes.get(i..i + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let index_at = u64_at(bytes.len() - 16).ok_or_else(bad)? as usize;
    let count = u64_at(index_at).ok_or_else(bad)? as usize;
    // Checked: the numbers are the file's, which may be anything.
    let end = count.checked_mul(ENTRY).and_then(|n| n.checked_add(index_at)).and_then(|n| n.checked_add(8 + 16));
    if end != Some(bytes.len()) {
        return Err(bad());
    }
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let e = index_at + 8 + i * ENTRY;
        let hash: Hash = bytes[e..e + 32].try_into().unwrap();
        let at = At {
            pack: 0,
            block: u64_at(e + 32).ok_or_else(bad)?,
            block_len: u32_at(e + 40).ok_or_else(bad)?,
            offset: u32_at(e + 44).ok_or_else(bad)?,
            len: u32_at(e + 48).ok_or_else(bad)?,
        };
        entries.push((hash, at));
    }
    Ok((entries, bytes.len() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("holon-packs-{name}-{}-{}", std::process::id(), now_ns()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn values(n: usize) -> Vec<(Hash, Vec<u8>)> {
        (0..n).map(|i| (*blake3::hash(&i.to_le_bytes()).as_bytes(), format!("{{\"note\":{i},\"title\":\"a note about the {i}th thing\",\"body\":\"some text\"}}").into_bytes())).collect()
    }

    /// Guards: a panic holding the index or the blocks read lately stops no read after it.
    /// Why: every run that reads a packed value takes them; a lock a panic poisoned would fail
    /// each of those runs until the server restarted.
    /// If it fails: one panic makes every packed value unreadable.
    #[test]
    fn a_panic_holding_the_packs_locks_stops_no_read_after_it() {
        let dir = temp("poisoned");
        let packs = Packs::open(&dir).unwrap();
        let vs = values(50);
        packs.pack(&vs).unwrap();
        assert_eq!(packs.get(&vs[0].0).unwrap().as_deref(), Some(&vs[0].1[..]));
        std::thread::scope(|s| {
            assert!(s.spawn(|| {
                let _index = packs.kept.write().unwrap();
                let _blocks = packs.blocks.lock().unwrap();
                panic!("a run panics holding both")
            }).join().is_err());
        });
        for (h, v) in &vs {
            assert_eq!(packs.get(h).unwrap().as_deref(), Some(&v[..]));
        }
        assert!(packs.contains(&vs[1].0));
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Packed values read back as put, from one opened again too; packed again, nothing is
    /// written twice; rewritten without some, those are gone and the rest read back.
    #[test]
    fn packed_values_read_back_as_put_and_a_rewrite_leaves_the_rest() {
        let dir = temp("roundtrip");
        let packs = Packs::open(&dir).unwrap();
        let vs = values(5_000);
        assert_eq!(packs.pack(&vs).unwrap(), 5_000);
        assert_eq!(packs.pack(&vs[..10]).unwrap(), 0, "packed already");
        for (h, v) in vs.iter().step_by(97) {
            assert_eq!(packs.get(h).unwrap().as_deref(), Some(&v[..]));
        }
        let held = packs.held();
        assert_eq!((held.packs, held.values), (1, 5_000));
        // As files of their own, each would take a whole 4 KiB block; packed, its share of a block
        // deflated with the others and its line of the index (52 bytes, most of it its hash).
        assert!(held.on_disk * 30 < 5_000 * 4096, "{held:?}");
        let again = Packs::open(&dir).unwrap();
        for (h, v) in vs.iter().step_by(101) {
            assert_eq!(again.get(h).unwrap().as_deref(), Some(&v[..]), "read from its file's index");
        }
        let gone: std::collections::HashSet<Hash> = vs.iter().step_by(2).map(|(h, _)| *h).collect();
        assert_eq!(packs.rewrite(&gone, none_again).unwrap().0, 2_500);
        for (i, (h, v)) in vs.iter().enumerate() {
            assert_eq!(packs.get(h).unwrap().is_some(), i % 2 == 1, "{i}");
            if i % 2 == 1 && i % 37 == 1 {
                assert_eq!(packs.get(h).unwrap().as_deref(), Some(&v[..]));
            }
        }
        assert_eq!(packs.held().packs, 1, "the old pack gone, the new one in");
        assert_eq!(fs::read_dir(dir.join("packs")).unwrap().count(), 1);
        let all: std::collections::HashSet<Hash> = vs.iter().map(|(h, _)| *h).collect();
        packs.rewrite(&all, none_again).unwrap();
        assert_eq!(packs.held(), Held::default(), "none left: no pack");
        let _ = fs::remove_dir_all(&dir);
    }

    fn none_again() -> ((), std::collections::HashSet<Hash>) {
        ((), std::collections::HashSet::new())
    }

    /// A rewrite writes its packs with no lock held and moves the index at once: a read of a value
    /// kept finds it throughout, and one put again before the index moves stays.
    #[test]
    fn a_rewrite_hides_no_value_it_keeps_and_keeps_one_put_again() {
        let dir = temp("rewrite-reads");
        let packs = Arc::new(Packs::open(&dir).unwrap());
        let vs = values(4_000);
        packs.pack(&vs).unwrap();
        let gone: std::collections::HashSet<Hash> = vs.iter().step_by(2).map(|(h, _)| *h).collect();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let (packs, stop, kept) = (Arc::clone(&packs), Arc::clone(&stop), vs.iter().skip(1).step_by(2).map(|(h, _)| *h).collect::<Vec<_>>());
            std::thread::spawn(move || {
                let mut reads = 0u64;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    for h in kept.iter().step_by(97) {
                        assert!(packs.get(h).unwrap().is_some(), "a value it keeps is read throughout");
                        reads += 1;
                    }
                }
                reads
            })
        };
        // The one put again between the look and the rewrite: the first of those gone.
        let again = vs[0].0;
        let (removed, _) = packs.rewrite(&gone, || ((), [again].into_iter().collect())).unwrap();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(reader.join().unwrap() > 0);
        assert_eq!(removed, gone.len() - 1);
        assert_eq!(packs.get(&again).unwrap().as_deref(), Some(&vs[0].1[..]), "put again: kept");
        assert_eq!(packs.get(&vs[2].0).unwrap(), None, "gone");
        assert_eq!(packs.held().values, 2_001);
        let again_open = Packs::open(&dir).unwrap();
        assert_eq!(again_open.held().values, 2_001, "as on disk");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A pack that can't be read (cut short, or its index's numbers changed) is said, and left as
    /// it is; the others read as they were, and the store opens.
    #[test]
    fn a_pack_cut_short_is_said_and_the_others_read() {
        let dir = temp("short");
        let packs = Packs::open(&dir).unwrap();
        let (first, second) = (values(100), values(200).split_off(100));
        packs.pack(&first).unwrap();
        packs.pack(&second).unwrap();
        let mut paths: Vec<PathBuf> = fs::read_dir(dir.join("packs")).unwrap().flatten().map(|e| e.path()).collect();
        paths.sort();
        let bytes = fs::read(&paths[0]).unwrap();
        fs::write(&paths[0], &bytes[..bytes.len() - 10]).unwrap();
        let again = Packs::open(&dir).unwrap();
        assert_eq!(again.unreadable().iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(), [paths[0].clone()]);
        assert_eq!(again.get(&first[0].0).unwrap(), None, "its values are missing");
        assert_eq!(again.get(&second[0].0).unwrap().as_deref(), Some(&second[0].1[..]), "the other's read");
        assert!(paths[0].exists(), "left as it is, to be restored");
        // An index whose count is past anything: not a whole pack, and no overflow.
        let mut bytes = fs::read(&paths[1]).unwrap();
        let index_at = u64::from_le_bytes(bytes[bytes.len() - 16..bytes.len() - 8].try_into().unwrap()) as usize;
        bytes[index_at..index_at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        fs::write(&paths[1], &bytes).unwrap();
        assert_eq!(Packs::open(&dir).unwrap().unreadable().len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }
}
