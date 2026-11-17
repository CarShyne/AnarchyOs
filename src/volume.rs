//! The encrypted volume. Sector 0 is a small plain header that says how to
//! check a passphrase. Every other sector is scrambled with ChaCha20 under a
//! key derived from the passphrase, using the sector number as the nonce, so
//! identical sectors never look the same on the disk.

use crate::crypto;
use crate::virtio_blk as blk;

const MAGIC: &[u8; 8] = b"ANARCHV2";
pub const LEGACY_MAGIC: &[u8; 8] = b"ANARCHFS";
const VERSION: u32 = 2;
/// Key stretching cost. Each step is an HMAC, so this is what makes guessing slow.
pub const ITERATIONS: u32 = 20_000;
const VERIFY: &[u8] = b"anarchyos volume check";

#[derive(Clone, Copy, PartialEq)]
pub enum State {
    /// No drive is attached, so there is nothing to lock or unlock.
    NoDisk,
    /// A blank disk. A passphrase has to be chosen before anything is stored.
    Blank,
    /// A disk from before encryption existed. Its files can be moved across.
    Legacy,
    Locked,
    Open,
}

struct Volume {
    state: State,
    key: [u8; 32],
}

static mut VOL: Volume = Volume { state: State::NoDisk, key: [0; 32] };

fn vol() -> &'static mut Volume {
    unsafe { &mut *core::ptr::addr_of_mut!(VOL) }
}

/// Look at sector 0 and decide what the screen should ask for.
pub fn probe() -> State {
    let v = vol();
    v.key = [0; 32];
    v.state = if !blk::ready() {
        State::NoDisk
    } else {
        let mut s = [0u8; 512];
        if !blk::read(0, &mut s) {
            State::NoDisk
        } else if &s[..8] == MAGIC {
            State::Locked
        } else if &s[..8] == LEGACY_MAGIC {
            State::Legacy
        } else {
            State::Blank
        }
    };
    v.state
}

pub fn state() -> State {
    vol().state
}

pub fn is_open() -> bool {
    vol().state == State::Open
}

fn nonce(lba: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..8].copy_from_slice(&lba.to_le_bytes());
    n
}

fn make_salt() -> [u8; 16] {
    let tsc = unsafe { core::arch::x86_64::_rdtsc() };
    let mut seed = [0u8; 16];
    seed[..8].copy_from_slice(&tsc.to_le_bytes());
    seed[8..].copy_from_slice(&crate::pit::ticks().to_le_bytes());
    let digest = crypto::sha256(&seed);
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&digest[..16]);
    salt
}

fn check_value(mac_key: &[u8]) -> [u8; 32] {
    crypto::hmac(mac_key, &[VERIFY])
}

/// Unlock with a passphrase. Returns false if it is wrong.
pub fn unlock(pass: &[u8]) -> bool {
    if vol().state != State::Locked {
        return false;
    }
    let mut h = [0u8; 512];
    if !blk::read(0, &mut h) {
        return false;
    }
    let iters = u32::from_le_bytes([h[12], h[13], h[14], h[15]]);
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&h[16..32]);
    let dk = crypto::pbkdf2(pass, &salt, iters);
    let expected = check_value(&dk[32..]);
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= expected[i] ^ h[32 + i];
    }
    if diff != 0 {
        return false;
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&dk[..32]);
    let v = vol();
    v.key = key;
    v.state = State::Open;
    true
}

/// Write a new header with a passphrase and open the volume. The caller
/// must have copied anything it needs from a legacy disk first, because
/// the header and the layout that follows it replace the old contents.
pub fn create(pass: &[u8]) -> Result<(), &'static str> {
    if !blk::ready() {
        return Err("no drive attached");
    }
    let salt = make_salt();
    let dk = crypto::pbkdf2(pass, &salt, ITERATIONS);
    let mut h = [0u8; 512];
    h[..8].copy_from_slice(MAGIC);
    h[8..12].copy_from_slice(&VERSION.to_le_bytes());
    h[12..16].copy_from_slice(&ITERATIONS.to_le_bytes());
    h[16..32].copy_from_slice(&salt);
    h[32..64].copy_from_slice(&check_value(&dk[32..]));
    if !blk::write(0, &h) {
        return Err("could not write the drive header");
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&dk[..32]);
    let v = vol();
    v.key = key;
    v.state = State::Open;
    Ok(())
}

/// Forget the key. Nothing on the drive can be read until the next unlock.
pub fn lock() {
    let v = vol();
    if v.state == State::Open {
        v.key = [0; 32];
        v.state = State::Locked;
    }
}

/// Read a sector through the cipher. Sector 0 is never encrypted.
pub fn read(lba: u64, buf: &mut [u8; 512]) -> bool {
    if lba == 0 {
        return blk::read(0, buf);
    }
    let v = vol();
    if v.state != State::Open || !blk::read(lba, buf) {
        return false;
    }
    crypto::chacha20_xor(&v.key, &nonce(lba), 0, buf);
    true
}

pub fn write(lba: u64, buf: &[u8; 512]) -> bool {
    if lba == 0 {
        return blk::write(0, buf);
    }
    let v = vol();
    if v.state != State::Open {
        return false;
    }
    let mut tmp = *buf;
    crypto::chacha20_xor(&v.key, &nonce(lba), 0, &mut tmp);
    blk::write(lba, &tmp)
}

/// Read a sector without decryption, for importing a legacy disk.
pub fn raw_read(lba: u64, buf: &mut [u8; 512]) -> bool {
    blk::read(lba, buf)
}

pub fn sectors() -> u64 {
    blk::sectors()
}

pub fn iterations_on_disk() -> u32 {
    let mut h = [0u8; 512];
    if blk::read(0, &mut h) {
        u32::from_le_bytes([h[12], h[13], h[14], h[15]])
    } else {
        0
    }
}
