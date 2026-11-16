//! Storage of recorded blocks: an append-only, crash-safe log with a small index.
//!
//! On disk (inside the store folder):
//!
//! ```text
//! data/2026-11.hflog    one log per calendar month (UTC) of the block's start
//! data/index.json       per-file summary so files need not be read at start
//! ```
//!
//! A log is text, one record per line: `<crc32 of payload, 8 hex digits>` TAB
//! `<payload>` LF. The payload is the block as JSON (a "put"), or
//! `{"del":<id>}` (a tombstone). The last record for an id wins. A line that
//! is torn, garbled or fails its checksum is skipped when the file is read; it
//! never stops the rest of the file from loading, and the next append starts
//! on a fresh line.
//!
//! Memory: months are loaded only when a query or an edit needs them, and at
//! most [`MAX_LOADED`] stay in memory. Recording new blocks (and re-writing
//! the block that is still growing) appends without loading anything.
//!
//! The index holds, per file: its length when the summary was made, the range
//! of ids, the earliest start, the latest end and the number of live blocks.
//! It is rewritten by [`Store::flush`], [`Store::compact`] and on drop. At
//! start a file whose length differs from the index (a crash before the index
//! was written) is simply read again. The ranges are conservative (they may
//! be wider than the truth, never narrower), which keeps every query correct.

use crate::model::{new_id, Block, BlockId};
use crate::timeutil::civil_from_days;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// How many months are kept in memory at once (disk stores only).
pub const MAX_LOADED: usize = 3;
const RECENT: usize = 64;
const EXT: &str = "hflog";
const BACKUP_FORMAT: &str = "hourforge-backup";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Live blocks.
    pub blocks: usize,
    /// Bytes used by the data files.
    pub bytes: u64,
    pub files: usize,
    /// Earliest start and latest end of anything stored.
    pub first: Option<i64>,
    pub last: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompactReport {
    pub bytes_before: u64,
    pub bytes_after: u64,
    /// Superseded or deleted records that were dropped.
    pub records_dropped: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub added: usize,
    pub replaced: usize,
    /// Lines that could not be read.
    pub skipped: usize,
}

// ---------------------------------------------------------------------------
// Record format
// ---------------------------------------------------------------------------

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

/// CRC-32 (IEEE 802.3), the same checksum zip and PNG use.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for b in bytes {
        c = CRC_TABLE[((c ^ *b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    c ^ 0xffff_ffff
}

#[derive(Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
enum Record {
    Put(Block),
    Del(BlockId),
}

#[derive(Serialize, Deserialize)]
struct Tombstone {
    del: BlockId,
}

fn line_for(payload: &str) -> Vec<u8> {
    format!("{:08x}\t{payload}\n", crc32(payload.as_bytes())).into_bytes()
}

fn put_line(block: &Block) -> io::Result<Vec<u8>> {
    let payload = serde_json::to_string(block).map_err(io::Error::other)?;
    Ok(line_for(&payload))
}

fn del_line(id: BlockId) -> Vec<u8> {
    line_for(&format!("{{\"del\":{id}}}"))
}

fn parse_payload(payload: &str) -> Option<Record> {
    if payload.starts_with("{\"del\":") {
        return serde_json::from_str::<Tombstone>(payload)
            .ok()
            .map(|t| Record::Del(t.del));
    }
    serde_json::from_str::<Block>(payload).ok().map(Record::Put)
}

/// Parses one log line (without its line feed). `None` = damaged.
fn parse_line(line: &[u8]) -> Option<Record> {
    let line = std::str::from_utf8(line).ok()?;
    let line = line.strip_suffix('\r').unwrap_or(line);
    let (crc, payload) = line.split_once('\t')?;
    if crc.len() != 8 || u32::from_str_radix(crc, 16).ok()? != crc32(payload.as_bytes()) {
        return None;
    }
    parse_payload(payload)
}

// ---------------------------------------------------------------------------
// Months
// ---------------------------------------------------------------------------

/// `year * 12 + month - 1` of the block's start, in UTC.
type MonthKey = i32;

fn month_key(start: i64) -> MonthKey {
    // Clamped to years 1970..=9999 so that every key has a plain file name.
    let day = start.div_euclid(86_400).clamp(0, 2_932_896) as i32;
    let (y, m, _) = civil_from_days(day);
    y * 12 + m as i32 - 1
}

fn month_name(key: MonthKey) -> String {
    format!("{:04}-{:02}", key.div_euclid(12), key.rem_euclid(12) + 1)
}

fn parse_month_name(stem: &str) -> Option<MonthKey> {
    let (y, m) = stem.split_once('-')?;
    if y.len() != 4 || m.len() != 2 {
        return None;
    }
    let y: i32 = y.parse().ok()?;
    let m: i32 = m.parse().ok()?;
    ((1970..=9999).contains(&y) && (1..=12).contains(&m)).then_some(y * 12 + m - 1)
}

/// What is known about one month without loading it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct Meta {
    /// File length these numbers were valid for.
    len: u64,
    min_id: BlockId,
    max_id: BlockId,
    min_start: i64,
    max_end: i64,
    live: usize,
    /// The ranges may be wider than the loaded content (after deletes).
    #[serde(skip)]
    stale: bool,
}

impl Meta {
    fn empty() -> Meta {
        Meta {
            len: 0,
            min_id: BlockId::MAX,
            max_id: 0,
            min_start: i64::MAX,
            max_end: i64::MIN,
            live: 0,
            stale: false,
        }
    }

    fn widen(&mut self, b: &Block) {
        self.min_id = self.min_id.min(b.id);
        self.max_id = self.max_id.max(b.id);
        self.min_start = self.min_start.min(b.start);
        self.max_end = self.max_end.max(b.end);
    }

    fn of(blocks: &HashMap<BlockId, Block>, len: u64) -> Meta {
        let mut m = Meta::empty();
        m.len = len;
        m.live = blocks.len();
        for b in blocks.values() {
            m.widen(b);
        }
        m
    }
}

#[derive(Serialize, Deserialize, Default)]
struct IndexFile {
    version: u32,
    months: BTreeMap<String, Meta>,
}

struct Loaded {
    blocks: HashMap<BlockId, Block>,
    used: u64,
}

struct MonthFile {
    blocks: HashMap<BlockId, Block>,
    /// Readable records, including superseded ones and tombstones.
    records: usize,
    damaged: usize,
    len: u64,
}

fn read_month_file(path: &Path) -> io::Result<MonthFile> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    let mut out = MonthFile {
        blocks: HashMap::new(),
        records: 0,
        damaged: 0,
        len: bytes.len() as u64,
    };
    for line in bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        match parse_line(line) {
            Some(Record::Put(b)) => {
                out.records += 1;
                out.blocks.insert(b.id, b);
            }
            Some(Record::Del(id)) => {
                out.records += 1;
                out.blocks.remove(&id);
            }
            None => out.damaged += 1,
        }
    }
    Ok(out)
}

fn sorted(blocks: &HashMap<BlockId, Block>) -> Vec<&Block> {
    let mut v: Vec<&Block> = blocks.values().collect();
    v.sort_by_key(|b| (b.start, b.id));
    v
}

/// Writes `bytes` to `path` through a temporary file and an atomic rename.
fn write_atomic(path: &Path, bytes: &[u8], sync: bool) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        if sync {
            f.sync_all()?;
        }
    }
    fs::rename(&tmp, path)
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

pub struct Store {
    dir: Option<PathBuf>,
    /// `dir/data`; `None` for a store that lives only in memory.
    data: Option<PathBuf>,
    metas: BTreeMap<MonthKey, Meta>,
    loaded: HashMap<MonthKey, Loaded>,
    clock: u64,
    /// Where the most recently written ids live, so that re-writing the
    /// growing block never needs a month in memory.
    recent: VecDeque<(BlockId, MonthKey)>,
    writer: Option<(MonthKey, File)>,
    /// Opened by `open_read_only`: nothing on disk is ever created or changed.
    read_only: bool,
    rev: u64,
    damaged: HashMap<MonthKey, usize>,
}

impl Store {
    /// A store that lives only in memory (demo mode, tests).
    pub fn in_memory() -> Store {
        Store {
            dir: None,
            data: None,
            metas: BTreeMap::new(),
            loaded: HashMap::new(),
            clock: 0,
            recent: VecDeque::new(),
            writer: None,
            read_only: false,
            rev: 0,
            damaged: HashMap::new(),
        }
    }

    /// Opens (creating if needed) the store in `dir`. Damaged or half-written
    /// records at the end of a file are ignored, never fatal.
    pub fn open(dir: &Path) -> io::Result<Store> {
        Store::open_with(dir, false)
    }

    /// Opens an existing data folder without ever creating, repairing or
    /// writing a file. For reading while the tracker (another process) keeps
    /// appending: a growing file or a half-written last line is fine. Every
    /// method that would change the store returns an error.
    pub fn open_read_only(dir: &Path) -> io::Result<Store> {
        Store::open_with(dir, true)
    }

    fn writable(&self) -> io::Result<()> {
        if self.read_only {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the store was opened read-only",
            ))
        } else {
            Ok(())
        }
    }

    fn open_with(dir: &Path, read_only: bool) -> io::Result<Store> {
        let data = dir.join("data");
        if !read_only {
            fs::create_dir_all(&data)?;
        }
        let mut store = Store::in_memory();
        store.dir = Some(dir.to_path_buf());
        store.data = Some(data.clone());
        store.read_only = read_only;

        let index: IndexFile = fs::read(data.join("index.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let entries = match fs::read_dir(&data) {
            Ok(e) => e,
            // Read-only and nothing recorded yet: an empty store.
            Err(e) if read_only && e.kind() == io::ErrorKind::NotFound => return Ok(store),
            Err(e) => return Err(e),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext == "tmp" {
                // Left behind by a compaction or index write that did not finish.
                if !read_only {
                    let _ = fs::remove_file(&path);
                }
                continue;
            }
            if ext != EXT {
                continue;
            }
            let Some(key) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(parse_month_name)
            else {
                continue;
            };
            let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let known = index
                .months
                .get(&month_name(key))
                .filter(|m| m.len == len && m.len > 0);
            let meta = match known {
                Some(m) => *m,
                None => {
                    let file = read_month_file(&path)?;
                    store.damaged.insert(key, file.damaged);
                    Meta::of(&file.blocks, file.len)
                }
            };
            store.metas.insert(key, meta);
        }
        Ok(store)
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// A counter that changes whenever the stored content changes. Views use
    /// it to know when to recompute.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// Damaged lines met while reading data files since the store was opened.
    pub fn damaged_lines(&self) -> usize {
        self.damaged.values().sum()
    }

    /// Drops every month held in memory (they are read again when needed).
    /// The tracker calls this when the window closes, to stay small.
    pub fn trim_cache(&mut self) {
        if self.data.is_some() {
            self.loaded.clear();
        }
    }

    fn is_disk(&self) -> bool {
        self.data.is_some()
    }

    fn path(&self, key: MonthKey) -> Option<PathBuf> {
        self.data
            .as_ref()
            .map(|d| d.join(format!("{}.{EXT}", month_name(key))))
    }

    fn touch(&mut self, key: MonthKey) {
        self.clock += 1;
        if let Some(l) = self.loaded.get_mut(&key) {
            l.used = self.clock;
        }
    }

    /// Makes sure a month is in memory. `false` when the month does not exist.
    fn ensure_loaded(&mut self, key: MonthKey) -> bool {
        if self.loaded.contains_key(&key) {
            self.touch(key);
            return true;
        }
        if !self.metas.contains_key(&key) {
            return false;
        }
        let Some(path) = self.path(key) else {
            return false;
        };
        let Ok(file) = read_month_file(&path) else {
            return false;
        };
        self.damaged.insert(key, file.damaged);
        self.metas.insert(key, Meta::of(&file.blocks, file.len));
        self.loaded.insert(
            key,
            Loaded {
                blocks: file.blocks,
                used: 0,
            },
        );
        self.touch(key);
        while self.loaded.len() > MAX_LOADED {
            let oldest = self
                .loaded
                .iter()
                .filter(|(k, _)| **k != key)
                .min_by_key(|(_, l)| l.used)
                .map(|(k, _)| *k);
            match oldest {
                Some(k) => {
                    self.loaded.remove(&k);
                }
                None => break,
            }
        }
        true
    }

    fn remember(&mut self, id: BlockId, key: MonthKey) {
        self.forget(id);
        if self.recent.len() >= RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back((id, key));
    }

    fn forget(&mut self, id: BlockId) {
        self.recent.retain(|(i, _)| *i != id);
    }

    /// The month an id lives in, if it exists at all.
    fn locate(&mut self, id: BlockId) -> Option<MonthKey> {
        if let Some((_, key)) = self.recent.iter().find(|(i, _)| *i == id) {
            return Some(*key);
        }
        if let Some((key, _)) = self.loaded.iter().find(|(_, l)| l.blocks.contains_key(&id)) {
            return Some(*key);
        }
        // Ids grow with time, so the id range of a file rules most months out.
        let candidates: Vec<MonthKey> = self
            .metas
            .iter()
            .rev()
            .filter(|(k, m)| !self.loaded.contains_key(k) && m.min_id <= id && id <= m.max_id)
            .map(|(k, _)| *k)
            .collect();
        candidates.into_iter().find(|&key| {
            self.ensure_loaded(key)
                && self
                    .loaded
                    .get(&key)
                    .is_some_and(|l| l.blocks.contains_key(&id))
        })
    }

    /// Appends one record to a month's file (nothing to do in memory).
    fn append(&mut self, key: MonthKey, line: &[u8]) -> io::Result<()> {
        let Some(path) = self.path(key) else {
            return Ok(());
        };
        if self.writer.as_ref().map(|(k, _)| *k) != Some(key) {
            self.writer = None;
            let mut file = OpenOptions::new()
                .read(true)
                .append(true)
                .create(true)
                .open(&path)?;
            // A torn last line must not swallow the next record.
            let len = file.metadata()?.len();
            if len > 0 {
                file.seek(SeekFrom::End(-1))?;
                let mut last = [0u8; 1];
                file.read_exact(&mut last)?;
                if last[0] != b'\n' {
                    file.write_all(b"\n")?;
                }
            }
            let len = file.metadata()?.len();
            self.metas.entry(key).or_insert_with(Meta::empty).len = len;
            self.writer = Some((key, file));
        }
        if let Some((_, file)) = self.writer.as_mut() {
            file.write_all(line)?;
        }
        self.metas.entry(key).or_insert_with(Meta::empty).len += line.len() as u64;
        Ok(())
    }

    /// Removes an id from a month: tombstone on disk, entry in memory.
    fn remove_from(&mut self, id: BlockId, key: MonthKey) -> io::Result<()> {
        self.append(key, &del_line(id))?;
        if let Some(l) = self.loaded.get_mut(&key) {
            l.blocks.remove(&id);
        }
        if let Some(m) = self.metas.get_mut(&key) {
            m.live = m.live.saturating_sub(1);
            m.stale = true;
        }
        Ok(())
    }

    /// Inserts a block, or replaces the block with the same id.
    pub fn put(&mut self, block: &Block) -> io::Result<()> {
        self.writable()?;
        let target = month_key(block.start);
        // An id above every id seen so far is new: nothing to look up.
        let newest = self.metas.values().map(|m| m.max_id).max().unwrap_or(0);
        let old = if block.id > newest {
            None
        } else {
            self.locate(block.id)
        };
        if let Some(old_key) = old {
            if old_key != target {
                // The start moved to another month: the old file must forget it.
                self.remove_from(block.id, old_key)?;
            }
        }
        self.append(target, &put_line(block)?)?;
        if !self.is_disk() {
            self.loaded.entry(target).or_insert_with(|| Loaded {
                blocks: HashMap::new(),
                used: 0,
            });
        }
        if let Some(l) = self.loaded.get_mut(&target) {
            l.blocks.insert(block.id, block.clone());
        }
        let meta = self.metas.entry(target).or_insert_with(Meta::empty);
        meta.widen(block);
        if old == Some(target) {
            meta.stale = true;
        } else {
            meta.live += 1;
        }
        self.remember(block.id, target);
        self.rev += 1;
        Ok(())
    }

    pub fn put_many(&mut self, blocks: &[Block]) -> io::Result<()> {
        for b in blocks {
            self.put(b)?;
        }
        Ok(())
    }

    /// Removes a block. Returns whether it existed.
    pub fn delete(&mut self, id: BlockId) -> io::Result<bool> {
        self.writable()?;
        let Some(key) = self.locate(id) else {
            return Ok(false);
        };
        self.remove_from(id, key)?;
        self.forget(id);
        self.rev += 1;
        Ok(true)
    }

    pub fn get(&mut self, id: BlockId) -> Option<Block> {
        let key = self.locate(id)?;
        if !self.ensure_loaded(key) {
            return None;
        }
        self.loaded.get(&key)?.blocks.get(&id).cloned()
    }

    /// Every block overlapping `[from, to)`, sorted by start time, then id.
    pub fn range(&mut self, from: i64, to: i64) -> Vec<Block> {
        // A block is filed by its start, but may reach into later months; the
        // per-month latest end tells exactly which earlier months matter.
        let keys: Vec<MonthKey> = self
            .metas
            .iter()
            .filter(|(_, m)| m.min_start < to && m.max_end > from)
            .map(|(k, _)| *k)
            .collect();
        let mut out = Vec::new();
        for key in keys {
            if !self.ensure_loaded(key) {
                continue;
            }
            if let Some(l) = self.loaded.get(&key) {
                out.extend(
                    l.blocks
                        .values()
                        .filter(|b| b.start < to && b.end > from)
                        .cloned(),
                );
            }
        }
        out.sort_by_key(|b| (b.start, b.id));
        out
    }

    /// Deletes everything recorded in `[from, to)`. Blocks reaching across an
    /// edge are cut at the edge; a block spanning the whole range is cut in
    /// two. Returns the number of blocks removed or changed.
    pub fn delete_range(&mut self, from: i64, to: i64) -> io::Result<usize> {
        self.writable()?;
        if to <= from {
            return Ok(0);
        }
        let hits = self.range(from, to);
        for b in &hits {
            let before = b.start < from;
            let after = b.end > to;
            if before {
                self.put(&Block {
                    end: from,
                    ..b.clone()
                })?;
            }
            if after {
                self.put(&Block {
                    id: if before { new_id() } else { b.id },
                    start: to,
                    ..b.clone()
                })?;
            }
            if !before && !after {
                self.delete(b.id)?;
            }
        }
        self.rev += 1;
        Ok(hits.len())
    }

    fn write_index(&self) -> io::Result<()> {
        if self.read_only {
            return Ok(());
        }
        let Some(data) = &self.data else {
            return Ok(());
        };
        let index = IndexFile {
            version: 1,
            months: self
                .metas
                .iter()
                .map(|(k, m)| (month_name(*k), *m))
                .collect(),
        };
        let json = serde_json::to_vec(&index).map_err(io::Error::other)?;
        write_atomic(&data.join("index.json"), &json, false)
    }

    /// Makes sure everything written so far is on disk.
    pub fn flush(&mut self) -> io::Result<()> {
        if let Some((_, file)) = self.writer.as_mut() {
            file.sync_all()?;
        }
        self.write_index()
    }

    /// Rewrites the data files without superseded and deleted records.
    ///
    /// Each file is written to a temporary file first and then renamed over
    /// the old one, so a crash at any point leaves either the old or the new
    /// file, both complete. A leftover temporary file is removed at the next
    /// start.
    pub fn compact(&mut self) -> io::Result<CompactReport> {
        self.writable()?;
        let mut report = CompactReport::default();
        if !self.is_disk() {
            return Ok(report);
        }
        self.writer = None;
        let keys: Vec<MonthKey> = self.metas.keys().copied().collect();
        for key in keys {
            let Some(path) = self.path(key) else { continue };
            let file = read_month_file(&path)?;
            self.damaged.insert(key, file.damaged);
            report.bytes_before += file.len;
            report.records_dropped += file.records + file.damaged - file.blocks.len();
            if file.blocks.is_empty() {
                let _ = fs::remove_file(&path);
                self.metas.remove(&key);
                self.loaded.remove(&key);
                continue;
            }
            let mut bytes = Vec::with_capacity(file.len as usize);
            for b in sorted(&file.blocks) {
                bytes.extend_from_slice(&put_line(b)?);
            }
            write_atomic(&path, &bytes, true)?;
            report.bytes_after += bytes.len() as u64;
            self.metas
                .insert(key, Meta::of(&file.blocks, bytes.len() as u64));
            if let Some(l) = self.loaded.get_mut(&key) {
                l.blocks = file.blocks;
            }
        }
        self.write_index()?;
        Ok(report)
    }

    /// Deletes everything that ended more than `keep_days` days before `now`.
    /// `keep_days == 0` keeps everything. Returns the number of blocks removed.
    pub fn apply_retention(&mut self, keep_days: u32, now: i64) -> io::Result<usize> {
        self.writable()?;
        if keep_days == 0 {
            return Ok(0);
        }
        let cutoff = now - keep_days as i64 * 86_400;
        let mut removed = 0;
        let keys: Vec<(MonthKey, Meta)> = self.metas.iter().map(|(k, m)| (*k, *m)).collect();
        for (key, meta) in keys {
            if meta.max_end <= cutoff {
                // Nothing in this month is young enough: drop the whole file.
                if self.writer.as_ref().map(|(k, _)| *k) == Some(key) {
                    self.writer = None;
                }
                if let Some(path) = self.path(key) {
                    match fs::remove_file(&path) {
                        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                        _ => {}
                    }
                }
                removed += meta.live;
                self.metas.remove(&key);
                self.loaded.remove(&key);
                self.recent.retain(|(_, k)| *k != key);
            } else if meta.min_start < cutoff && self.ensure_loaded(key) {
                let old: Vec<BlockId> = self
                    .loaded
                    .get(&key)
                    .map(|l| {
                        l.blocks
                            .values()
                            .filter(|b| b.end <= cutoff)
                            .map(|b| b.id)
                            .collect()
                    })
                    .unwrap_or_default();
                for id in old {
                    self.remove_from(id, key)?;
                    self.forget(id);
                    removed += 1;
                }
            }
        }
        if removed > 0 {
            self.rev += 1;
        }
        Ok(removed)
    }

    /// Calls `f` with the live blocks of every month, oldest month first,
    /// without filling the cache.
    fn for_each_month(
        &mut self,
        mut f: impl FnMut(&HashMap<BlockId, Block>) -> io::Result<()>,
    ) -> io::Result<()> {
        let keys: Vec<MonthKey> = self.metas.keys().copied().collect();
        for key in keys {
            if let Some(l) = self.loaded.get(&key) {
                f(&l.blocks)?;
            } else if let Some(path) = self.path(key) {
                f(&read_month_file(&path)?.blocks)?;
            }
        }
        Ok(())
    }

    /// Writes every block into one backup file. Returns the number of blocks.
    ///
    /// The file starts with a header line
    /// (`{"format":"hourforge-backup","version":1,...}`) followed by one
    /// checksummed record per line, the same as in the data files.
    pub fn backup(&mut self, to: &Path) -> io::Result<usize> {
        let total: usize = self.metas.values().map(|m| m.live).sum();
        let header = serde_json::json!({
            "format": BACKUP_FORMAT,
            "version": 1,
            "blocks": total,
            "created": crate::timeutil::now(),
        });
        let mut bytes = format!("{header}\n").into_bytes();
        let mut count = 0;
        self.for_each_month(|blocks| {
            for b in sorted(blocks) {
                bytes.extend_from_slice(&put_line(b)?);
                count += 1;
            }
            Ok(())
        })?;
        write_atomic(to, &bytes, true)?;
        Ok(count)
    }

    /// Merges a backup file into the store (blocks with the same id are replaced).
    ///
    /// Reads backups written by [`Store::backup`], raw data files, and plain
    /// files with one block as JSON per line. Lines that cannot be read are
    /// counted and skipped.
    pub fn import(&mut self, from: &Path) -> io::Result<ImportReport> {
        self.writable()?;
        let bytes = fs::read(from)?;
        let mut report = ImportReport::default();
        for (n, raw) in bytes.split(|b| *b == b'\n').enumerate() {
            let Ok(text) = std::str::from_utf8(raw) else {
                report.skipped += 1;
                continue;
            };
            let text = text.trim_start_matches('\u{feff}').trim();
            if text.is_empty() {
                continue;
            }
            if n == 0 && text.starts_with('{') {
                let header = serde_json::from_str::<serde_json::Value>(text).ok();
                if header.is_some_and(|h| h.get("format").is_some()) {
                    continue;
                }
            }
            let record = if text.starts_with('{') {
                parse_payload(text)
            } else {
                parse_line(text.as_bytes())
            };
            match record {
                Some(Record::Put(b)) if b.end >= b.start => {
                    if self.locate(b.id).is_some() {
                        report.replaced += 1;
                    } else {
                        report.added += 1;
                    }
                    self.put(&b)?;
                }
                Some(Record::Del(id)) => {
                    // Only raw data files contain these: the block was deleted later.
                    self.delete(id)?;
                }
                _ => report.skipped += 1,
            }
        }
        self.flush()?;
        Ok(report)
    }

    pub fn stats(&mut self) -> Stats {
        // Ranges of months in memory can be made exact again cheaply.
        for (key, l) in &self.loaded {
            if let Some(m) = self.metas.get_mut(key) {
                if m.stale {
                    *m = Meta::of(&l.blocks, m.len);
                }
            }
        }
        let live = || self.metas.values().filter(|m| m.live > 0);
        Stats {
            blocks: live().map(|m| m.live).sum(),
            bytes: self.metas.values().map(|m| m.len).sum(),
            files: if self.is_disk() { self.metas.len() } else { 0 },
            first: live().map(|m| m.min_start).min(),
            last: live().map(|m| m.max_end).max(),
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // Best effort: without it the next start just reads the changed files again.
        let _ = self.write_index();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Kind;
    use crate::timeutil::days_from_civil;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let dir = std::env::temp_dir().join(format!(
                "hourforge-store-{tag}-{}-{}",
                std::process::id(),
                new_id()
            ));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn log(&self, name: &str) -> PathBuf {
            self.0.join("data").join(format!("{name}.hflog"))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn at(y: i32, m: u32, d: u32, h: i64) -> i64 {
        days_from_civil(y, m, d) as i64 * 86_400 + h * 3600
    }

    fn block(id: u64, start: i64, end: i64, title: &str) -> Block {
        Block {
            id,
            start,
            end,
            exe: "editor.exe".into(),
            app: "Editor".into(),
            title: title.into(),
            ..Default::default()
        }
    }

    fn all(store: &mut Store) -> Vec<Block> {
        store.range(i64::MIN, i64::MAX)
    }

    #[test]
    fn crc_matches_the_standard() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(
            parse_line(&del_line(42)[..del_line(42).len() - 1]),
            Some(Record::Del(42))
        );
    }

    #[test]
    fn month_keys_and_names() {
        let k = month_key(at(2026, 11, 12, 5));
        assert_eq!(month_name(k), "2026-11");
        assert_eq!(parse_month_name("2026-11"), Some(k));
        assert_eq!(month_key(at(2026, 12, 1, 0)), k + 1);
        assert_eq!(month_key(at(2026, 12, 1, 0) - 1), k);
        assert_eq!(month_name(month_key(-5)), "1970-01");
        assert_eq!(parse_month_name("index"), None);
        assert_eq!(parse_month_name("2026-13"), None);
    }

    #[test]
    fn round_trip_and_reopen() {
        let tmp = TempDir::new("roundtrip");
        let t = at(2026, 11, 12, 9);
        {
            let mut s = Store::open(&tmp.0).unwrap();
            assert_eq!(s.dir(), Some(tmp.0.as_path()));
            s.put(&block(1, t, t + 600, "one")).unwrap();
            s.put(&block(
                2,
                t + 600,
                t + 900,
                "two, with \"quotes\"\tand\nnewline",
            ))
            .unwrap();
            let mut meeting = block(3, t + 900, t + 1800, "");
            meeting.kind = Kind::Away;
            meeting.away = crate::model::AwayLabel::Meeting;
            meeting.tags = vec!["a".into(), "b".into()];
            s.put(&meeting).unwrap();
            assert_eq!(s.rev(), 3);
            s.flush().unwrap();
        }
        let mut s = Store::open(&tmp.0).unwrap();
        let got = all(&mut s);
        assert_eq!(got.len(), 3);
        assert_eq!(got[1].title, "two, with \"quotes\"\tand\nnewline");
        assert_eq!(got[2].tags, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(s.get(1).unwrap().title, "one");
        assert!(s.get(99).is_none());
        assert_eq!(s.range(t + 100, t + 700).len(), 2);
        assert_eq!(s.range(t + 600, t + 601).len(), 1);
        assert!(s.range(t + 1800, t + 5000).is_empty());
        let st = s.stats();
        assert_eq!((st.blocks, st.files), (3, 1));
        assert_eq!((st.first, st.last), (Some(t), Some(t + 1800)));
        assert!(st.bytes > 100);
        assert_eq!(s.damaged_lines(), 0);
    }

    #[test]
    fn reopen_without_index_reads_the_files() {
        let tmp = TempDir::new("noindex");
        let t = at(2026, 5, 3, 9);
        {
            let mut s = Store::open(&tmp.0).unwrap();
            s.put(&block(1, t, t + 60, "a")).unwrap();
            s.put(&block(2, t + 60, t + 120, "b")).unwrap();
        }
        fs::remove_file(tmp.0.join("data").join("index.json")).unwrap();
        let mut s = Store::open(&tmp.0).unwrap();
        assert_eq!(s.stats().blocks, 2);
        assert_eq!(all(&mut s).len(), 2);
    }

    #[test]
    fn replace_and_delete() {
        for disk in [false, true] {
            let tmp = TempDir::new("replace");
            let mut s = if disk {
                Store::open(&tmp.0).unwrap()
            } else {
                Store::in_memory()
            };
            let t = at(2026, 11, 12, 9);
            s.put(&block(1, t, t + 60, "open")).unwrap();
            // The growing block is written again and again.
            for i in 2..10 {
                s.put(&block(1, t, t + 60 * i, "open")).unwrap();
            }
            s.put(&block(2, t + 600, t + 700, "next")).unwrap();
            assert_eq!(s.stats().blocks, 2);
            assert_eq!(s.get(1).unwrap().end, t + 540);
            assert!(s.delete(2).unwrap());
            assert!(!s.delete(2).unwrap());
            assert!(!s.delete(77).unwrap());
            assert_eq!(all(&mut s).len(), 1);
            assert_eq!(s.stats().blocks, 1);
            if disk {
                drop(s);
                let mut s = Store::open(&tmp.0).unwrap();
                let got = all(&mut s);
                assert_eq!(got.len(), 1);
                assert_eq!(got[0].end, t + 540);
            }
        }
    }

    #[test]
    fn appending_does_not_load_the_month() {
        let tmp = TempDir::new("lazy");
        let t = at(2026, 11, 12, 9);
        {
            let mut s = Store::open(&tmp.0).unwrap();
            for i in 0..50u64 {
                s.put(&block(
                    i + 1,
                    t + i as i64 * 60,
                    t + i as i64 * 60 + 60,
                    "x",
                ))
                .unwrap();
            }
        }
        let mut s = Store::open(&tmp.0).unwrap();
        assert!(s.loaded.is_empty());
        let id = new_id();
        s.put(&block(id, t + 9000, t + 9060, "new")).unwrap();
        s.put(&block(id, t + 9000, t + 9120, "new")).unwrap();
        assert!(s.loaded.is_empty(), "new blocks append without loading");
        assert_eq!(s.stats().blocks, 51);
        assert_eq!(s.range(t + 9000, t + 9001).len(), 1);
        assert_eq!(s.loaded.len(), 1);
        s.trim_cache();
        assert!(s.loaded.is_empty());
        assert_eq!(s.get(id).unwrap().end, t + 9120);
    }

    #[test]
    fn torn_tail_and_corrupted_middle_are_skipped() {
        let tmp = TempDir::new("torn");
        let t = at(2026, 11, 12, 9);
        {
            let mut s = Store::open(&tmp.0).unwrap();
            for i in 0..5u64 {
                s.put(&block(
                    i + 1,
                    t + i as i64 * 100,
                    t + i as i64 * 100 + 100,
                    "ok",
                ))
                .unwrap();
            }
        }
        let path = tmp.log("2026-11");
        let mut bytes = fs::read(&path).unwrap();
        // Flip a byte inside the third record's payload...
        let starts: Vec<usize> = std::iter::once(0)
            .chain(
                bytes
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| **b == b'\n')
                    .map(|(i, _)| i + 1),
            )
            .collect();
        bytes[starts[2] + 20] ^= 0x55;
        // ...add a line of garbage, and a record cut off by a crash.
        bytes.extend_from_slice(b"\xff\xfe not a record\n");
        let torn = put_line(&block(6, t + 900, t + 950, "torn")).unwrap();
        bytes.extend_from_slice(&torn[..torn.len() / 2]);
        fs::write(&path, &bytes).unwrap();

        let mut s = Store::open(&tmp.0).unwrap();
        let ids: Vec<u64> = all(&mut s).iter().map(|b| b.id).collect();
        assert_eq!(ids, vec![1, 2, 4, 5]);
        assert_eq!(s.damaged_lines(), 3);
        // Appending after a torn tail must stay readable.
        s.put(&block(7, t + 1000, t + 1100, "after")).unwrap();
        s.put(&block(3, t + 200, t + 300, "restored")).unwrap();
        drop(s);
        let mut s = Store::open(&tmp.0).unwrap();
        let ids: Vec<u64> = all(&mut s).iter().map(|b| b.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5, 7]);
        // An empty and a missing-newline-only file are harmless too.
        fs::write(tmp.log("2026-10"), b"").unwrap();
        fs::write(tmp.log("2026-09"), b"zzzz").unwrap();
        fs::write(tmp.0.join("data").join("notes.txt"), b"ignored").unwrap();
        drop(s);
        let mut s = Store::open(&tmp.0).unwrap();
        assert_eq!(all(&mut s).len(), 6);
        s.put(&block(8, at(2026, 9, 2, 1), at(2026, 9, 2, 2), "sep"))
            .unwrap();
        drop(s);
        let mut s = Store::open(&tmp.0).unwrap();
        assert_eq!(s.get(8).unwrap().title, "sep");
    }

    #[test]
    fn month_boundaries() {
        let tmp = TempDir::new("months");
        let edge = at(2026, 12, 1, 0);
        let mut s = Store::open(&tmp.0).unwrap();
        s.put(&block(1, edge - 3600, edge - 60, "november"))
            .unwrap();
        // Starts in November, ends in December.
        s.put(&block(2, edge - 60, edge + 600, "across")).unwrap();
        s.put(&block(3, edge + 600, edge + 900, "december"))
            .unwrap();
        // A long manual entry from October reaching into December.
        s.put(&block(4, at(2026, 10, 20, 0), edge + 100, "long"))
            .unwrap();
        s.put(&block(5, at(2027, 1, 1, 0), at(2027, 1, 1, 1), "january"))
            .unwrap();
        assert!(tmp.log("2026-10").exists());
        assert!(tmp.log("2026-11").exists());
        assert!(tmp.log("2026-12").exists());
        assert!(tmp.log("2027-01").exists());
        let ids = |v: Vec<Block>| v.iter().map(|b| b.id).collect::<Vec<_>>();
        assert_eq!(ids(s.range(edge, edge + 50)), vec![4, 2]);
        assert_eq!(ids(s.range(edge + 700, edge + 800)), vec![3]);
        assert_eq!(ids(s.range(edge - 30, edge + 700)), vec![4, 2, 3]);
        assert_eq!(
            ids(s.range(at(2026, 12, 31, 23), at(2027, 2, 1, 0))),
            vec![5]
        );
        drop(s);
        let mut s = Store::open(&tmp.0).unwrap();
        s.trim_cache();
        assert_eq!(ids(s.range(edge, edge + 50)), vec![4, 2]);
        assert_eq!(s.stats().files, 4);
    }

    #[test]
    fn a_block_that_moves_to_another_month_leaves_the_old_file() {
        let tmp = TempDir::new("move");
        let nov = at(2026, 11, 30, 23);
        let dec = at(2026, 12, 1, 1);
        {
            let mut s = Store::open(&tmp.0).unwrap();
            s.put(&block(1, nov, nov + 600, "moving")).unwrap();
            s.put(&block(2, nov, nov + 60, "stays")).unwrap();
        }
        {
            // A fresh process: nothing is in memory, nothing in the recent list.
            let mut s = Store::open(&tmp.0).unwrap();
            s.put(&block(1, dec, dec + 600, "moved")).unwrap();
            assert_eq!(all(&mut s).len(), 2);
            assert_eq!(s.stats().blocks, 2);
        }
        let mut s = Store::open(&tmp.0).unwrap();
        let got = all(&mut s);
        assert_eq!(got.len(), 2);
        assert_eq!(s.get(1).unwrap().title, "moved");
        assert!(s.range(nov, nov + 3600).iter().all(|b| b.id == 2));
        // The old file really forgot it, even read on its own.
        let november = read_month_file(&tmp.log("2026-11")).unwrap();
        assert!(!november.blocks.contains_key(&1));
        // And back again.
        s.put(&block(1, nov, nov + 30, "back")).unwrap();
        drop(s);
        let mut s = Store::open(&tmp.0).unwrap();
        assert_eq!(all(&mut s).len(), 2);
        assert_eq!(s.get(1).unwrap().start, nov);
        assert!(!read_month_file(&tmp.log("2026-12"))
            .unwrap()
            .blocks
            .contains_key(&1));
    }

    #[test]
    fn delete_range_trims_and_splits() {
        for disk in [false, true] {
            let tmp = TempDir::new("delrange");
            let mut s = if disk {
                Store::open(&tmp.0).unwrap()
            } else {
                Store::in_memory()
            };
            let t = at(2026, 11, 12, 9);
            s.put(&block(1, t, t + 100, "before")).unwrap();
            s.put(&block(2, t + 100, t + 300, "left edge")).unwrap();
            s.put(&block(3, t + 300, t + 400, "inside")).unwrap();
            s.put(&block(4, t + 400, t + 600, "right edge")).unwrap();
            s.put(&block(5, t + 600, t + 700, "after")).unwrap();
            assert_eq!(s.delete_range(t + 200, t + 500).unwrap(), 3);
            let got: Vec<(u64, i64, i64)> = all(&mut s)
                .iter()
                .map(|b| (b.id, b.start - t, b.end - t))
                .collect();
            assert_eq!(
                got,
                vec![(1, 0, 100), (2, 100, 200), (4, 500, 600), (5, 600, 700)]
            );
            // One block spanning the whole range is cut in two.
            s.put(&block(9, t + 1000, t + 2000, "long")).unwrap();
            assert_eq!(s.delete_range(t + 1200, t + 1300).unwrap(), 1);
            let halves = s.range(t + 1000, t + 2000);
            assert_eq!(halves.len(), 2);
            assert_eq!((halves[0].id, halves[0].end), (9, t + 1200));
            assert_ne!(halves[1].id, 9);
            assert_eq!((halves[1].start, halves[1].end), (t + 1300, t + 2000));
            assert_eq!(halves[1].title, "long");
            assert_eq!(s.delete_range(t + 5000, t + 6000).unwrap(), 0);
            assert_eq!(s.delete_range(t + 10, t + 10).unwrap(), 0);
            assert_eq!(s.stats().blocks, 6);
            if disk {
                drop(s);
                let mut s = Store::open(&tmp.0).unwrap();
                assert_eq!(all(&mut s).len(), 6);
            }
        }
    }

    #[test]
    fn retention() {
        for disk in [false, true] {
            let tmp = TempDir::new("retention");
            let mut s = if disk {
                Store::open(&tmp.0).unwrap()
            } else {
                Store::in_memory()
            };
            let now = at(2026, 11, 12, 12);
            s.put(&block(
                1,
                at(2026, 6, 1, 9),
                at(2026, 6, 1, 10),
                "old month",
            ))
            .unwrap();
            s.put(&block(
                2,
                at(2026, 6, 2, 9),
                at(2026, 6, 2, 10),
                "old month",
            ))
            .unwrap();
            s.put(&block(
                3,
                at(2026, 10, 2, 9),
                at(2026, 10, 2, 10),
                "too old",
            ))
            .unwrap();
            s.put(&block(4, at(2026, 10, 20, 9), at(2026, 10, 20, 10), "kept"))
                .unwrap();
            // Ends exactly at the cut-off: gone. Ends one second later: kept.
            let cutoff = now - 30 * 86_400;
            s.put(&block(5, cutoff - 100, cutoff, "edge out")).unwrap();
            s.put(&block(6, cutoff - 100, cutoff + 1, "edge in"))
                .unwrap();
            assert_eq!(s.apply_retention(0, now).unwrap(), 0);
            assert_eq!(s.apply_retention(30, now).unwrap(), 4);
            let ids: Vec<u64> = all(&mut s).iter().map(|b| b.id).collect();
            assert_eq!(ids, vec![6, 4]);
            assert_eq!(s.apply_retention(30, now).unwrap(), 0);
            if disk {
                assert!(!tmp.log("2026-06").exists());
                drop(s);
                let mut s = Store::open(&tmp.0).unwrap();
                assert_eq!(all(&mut s).len(), 2);
                assert_eq!(s.stats().blocks, 2);
            }
        }
    }

    #[test]
    fn compaction_shrinks_and_preserves() {
        let tmp = TempDir::new("compact");
        let t = at(2026, 11, 12, 9);
        let mut s = Store::open(&tmp.0).unwrap();
        for i in 1..=200i64 {
            s.put(&block(1, t, t + i * 60, "the growing block"))
                .unwrap();
        }
        s.put(&block(2, t + 20_000, t + 20_100, "kept")).unwrap();
        s.put(&block(3, t + 20_100, t + 20_200, "deleted")).unwrap();
        s.delete(3).unwrap();
        // A month where everything was deleted disappears.
        s.put(&block(4, at(2026, 3, 1, 1), at(2026, 3, 1, 2), "gone"))
            .unwrap();
        s.delete(4).unwrap();
        let before = all(&mut s);
        // A temporary file from a compaction that crashed half-way.
        fs::write(tmp.0.join("data").join("2026-11.hflog.tmp"), b"half a fi").unwrap();
        let size_before = fs::metadata(tmp.log("2026-11")).unwrap().len();
        let report = s.compact().unwrap();
        let size_after = fs::metadata(tmp.log("2026-11")).unwrap().len();
        assert!(size_after < size_before / 20);
        assert_eq!(report.bytes_after, size_after);
        assert!(report.bytes_before > report.bytes_after);
        assert_eq!(report.records_dropped, 199 + 2 + 2);
        assert!(!tmp.log("2026-03").exists());
        assert_eq!(all(&mut s), before);
        // Writing continues normally afterwards.
        s.put(&block(5, t + 30_000, t + 30_100, "later")).unwrap();
        assert_eq!(s.compact().unwrap().records_dropped, 0);
        drop(s);
        // A leftover temporary file is removed at the next start and changes nothing.
        let leftover = tmp.0.join("data").join("2026-11.hflog.tmp");
        fs::write(&leftover, b"junk").unwrap();
        let mut s = Store::open(&tmp.0).unwrap();
        assert!(!leftover.exists());
        assert_eq!(all(&mut s).len(), 3);
        assert_eq!(s.stats().blocks, 3);
        assert_eq!(
            Store::in_memory().compact().unwrap(),
            CompactReport::default()
        );
    }

    #[test]
    fn read_only_never_touches_the_files() {
        let tmp = TempDir::new("readonly");
        let t = at(2026, 11, 12, 9);
        // A folder that does not exist stays missing and reads as empty.
        let missing = tmp.0.join("nothing-here");
        let mut ro = Store::open_read_only(&missing).unwrap();
        assert!(all(&mut ro).is_empty());
        drop(ro);
        assert!(!missing.exists());

        // The tracker wrote some records...
        let mut writer = Store::open(&tmp.0).unwrap();
        writer.put(&block(1, t, t + 60, "one")).unwrap();
        writer.put(&block(2, t + 60, t + 120, "two")).unwrap();
        drop(writer);
        let path = tmp.log("2026-11");
        // ...one is only half on disk yet, and there is a stray temporary file.
        let torn = put_line(&block(3, t + 120, t + 180, "half")).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(&torn[..torn.len() / 2]);
        fs::write(&path, &bytes).unwrap();
        let stray = tmp.0.join("data").join("2026-10.hflog.tmp");
        fs::write(&stray, b"x").unwrap();
        let index = tmp.0.join("data").join("index.json");
        let index_before = fs::read(&index).ok();

        let mut ro = Store::open_read_only(&tmp.0).unwrap();
        let ids: Vec<u64> = all(&mut ro).iter().map(|b| b.id).collect();
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(ro.get(2).unwrap().title, "two");
        assert_eq!(ro.stats().blocks, 2);
        // Everything that would change the store is refused.
        assert!(ro.put(&block(9, t, t + 1, "no")).is_err());
        assert!(ro.put_many(&[block(9, t, t + 1, "no")]).is_err());
        assert!(ro.delete(1).is_err());
        assert!(ro.delete_range(t, t + 10).is_err());
        assert!(ro.compact().is_err());
        assert!(ro.apply_retention(1, t + 90 * 86_400).is_err());
        assert!(ro.import(&path).is_err());
        ro.flush().unwrap();
        // A backup goes to a file of its own and is allowed.
        assert_eq!(ro.backup(&tmp.0.join("copy.hfbackup")).unwrap(), 2);
        drop(ro);
        // Not a byte changed: no repairing newline, no index, no clean-up.
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(stray.exists());
        assert_eq!(fs::read(&index).ok(), index_before);
        // The tracker carries on as if nobody had looked.
        let mut writer = Store::open(&tmp.0).unwrap();
        writer.put(&block(4, t + 200, t + 260, "four")).unwrap();
        drop(writer);
        let mut s = Store::open(&tmp.0).unwrap();
        let ids: Vec<u64> = all(&mut s).iter().map(|b| b.id).collect();
        assert_eq!(ids, vec![1, 2, 4]);
    }

    #[test]
    fn backup_and_import() {
        let tmp = TempDir::new("backup");
        let t = at(2026, 11, 12, 9);
        let mut s = Store::open(&tmp.0.join("a")).unwrap();
        s.put(&block(1, t, t + 100, "one")).unwrap();
        s.put(&block(2, at(2026, 8, 1, 9), at(2026, 8, 1, 10), "two"))
            .unwrap();
        s.put(&block(3, t + 100, t + 200, "three")).unwrap();
        s.put(&block(3, t + 100, t + 250, "three, grown")).unwrap();
        s.put(&block(4, t + 300, t + 400, "deleted")).unwrap();
        s.delete(4).unwrap();
        let file = tmp.0.join("all.hfbackup");
        assert_eq!(s.backup(&file).unwrap(), 3);
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.lines().next().unwrap().contains("hourforge-backup"));
        assert_eq!(text.lines().count(), 4);

        // Into an empty store.
        let mut empty = Store::open(&tmp.0.join("b")).unwrap();
        let r = empty.import(&file).unwrap();
        assert_eq!((r.added, r.replaced, r.skipped), (3, 0, 0));
        assert_eq!(all(&mut empty), all(&mut s));
        // Into memory.
        let mut mem = Store::in_memory();
        assert_eq!(mem.import(&file).unwrap().added, 3);
        assert_eq!(all(&mut mem), all(&mut s));
        assert_eq!(mem.backup(&tmp.0.join("mem.hfbackup")).unwrap(), 3);

        // Into a store with overlaps, from a file with damage in it.
        let mut other = Store::open(&tmp.0.join("c")).unwrap();
        other.put(&block(1, t, t + 50, "older copy")).unwrap();
        other
            .put(&block(10, t + 5000, t + 5100, "only here"))
            .unwrap();
        let mut damaged = text.clone();
        damaged.push_str("deadbeef\t{\"i\":50,\"s\":1,\"e\":2}\n");
        damaged.push_str("complete nonsense\n");
        damaged.push_str("{\"i\":60,\"s\":100,\"e\":50}\n");
        damaged.push_str(&format!(
            "{{\"i\":61,\"s\":{},\"e\":{}}}\n",
            t + 9000,
            t + 9100
        ));
        let damaged_file = tmp.0.join("damaged.hfbackup");
        fs::write(&damaged_file, damaged).unwrap();
        let r = other.import(&damaged_file).unwrap();
        assert_eq!((r.added, r.replaced, r.skipped), (3, 1, 3));
        assert_eq!(other.get(1).unwrap().title, "one");
        assert_eq!(all(&mut other).len(), 5);
        // A raw data file can be imported as well.
        let mut raw = Store::in_memory();
        let r = raw
            .import(&tmp.0.join("a").join("data").join("2026-11.hflog"))
            .unwrap();
        assert_eq!(r.skipped, 0);
        assert_eq!(all(&mut raw).len(), 2);
        assert!(Store::in_memory().import(&tmp.0.join("missing")).is_err());
    }

    /// A tiny deterministic generator (xorshift), so the test needs no crate.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    #[test]
    fn random_workload_matches_a_simple_model() {
        let tmp = TempDir::new("random");
        let base = at(2026, 8, 15, 0);
        let span = 120 * 86_400; // about four months
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut model: HashMap<BlockId, Block> = HashMap::new();
        let mut ids: Vec<BlockId> = Vec::new();
        let mut store = Store::open(&tmp.0).unwrap();
        let mut memory = Store::in_memory();
        let mut next_id = 1000u64;
        for step in 0..3000 {
            match rng.below(100) {
                0..=54 => {
                    next_id += 1 + rng.below(3);
                    let start = base + rng.below(span) as i64;
                    let b = block(next_id, start, start + 1 + rng.below(7200) as i64, "new");
                    store.put(&b).unwrap();
                    memory.put(&b).unwrap();
                    model.insert(b.id, b);
                    ids.push(next_id);
                }
                55..=79 if !ids.is_empty() => {
                    // Rewrite an existing block; one time in three it moves anywhere.
                    let id = ids[rng.below(ids.len() as u64) as usize];
                    let Some(old) = model.get(&id).cloned() else {
                        continue;
                    };
                    let start = if rng.below(3) == 0 {
                        base + rng.below(span) as i64
                    } else {
                        old.start
                    };
                    let b = block(id, start, start + 1 + rng.below(90_000) as i64, "edited");
                    store.put(&b).unwrap();
                    memory.put(&b).unwrap();
                    model.insert(id, b);
                }
                80..=91 if !ids.is_empty() => {
                    let id = ids[rng.below(ids.len() as u64) as usize];
                    let existed = model.remove(&id).is_some();
                    assert_eq!(store.delete(id).unwrap(), existed);
                    assert_eq!(memory.delete(id).unwrap(), existed);
                }
                92..=94 => {
                    drop(store);
                    store = Store::open(&tmp.0).unwrap();
                }
                95 => {
                    store.compact().unwrap();
                }
                96 => store.trim_cache(),
                _ => {
                    let from = base + rng.below(span) as i64;
                    let to = from + rng.below(10 * 86_400) as i64;
                    let mut want: Vec<Block> = model
                        .values()
                        .filter(|b| b.start < to && b.end > from)
                        .cloned()
                        .collect();
                    want.sort_by_key(|b| (b.start, b.id));
                    assert_eq!(store.range(from, to), want, "step {step}");
                    assert_eq!(memory.range(from, to), want, "step {step}");
                }
            }
            assert!(store.loaded.len() <= MAX_LOADED);
        }
        let mut want: Vec<Block> = model.values().cloned().collect();
        want.sort_by_key(|b| (b.start, b.id));
        assert_eq!(all(&mut store), want);
        assert_eq!(all(&mut memory), want);
        assert_eq!(store.stats().blocks, want.len());
        assert_eq!(memory.stats().blocks, want.len());
        drop(store);
        let mut store = Store::open(&tmp.0).unwrap();
        assert_eq!(store.stats().blocks, want.len());
        assert_eq!(all(&mut store), want);
        store.compact().unwrap();
        assert_eq!(all(&mut store), want);
        let st = store.stats();
        assert_eq!(st.first, want.iter().map(|b| b.start).min());
        assert_eq!(st.last, want.iter().map(|b| b.end).max());
    }
}
