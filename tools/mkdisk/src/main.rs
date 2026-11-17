//! Builds an encrypted AnarchyFS drive image on the build machine with files
//! already inside it. It compiles the kernel's own crypto, volume and
//! filesystem code, so the image is exactly what the kernel expects.
//!
//! usage: mkdisk <image> <path-in-drive> <host-file> [<path> <host-file> ...]
//! The passphrase is read from the ANARCHY_PASS environment variable.
//! An image that already holds data is never overwritten.

#![allow(dead_code, unused_unsafe, static_mut_refs)]
extern crate alloc;

#[macro_export]
macro_rules! kprintln {
    ($($a:tt)*) => { eprintln!($($a)*) };
}

#[path = "../../../src/crypto.rs"]
mod crypto;
#[path = "../../../src/volume.rs"]
mod volume;
#[path = "../../../src/fs.rs"]
mod fs;

/// The drive is kept in memory while the tool works, then written out.
mod virtio_blk {
    pub static mut DISK: Vec<u8> = Vec::new();

    pub fn ready() -> bool {
        true
    }

    pub fn sectors() -> u64 {
        unsafe { (DISK.len() / 512) as u64 }
    }

    pub fn read(lba: u64, buf: &mut [u8; 512]) -> bool {
        let at = lba as usize * 512;
        unsafe {
            if at + 512 > DISK.len() {
                return false;
            }
            buf.copy_from_slice(&DISK[at..at + 512]);
        }
        true
    }

    pub fn write(lba: u64, buf: &[u8; 512]) -> bool {
        let at = lba as usize * 512;
        unsafe {
            if at + 512 > DISK.len() {
                return false;
            }
            DISK[at..at + 512].copy_from_slice(buf);
        }
        true
    }
}

/// The volume salts its keys with the clock, so the tool uses the wall clock too.
mod pit {
    pub fn ticks() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }
}

const DRIVE_BYTES: usize = 32 << 20;

fn die(message: &str) -> ! {
    eprintln!("mkdisk: {}", message);
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 || args.len() % 2 != 1 {
        die("usage: mkdisk <image> <path-in-drive> <host-file> [...]");
    }
    let image = &args[0];
    let pass = std::env::var("ANARCHY_PASS")
        .unwrap_or_else(|_| die("set ANARCHY_PASS to the passphrase for the new drive"));
    if pass.len() < 8 {
        die("the passphrase must be at least 8 characters");
    }

    if let Ok(existing) = std::fs::read(image) {
        if existing.iter().any(|&b| b != 0) {
            die(&format!("{} already holds data, so it was not touched", image));
        }
    }

    // Read every host file before touching anything, so a typo fails early.
    let mut files = Vec::new();
    for pair in args[1..].chunks(2) {
        let data = std::fs::read(&pair[1]).unwrap_or_else(|e| die(&format!("{}: {}", pair[1], e)));
        files.push((pair[0].clone(), data));
    }

    unsafe { virtio_blk::DISK = vec![0u8; DRIVE_BYTES] };
    volume::create(pass.as_bytes()).unwrap_or_else(|e| die(e));
    fs::format().unwrap_or_else(|e| die(e));
    for (dest, data) in &files {
        fs::write(dest, data).unwrap_or_else(|e| die(&format!("{}: {}", dest, e)));
        eprintln!("added {} ({} bytes)", dest, data.len());
    }

    // Check the image the way the kernel will use it: lock, unlock, read back.
    volume::lock();
    fs::unmount();
    if !volume::unlock(pass.as_bytes()) {
        die("the new drive did not unlock with the passphrase");
    }
    fs::mount();
    for (dest, data) in &files {
        match fs::read(dest) {
            Some(back) if back == *data => {}
            _ => die(&format!("{} did not read back correctly", dest)),
        }
    }

    let bytes = unsafe { &virtio_blk::DISK };
    std::fs::write(image, bytes).unwrap_or_else(|e| die(&format!("{}: {}", image, e)));
    eprintln!("wrote {} ({} MiB)", image, DRIVE_BYTES >> 20);
}
