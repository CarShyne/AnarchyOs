//! AnarchyFS v2: folders and files on the encrypted volume.
//!
//! Layout, in sectors. Sector 0 is the volume header (volume.rs). Sectors
//! 1 to 16 hold the inode table: 128 entries of 64 bytes each. Everything
//! after that is file data, stored as one contiguous run per file. Folders
//! take no data sectors, only an inode.
//!
//! The whole inode table is kept in memory while the drive is unlocked, so
//! listing folders never touches the disk. Every change is written through
//! to the disk straight away.

use crate::volume;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::ptr::addr_of_mut;

const TABLE_LBA: u64 = 1;
const TABLE_SECTORS: u64 = 16;
const DATA_LBA: u64 = TABLE_LBA + TABLE_SECTORS;
const INODE: usize = 64;
const PER_SECTOR: usize = 512 / INODE;
pub const MAX_INODES: usize = TABLE_SECTORS as usize * PER_SECTOR;
pub const NAME_MAX: usize = 48;
const ROOT: usize = 0;

const FREE: u8 = 0;
const FILE: u8 = 1;
const DIR: u8 = 2;

const DEFAULT_FOLDERS: [&str; 8] =
    ["Applications", "Desktop", "Documents", "Downloads", "Music", "Pictures", "System", "Videos"];

type Inode = [u8; INODE];

static mut TABLE: Vec<Inode> = Vec::new();
static mut MOUNTED: bool = false;

fn table() -> &'static mut Vec<Inode> {
    unsafe { &mut *addr_of_mut!(TABLE) }
}

fn kind(e: &Inode) -> u8 {
    e[0]
}

fn parent(e: &Inode) -> usize {
    u16::from_le_bytes([e[2], e[3]]) as usize
}

fn start(e: &Inode) -> u64 {
    u32::from_le_bytes([e[4], e[5], e[6], e[7]]) as u64
}

fn size(e: &Inode) -> u32 {
    u32::from_le_bytes([e[8], e[9], e[10], e[11]])
}

fn name(e: &Inode) -> &str {
    let raw = &e[16..16 + NAME_MAX];
    let end = raw.iter().position(|&b| b == 0).unwrap_or(NAME_MAX);
    core::str::from_utf8(&raw[..end]).unwrap_or("?")
}

fn fill(e: &mut Inode, k: u8, par: usize, first: u64, len: u32, label: &str) {
    e.fill(0);
    e[0] = k;
    e[2..4].copy_from_slice(&(par as u16).to_le_bytes());
    e[4..8].copy_from_slice(&(first as u32).to_le_bytes());
    e[8..12].copy_from_slice(&len.to_le_bytes());
    e[16..16 + label.len()].copy_from_slice(label.as_bytes());
}

fn sectors_for(bytes: u32) -> u64 {
    (bytes as u64 + 511) / 512
}

fn save_sector(sector: usize) -> bool {
    let t = table();
    let mut buf = [0u8; 512];
    for k in 0..PER_SECTOR {
        let i = sector * PER_SECTOR + k;
        buf[k * INODE..(k + 1) * INODE].copy_from_slice(&t[i]);
    }
    volume::write(TABLE_LBA + sector as u64, &buf)
}

fn save(i: usize) -> Result<(), &'static str> {
    if save_sector(i / PER_SECTOR) {
        Ok(())
    } else {
        Err("could not write the drive")
    }
}

pub fn mounted() -> bool {
    unsafe { MOUNTED }
}

/// Load the inode table. Returns false if the drive is locked or unformatted.
pub fn mount() -> bool {
    unsafe { MOUNTED = false };
    table().clear();
    if !volume::is_open() {
        return false;
    }
    for s in 0..TABLE_SECTORS {
        let mut buf = [0u8; 512];
        if !volume::read(TABLE_LBA + s, &mut buf) {
            table().clear();
            return false;
        }
        for k in 0..PER_SECTOR {
            let mut e = [0u8; INODE];
            e.copy_from_slice(&buf[k * INODE..(k + 1) * INODE]);
            table().push(e);
        }
    }
    if kind(&table()[ROOT]) != DIR {
        table().clear();
        return false;
    }
    unsafe { MOUNTED = true };
    true
}

/// Forget the inode table. Called when the drive locks.
pub fn unmount() {
    table().clear();
    unsafe { MOUNTED = false };
}

/// Write an empty file system with the standard folders. The volume must be open.
pub fn format() -> Result<(), &'static str> {
    if !volume::is_open() {
        return Err("drive is locked");
    }
    unsafe { MOUNTED = true };
    let t = table();
    t.clear();
    t.resize(MAX_INODES, [0; INODE]);
    fill(&mut t[ROOT], DIR, ROOT, 0, 0, "/");
    for s in 0..TABLE_SECTORS as usize {
        if !save_sector(s) {
            unsafe { MOUNTED = false };
            return Err("could not format the drive");
        }
    }
    for folder in DEFAULT_FOLDERS {
        mkdir(&format!("/{}", folder))?;
    }
    write(
        "/Documents/welcome.txt",
        b"Welcome to AnarchyFS.\nThis drive is encrypted. Use the Files app or the Terminal.\n",
    )
}

/// Turn a path into an absolute, tidy one. `cwd` is used for relative paths.
pub fn normalize(cwd: &str, path: &str) -> String {
    let joined = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("{}/{}", if cwd.is_empty() { "/" } else { cwd }, path)
    };
    let mut parts: Vec<&str> = Vec::new();
    for comp in joined.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    let mut out = String::new();
    for p in parts {
        out.push('/');
        out.push_str(p);
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

fn child(par: usize, label: &str) -> Option<usize> {
    let t = table();
    (1..t.len()).find(|&i| kind(&t[i]) != FREE && parent(&t[i]) == par && name(&t[i]) == label)
}

/// Find the inode for an absolute path.
fn resolve(path: &str) -> Option<usize> {
    let mut cur = ROOT;
    for comp in path.split('/').filter(|c| !c.is_empty()) {
        cur = child(cur, comp)?;
    }
    Some(cur)
}

fn valid_name(label: &str) -> Result<(), &'static str> {
    if label.is_empty() || label.len() > NAME_MAX || label == "." || label == ".." || label.contains('/') {
        Err("bad name")
    } else {
        Ok(())
    }
}

/// Split an absolute path into its parent path and final name.
fn split(path: &str) -> Option<(String, String)> {
    if path == "/" {
        return None;
    }
    let cut = path.rfind('/')?;
    let parent = if cut == 0 { "/".to_string() } else { path[..cut].to_string() };
    Some((parent, path[cut + 1..].to_string()))
}

fn free_inode() -> Option<usize> {
    (1..table().len()).find(|&i| kind(&table()[i]) == FREE)
}

/// First sector after every file. Space freed in the middle is not reused yet.
fn data_end() -> u64 {
    let t = table();
    t.iter()
        .filter(|e| kind(e) == FILE)
        .map(|e| start(e) + sectors_for(size(e)))
        .fold(DATA_LBA, u64::max)
}

fn require_mounted() -> Result<(), &'static str> {
    if mounted() {
        Ok(())
    } else {
        Err("no drive is open")
    }
}

pub fn exists(path: &str) -> bool {
    mounted() && resolve(&normalize("/", path)).is_some()
}

pub fn is_dir(path: &str) -> bool {
    mounted() && resolve(&normalize("/", path)).map_or(false, |i| kind(&table()[i]) == DIR)
}

/// One row of a folder listing.
#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u32,
}

/// The contents of a folder: folders first, then files, each sorted by name.
pub fn list(path: &str) -> Option<Vec<Entry>> {
    if !mounted() {
        return None;
    }
    let dir = resolve(&normalize("/", path))?;
    if kind(&table()[dir]) != DIR {
        return None;
    }
    let t = table();
    let mut out: Vec<Entry> = (1..t.len())
        .filter(|&i| kind(&t[i]) != FREE && parent(&t[i]) == dir)
        .map(|i| Entry { name: name(&t[i]).to_string(), dir: kind(&t[i]) == DIR, size: size(&t[i]) })
        .collect();
    out.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Some(out)
}

pub fn read(path: &str) -> Option<Vec<u8>> {
    require_mounted().ok()?;
    let i = resolve(&normalize("/", path))?;
    let e = table()[i];
    if kind(&e) != FILE {
        return None;
    }
    let len = size(&e) as usize;
    let mut out: Vec<u8> = Vec::with_capacity(len);
    for k in 0..sectors_for(size(&e)) {
        let mut s = [0u8; 512];
        if !volume::read(start(&e) + k, &mut s) {
            return None;
        }
        out.extend_from_slice(&s);
    }
    out.truncate(len);
    Some(out)
}

/// Create or replace a file. Its parent folder must already exist.
pub fn write(path: &str, data: &[u8]) -> Result<(), &'static str> {
    require_mounted()?;
    let p = normalize("/", path);
    let (dir_path, label) = split(&p).ok_or("bad file name")?;
    valid_name(&label)?;
    let par = resolve(&dir_path).ok_or("no such folder")?;
    if kind(&table()[par]) != DIR {
        return Err("not a folder");
    }

    let existing = child(par, &label);
    if let Some(i) = existing {
        if kind(&table()[i]) == DIR {
            return Err("that is a folder");
        }
    }
    let need = sectors_for(data.len() as u32);
    let reuse = existing.filter(|&i| need <= sectors_for(size(&table()[i])));
    let first = match reuse {
        Some(i) => start(&table()[i]),
        None => data_end(),
    };
    if first + need > volume::sectors() {
        return Err("disk full");
    }

    for k in 0..need {
        let from = k as usize * 512;
        let to = (from + 512).min(data.len());
        let mut s = [0u8; 512];
        s[..to - from].copy_from_slice(&data[from..to]);
        if !volume::write(first + k, &s) {
            return Err("disk write failed");
        }
    }

    let idx = match existing {
        Some(i) => i,
        None => free_inode().ok_or("too many files and folders")?,
    };
    fill(&mut table()[idx], FILE, par, first, data.len() as u32, &label);
    save(idx)
}

pub fn mkdir(path: &str) -> Result<(), &'static str> {
    require_mounted()?;
    let p = normalize("/", path);
    let (dir_path, label) = split(&p).ok_or("bad folder name")?;
    valid_name(&label)?;
    let par = resolve(&dir_path).ok_or("no such folder")?;
    if kind(&table()[par]) != DIR {
        return Err("not a folder");
    }
    if child(par, &label).is_some() {
        return Err("already exists");
    }
    let idx = free_inode().ok_or("too many files and folders")?;
    fill(&mut table()[idx], DIR, par, 0, 0, &label);
    save(idx)
}

fn has_children(idx: usize) -> bool {
    let t = table();
    (1..t.len()).any(|i| kind(&t[i]) != FREE && parent(&t[i]) == idx)
}

pub fn remove(path: &str) -> Result<(), &'static str> {
    require_mounted()?;
    let p = normalize("/", path);
    if p == "/" {
        return Err("cannot remove the root folder");
    }
    let idx = resolve(&p).ok_or("no such file or folder")?;
    if kind(&table()[idx]) == DIR && has_children(idx) {
        return Err("folder is not empty");
    }
    table()[idx] = [0; INODE];
    save(idx)
}

/// Move a file or folder, or rename it. A destination that is an existing
/// folder receives the item inside it.
pub fn rename(from: &str, to: &str) -> Result<(), &'static str> {
    require_mounted()?;
    let f = normalize("/", from);
    let mut t = normalize("/", to);
    if f == "/" {
        return Err("cannot move the root folder");
    }
    let src = resolve(&f).ok_or("no such file or folder")?;
    if is_dir(&t) {
        let (_, label) = split(&f).ok_or("bad name")?;
        t = if t == "/" { format!("/{}", label) } else { format!("{}/{}", t, label) };
    }
    if resolve(&t).is_some() {
        return Err("destination already exists");
    }
    if kind(&table()[src]) == DIR && (t == f || t.starts_with(&format!("{}/", f))) {
        return Err("cannot move a folder into itself");
    }
    let (dest_path, label) = split(&t).ok_or("bad name")?;
    valid_name(&label)?;
    let dest = resolve(&dest_path).ok_or("no such folder")?;
    if kind(&table()[dest]) != DIR {
        return Err("not a folder");
    }
    let e = &mut table()[src];
    e[2..4].copy_from_slice(&(dest as u16).to_le_bytes());
    e[16..16 + NAME_MAX].fill(0);
    e[16..16 + label.len()].copy_from_slice(label.as_bytes());
    save(src)
}

pub fn total_bytes() -> u64 {
    volume::sectors().saturating_sub(DATA_LBA) * 512
}

pub fn used_bytes() -> u64 {
    if !mounted() {
        return 0;
    }
    (data_end() - DATA_LBA + TABLE_SECTORS) * 512
}

/// Legacy disks (before folders and encryption) hold a flat list of files
/// in plain text. Read them out so they can be copied to the new layout.
pub fn read_legacy() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut dir = vec![0u8; 8 * 512];
    for i in 0..8 {
        let mut s = [0u8; 512];
        if !volume::raw_read(1 + i as u64, &mut s) {
            return out;
        }
        dir[i * 512..(i + 1) * 512].copy_from_slice(&s);
    }
    for k in 0..8 * 512 / 64 {
        let e = &dir[k * 64..(k + 1) * 64];
        if e[56] == 0 {
            continue;
        }
        let end = e[..48].iter().position(|&b| b == 0).unwrap_or(48);
        let label = match core::str::from_utf8(&e[..end]) {
            Ok(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };
        let first = u32::from_le_bytes([e[48], e[49], e[50], e[51]]) as u64;
        let len = u32::from_le_bytes([e[52], e[53], e[54], e[55]]) as usize;
        let mut data = Vec::with_capacity(len);
        for s in 0..sectors_for(len as u32) {
            let mut sector = [0u8; 512];
            if !volume::raw_read(first + s, &mut sector) {
                break;
            }
            data.extend_from_slice(&sector);
        }
        data.truncate(len);
        out.push((label, data));
    }
    out
}
