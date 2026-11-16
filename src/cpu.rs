//! Asks the processor to identify itself.

use alloc::string::String;
use core::arch::x86_64::__cpuid;
use core::ptr::addr_of_mut;

static mut BRAND: Option<String> = None;

/// The marketing name of the CPU, for example "Intel(R) Core(TM) i7-9700".
/// Asked once, then remembered.
pub fn brand() -> &'static str {
    // SAFETY: only the main loop calls this.
    let slot = unsafe { &mut *addr_of_mut!(BRAND) };
    slot.get_or_insert_with(query).as_str()
}

fn query() -> String {
    let max_ext = unsafe { __cpuid(0x8000_0000) }.eax;
    let mut bytes: [u8; 48] = [0; 48];
    let mut n = 0;

    if max_ext >= 0x8000_0004 {
        for leaf in 0x8000_0002u32..=0x8000_0004 {
            let r = unsafe { __cpuid(leaf) };
            for reg in [r.eax, r.ebx, r.ecx, r.edx] {
                bytes[n..n + 4].copy_from_slice(&reg.to_le_bytes());
                n += 4;
            }
        }
    } else {
        let r = unsafe { __cpuid(0) };
        for reg in [r.ebx, r.edx, r.ecx] {
            bytes[n..n + 4].copy_from_slice(&reg.to_le_bytes());
            n += 4;
        }
    }

    let mut out = String::new();
    let mut last_space = true;
    for &b in &bytes[..n] {
        if b == 0 {
            break;
        }
        let c = if (33..=126).contains(&b) { b as char } else { ' ' };
        // Collapse the padding some vendors put in the name.
        if c == ' ' && last_space {
            continue;
        }
        last_space = c == ' ';
        out.push(c);
    }
    let trimmed = out.trim_end();
    if trimmed.is_empty() {
        String::from("x86_64 processor")
    } else {
        String::from(trimmed)
    }
}
