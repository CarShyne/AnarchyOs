//! The passphrase screen. It appears at boot when the drive is locked, when
//! a blank disk needs a passphrase, and after "lock" in the Terminal.

use crate::framebuffer as fb;
use crate::fs;
use crate::gfx;
use crate::image::{BODY, LABEL, SMALL, TITLE, WORDMARK};
use crate::scene;
use crate::theme::{self, FAINT, MUTED, TEXT, WHITE};
use crate::ui::Rect;
use crate::volume::{self, State as Volume};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ptr::addr_of_mut;

const MIN_LEN: usize = 8;
const MAX_LEN: usize = 64;
const RED: u32 = 0x00FF_5F57;
const CARD_W: i32 = 460;
const CARD_H: i32 = 350;

#[derive(Clone, Copy, PartialEq)]
pub enum Stage {
    Unlock,
    Create,
    Confirm,
}

/// What happened when the passphrase was submitted.
#[derive(PartialEq)]
pub enum Outcome {
    Unlocked,
    /// A new passphrase was typed once and is waiting for its confirmation.
    Advanced,
    Failed,
}

struct Lock {
    stage: Stage,
    typed: String,
    first: String,
    message: &'static str,
    busy: bool,
}

static mut LOCK: Lock = Lock {
    stage: Stage::Unlock,
    typed: String::new(),
    first: String::new(),
    message: "",
    busy: false,
};

fn lock() -> &'static mut Lock {
    unsafe { &mut *addr_of_mut!(LOCK) }
}

/// Whether the passphrase screen should cover the desktop right now.
pub fn showing() -> bool {
    matches!(volume::state(), Volume::Locked | Volume::Blank | Volume::Legacy)
}

fn stage() -> Stage {
    let l = lock();
    match volume::state() {
        Volume::Locked => Stage::Unlock,
        _ if l.stage == Stage::Unlock => Stage::Create,
        _ => l.stage,
    }
}

/// Add or remove a typed character. Returns true when Enter was pressed.
pub fn key(c: u8) -> bool {
    let l = lock();
    if l.busy {
        return false;
    }
    match c {
        b'\n' | b'\r' => true,
        0x08 => {
            l.typed.pop();
            l.message = "";
            false
        }
        0x20..=0x7E => {
            if l.typed.len() < MAX_LEN {
                l.typed.push(c as char);
            }
            l.message = "";
            false
        }
        _ => false,
    }
}

pub fn set_busy(busy: bool) {
    lock().busy = busy;
}

/// Act on the typed passphrase. Key stretching takes a moment, so the
/// window manager draws a "Working" frame before calling this.
pub fn submit() -> Outcome {
    let l = lock();
    let pass = core::mem::take(&mut l.typed);
    match stage() {
        Stage::Unlock => {
            if volume::unlock(pass.as_bytes()) && fs::mount() {
                l.stage = Stage::Unlock;
                l.message = "";
                Outcome::Unlocked
            } else {
                l.message = "That passphrase is not right.";
                Outcome::Failed
            }
        }
        Stage::Create => {
            if pass.len() < MIN_LEN {
                l.message = "Use at least 8 characters.";
                return Outcome::Failed;
            }
            l.first = pass;
            l.stage = Stage::Confirm;
            l.message = "";
            Outcome::Advanced
        }
        Stage::Confirm => {
            if pass != l.first {
                l.first.clear();
                l.stage = Stage::Create;
                l.message = "The two passphrases did not match. Start again.";
                return Outcome::Failed;
            }
            match setup(&pass) {
                Ok(()) => {
                    l.first.clear();
                    l.stage = Stage::Unlock;
                    l.message = "";
                    Outcome::Unlocked
                }
                Err(e) => {
                    l.message = e;
                    Outcome::Failed
                }
            }
        }
    }
}

/// Encrypt a blank disk, or move a legacy one into the encrypted layout.
fn setup(pass: &str) -> Result<(), &'static str> {
    let legacy: Vec<(String, Vec<u8>)> =
        if volume::state() == Volume::Legacy { fs::read_legacy() } else { Vec::new() };
    volume::create(pass.as_bytes())?;
    fs::format()?;
    for (name, data) in legacy {
        let _ = fs::write(&format!("/{}", name), &data);
    }
    Ok(())
}

/// Lock the drive now: forget the key and the inode table.
pub fn lock_now() -> bool {
    if !volume::is_open() {
        return false;
    }
    volume::lock();
    fs::unmount();
    let l = lock();
    l.stage = Stage::Unlock;
    l.typed.clear();
    l.first.clear();
    l.message = "";
    true
}

pub fn draw(screen: Rect) {
    scene::draw_background();
    let cx = screen.w / 2;
    let cy = screen.h / 2;
    let card = Rect::new(cx - CARD_W / 2, cy - CARD_H / 2, CARD_W, CARD_H);
    let c = theme::cfg();
    gfx::shadow(card.x, card.y, card.w, card.h, 22, 40);
    gfx::round_frost(
        card.x,
        card.y,
        card.w,
        card.h,
        22,
        theme::WINDOW,
        theme::alpha(c.window_opacity),
        scene::frost(),
        fb::width() as i32,
    );
    gfx::stroke(card.x, card.y, card.w, card.h, 22, WHITE, theme::EDGE_A);

    let mut y = card.y + 26;
    gfx::image_a(cx - WORDMARK.w as i32 / 2, y, &WORDMARK, 255);
    y += WORDMARK.h as i32 + 16;

    let l = lock();
    let (title, lines) = match stage() {
        Stage::Unlock => ("Unlock your drive", ["Enter the passphrase to open your files.", ""]),
        Stage::Create if volume::state() == Volume::Legacy => (
            "Create a passphrase",
            ["Your old files are moved onto the encrypted drive.", "Keep this passphrase safe. There is no reset."],
        ),
        Stage::Create => (
            "Create a passphrase",
            ["It encrypts everything on the drive. There is no reset,", "so keep it somewhere safe."],
        ),
        Stage::Confirm => ("Confirm your passphrase", ["Type the same passphrase once more.", ""]),
    };
    gfx::text_center(cx, y, &TITLE, title, TEXT);
    y += TITLE.height + 8;
    for line in lines {
        if !line.is_empty() {
            gfx::text_center(cx, y, &BODY, line, MUTED);
            y += BODY.height + 2;
        }
    }

    let field = Rect::new(card.x + 44, y + 12, card.w - 88, 46);
    gfx::round_a(field.x, field.y, field.w, field.h, 12, WHITE, theme::CARD_A);
    gfx::stroke(field.x, field.y, field.w, field.h, 12, WHITE, theme::LINE_A);
    let shown = l.typed.len().min(28);
    for i in 0..shown {
        gfx::round(field.x + 20 + i as i32 * 13, field.y + 19, 8, 8, 4, TEXT);
    }
    if l.typed.is_empty() {
        gfx::text(field.x + 20, field.y + (field.h - LABEL.height) / 2, &LABEL, "passphrase", FAINT);
    } else {
        let caret_x = field.x + 20 + shown as i32 * 13;
        gfx::rect(caret_x, field.y + 12, 2, field.h - 24, theme::accent());
    }

    let message = if l.busy {
        "Working..."
    } else if !l.message.is_empty() {
        l.message
    } else {
        ""
    };
    let message_color = if l.busy { MUTED } else { RED };
    gfx::text_center(cx, field.y + field.h + 12, &SMALL, message, message_color);

    let hint = match stage() {
        Stage::Unlock => "Press Enter to unlock",
        Stage::Create => "Press Enter to continue",
        Stage::Confirm => "Press Enter to create the drive",
    };
    gfx::text_center(cx, card.y + card.h - 30, &SMALL, hint, FAINT);
}
