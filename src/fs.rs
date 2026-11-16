use crate::virtio_blk as blk;
use alloc::vec;
use alloc::vec::Vec;

const MAGIC: &[u8; 8] = b"ANARCHFS";
const DIR_SECTORS: u64 = 8;
const FIRST_DATA: u64 = 1 + DIR_SECTORS;
const ENTRY: usize = 64;
const MAX_FILES: usize = (DIR_SECTORS as usize * 512) / ENTRY;
const NAME_MAX: usize = 48;

fn sectors_for(size: u32) -> u64 {
    (size as u64 + 511) / 512
}

fn load_dir() -> Option<Vec<u8>> {
    let mut dir = vec![0u8; DIR_SECTORS as usize * 512];
    for i in 0..DIR_SECTORS as usize {
        let mut s = [0u8; 512];
        if !blk::read(1 + i as u64, &mut s) {
            return None;
        }
        dir[i * 512..(i + 1) * 512].copy_from_slice(&s);
    }
    Some(dir)
}

fn save_dir(dir: &[u8]) -> bool {
    for i in 0..DIR_SECTORS as usize {
        let mut s = [0u8; 512];
        s.copy_from_slice(&dir[i * 512..(i + 1) * 512]);
        if !blk::write(1 + i as u64, &s) {
            return false;
        }
    }
    true
}

fn slot(dir: &[u8], i: usize) -> &[u8] {
    &dir[i * ENTRY..(i + 1) * ENTRY]
}

fn slot_used(e: &[u8]) -> bool {
    e[56] != 0
}

fn slot_name(e: &[u8]) -> &str {
    let end = e[..NAME_MAX].iter().position(|&b| b == 0).unwrap_or(NAME_MAX);
    core::str::from_utf8(&e[..end]).unwrap_or("?")
}

fn slot_start(e: &[u8]) -> u64 {
    u32::from_le_bytes([e[48], e[49], e[50], e[51]]) as u64
}

fn slot_size(e: &[u8]) -> u32 {
    u32::from_le_bytes([e[52], e[53], e[54], e[55]])
}

fn find(dir: &[u8], name: &str) -> Option<usize> {
    (0..MAX_FILES).find(|&i| {
        let e = slot(dir, i);
        slot_used(e) && slot_name(e) == name
    })
}

fn set_slot(dir: &mut [u8], i: usize, name: &str, start: u64, size: u32) {
    let e = &mut dir[i * ENTRY..(i + 1) * ENTRY];
    e.fill(0);
    e[..name.len()].copy_from_slice(name.as_bytes());
    e[48..52].copy_from_slice(&(start as u32).to_le_bytes());
    e[52..56].copy_from_slice(&size.to_le_bytes());
    e[56] = 1;
}

fn next_free(dir: &[u8]) -> u64 {
    let mut next = FIRST_DATA;
    for i in 0..MAX_FILES {
        let e = slot(dir, i);
        if slot_used(e) {
            next = next.max(slot_start(e) + sectors_for(slot_size(e)));
        }
    }
    next
}

pub fn init() -> bool {
    if !blk::ready() {
        return false;
    }
    let mut sb = [0u8; 512];
    if !blk::read(0, &mut sb) {
        return false;
    }
    if sb[..8] == MAGIC[..] {
        kprintln!("fs: mounted");
        return true;
    }

    kprintln!("fs: formatting new disk");
    let mut sb = [0u8; 512];
    sb[..8].copy_from_slice(MAGIC);
    if !blk::write(0, &sb) {
        return false;
    }
    let zero = vec![0u8; DIR_SECTORS as usize * 512];
    if !save_dir(&zero) {
        return false;
    }
    write("welcome.txt", b"Welcome to AnarchyFS.\nUse ls, cat and write in the terminal.\n").is_ok()
}

pub fn list(mut f: impl FnMut(&str, u32)) {
    let dir = match load_dir() {
        Some(d) => d,
        None => return,
    };
    for i in 0..MAX_FILES {
        let e = slot(&dir, i);
        if slot_used(e) {
            f(slot_name(e), slot_size(e));
        }
    }
}

pub fn read(name: &str) -> Option<Vec<u8>> {
    let dir = load_dir()?;
    let i = find(&dir, name)?;
    let e = slot(&dir, i);
    let start = slot_start(e);
    let size = slot_size(e);
    let mut out: Vec<u8> = Vec::with_capacity(size as usize);
    for k in 0..sectors_for(size) {
        let mut s = [0u8; 512];
        if !blk::read(start + k, &mut s) {
            return None;
        }
        out.extend_from_slice(&s);
    }
    out.truncate(size as usize);
    Some(out)
}

pub fn write(name: &str, data: &[u8]) -> Result<(), &'static str> {
    if name.is_empty() || name.len() > NAME_MAX {
        return Err("bad file name");
    }
    let mut dir = load_dir().ok_or("disk read failed")?;
    let need = sectors_for(data.len() as u32);
    let existing = find(&dir, name);

    let (slot_i, start) = match existing {
        Some(i) if need <= sectors_for(slot_size(slot(&dir, i))) => {
            (i, slot_start(slot(&dir, i)))
        }
        Some(i) => (i, next_free(&dir)),
        None => {
            let i = (0..MAX_FILES)
                .find(|&i| !slot_used(slot(&dir, i)))
                .ok_or("directory full")?;
            (i, next_free(&dir))
        }
    };

    if start + need > blk::sectors() {
        return Err("disk full");
    }

    for k in 0..need {
        let from = k as usize * 512;
        let to = (from + 512).min(data.len());
        let mut s = [0u8; 512];
        s[..to - from].copy_from_slice(&data[from..to]);
        if !blk::write(start + k, &s) {
            return Err("disk write failed");
        }
    }

    set_slot(&mut dir, slot_i, name, start, data.len() as u32);
    if !save_dir(&dir) {
        return Err("directory write failed");
    }
    Ok(())
}
