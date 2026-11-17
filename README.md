# AnarchyOS

An operating system written from scratch in Rust: its own kernel, drivers,
window manager and desktop. It boots on x86_64 through the Limine bootloader.

## What is in it

- **Kernel:** memory management, interrupts, a preemptive scheduler, a heap.
- **Drivers:** PS/2 keyboard and mouse, virtio disk and network, the CMOS clock.
- **Storage:** AnarchyFS v2, a folder-based filesystem on an encrypted drive.
  The drive is ChaCha20 encrypted, and its key comes from a passphrase.
- **Desktop:** frosted-glass windows, a menu bar with the date and time, a
  dock, an app drawer, the Files app, a Terminal, and a Settings app.
- **AnarchyAi:** a native version of the AnarchyAi web GUI, with the same
  header (Local, Hybrid, Cloud and a context meter), sidebar and chat. It has
  no language model yet, because AnarchyOS has no network stack. Sections
  that need the Python backend show a notice. Its `remember`, `recall` and
  `forget` commands keep notes on the encrypted drive in `/Documents/AnarchyAi`.

## Building and running (macOS)

One-time setup:

```sh
brew install qemu xorriso rustup
rustup-init -y
git clone https://github.com/limine-bootloader/limine.git --branch=v8.x-binary --depth=1 third_party/limine
make -C third_party/limine
```

Then:

```sh
make run
```

The first build takes a minute or two longer than later ones, because it
also builds the tools that prepare the artwork and fonts.

## The encrypted drive

The first time you boot onto a blank drive, AnarchyOS asks you to create a
passphrase (at least 8 characters). Every later boot asks for it before the
desktop appears.

- Sector 0 of the disk holds a small plain header with a random salt and a
  check value. The passphrase is stretched with PBKDF2-HMAC-SHA256 (20,000
  rounds) into an encryption key and a check key.
- Every other sector is encrypted with ChaCha20, using its sector number as
  the nonce, so the disk never holds readable file names or contents.
- Lock the drive at any time with **Settings > Storage > Lock the drive**, or
  with `lock` in the Terminal.
- There is no reset. A forgotten passphrase means the data is gone.

The crypto code is written from the published specifications and checked
against their test vectors (`cargo test` in a crate that includes
`src/crypto.rs`). It has not been audited, so treat it as a learning project,
not a safe place for real secrets.

A disk from before encryption existed is moved across automatically: after
you create a passphrase, its files appear in the root folder.

To start with a fresh, larger disk (32 MiB), run `make reset-disk` and then
`make run`. The disk file is created with 32 MiB by the `disk` target.

## Folders and files

AnarchyFS has real folders. A new drive gets `Applications`, `Desktop`,
`Documents`, `Downloads`, `Music`, `Pictures`, `System` and `Videos`.
Settings are kept in `/System/settings.cfg`.

Terminal commands:

| Command | What it does |
| --- | --- |
| `pwd`, `cd <dir>`, `ls [dir]` | Where you are, move, list. |
| `mkdir <dir>`, `rm <path>` | Make a folder, or remove a file or an empty folder. |
| `mv <from> <to>` | Move or rename. A folder destination takes the item inside it. |
| `cat <file>`, `write <file> <text>` | Read and write files. |
| `drive`, `lock` | Show the drive's state, or lock it. |
| `run hello`, `run <file>` | Run a static Linux x86_64 program in ring 3. `hello` is built into the kernel. |

The Files app shows the same folders. Click a folder to open it, a file to
select it, **Up** to go back, and **New folder** to make one.

## Icons: adding folder and app icons

Full-colour icons are picked up automatically when the kernel is built. Drop
an image into one of these folders and run `make run`:

| Folder | Used for |
| --- | --- |
| `assets/icons/folders/` | Folder icons in the Files app. A folder called `Music` uses `Music.png` (or `Music.ico`). Matching is case-insensitive. |
| `assets/icons/apps/` | App icons in the dock and app drawer. |

PNG, JPEG, BMP and ICO files all work. Each icon is named after its file
name without the extension, so `Finder.png` is called `Finder`. Icons are
resized to 48 and 64 pixels when the kernel is built.

To give an app an icon:

1. Drop the image into `assets/icons/apps/`, for example `Chrome.png`.
2. In `src/wm.rs`, set that app's `art` field in `APPS` to `"Chrome"`.

Apps without an `art` name use the built-in gradient tile with a white glyph.

## Artwork, wallpapers and fonts

Everything in `assets/` is baked into the kernel when it is built. `build.rs`
does the converting, so the kernel needs no image or font decoder.

| Path | Used for |
| --- | --- |
| `assets/wallpaper/` | Every PNG or JPEG here shows up in Settings > Wallpaper. |
| `assets/AnarchyAI-Logo.png` | The Anarchy "A" in the top-left of the menu bar. |
| `assets/App-Drawer.png` | The app drawer button in the dock. |
| `assets/AnarchyOs-logo.png` | The wordmark on the Welcome and lock screens and in Settings > About. |
| `assets/icons/folders/` | Folder icons (see above). |
| `assets/icons/apps/` | App icons (see above). |
| `assets/fonts/` | Interface fonts and the icon font. |

## Programs: static Linux binaries on the drive

AnarchyOS runs static Linux x86_64 programs in ring 3. The kernel implements
the Linux system calls they use (file I/O, `brk`, anonymous `mmap`, `uname`,
and a few more). Programs must be static and built for musl.

On the build machine, install a musl cross compiler (for example
`brew install filosottile/musl-cross/musl-cross`), then:

```sh
make userland                      # builds userland/hello with musl
ANARCHY_PASS='your passphrase' make disk-programs
```

`disk-programs` writes a new encrypted drive to `build/disk.img` with the
programs inside: `/Applications/hello` and `/Documents/hello.txt`. It never
overwrites a drive that already holds data; run `make reset-disk` first to
start over. Then `make run`, unlock with the same passphrase, and in the
Terminal type `run /Applications/hello`.

`run hello` (without a path) runs a tiny program built into the kernel.

## Network

The network card driver is virtio-net, which QEMU emulates. At boot the
system asks for an address by DHCP, and it falls back to QEMU's usual
addresses if nothing answers. The Terminal commands:

| Command | What it does |
| --- | --- |
| `net` | Show the link, address, gateway, DNS server and ARP table. |
| `dhcp` | Ask for an address again. |
| `ping a.b.c.d` | Send ICMP echo requests. |
| `dns <name>` | Look up an IPv4 address. |
| `fetch http://host/path /Documents/file` | Download a page into a file on the drive. |

Limits for now: plain HTTP only (no HTTPS yet, so most package downloads
will not work), one TCP connection at a time, and no Wi-Fi or Bluetooth.
The protocol code has tests. Run `make test-net` on the build machine.

## The dock

Pinned apps stay on the dock across restarts. An app that is open but not
pinned appears at the end of the dock until you close it.

- **Open** an app by clicking its icon. Click the app in front to tuck it away.
- **Remove** an app from the dock by dragging its icon up, off the dock, and
  letting go. The app keeps running if it is open.
- **Right-click** an icon for a menu: Open or Hide, Keep in dock or Remove from
  dock, and Close window.

The pinned list is saved on the drive as `dock` in `/System/settings.cfg`.

## Rice: window styling and desktop mods

Settings has a **Rice** group with three pages:

- **Window Styling**: border width and colour, corner radius, shadows and
  their strength, blur, an accent tint in the window glass, and a glow around
  the focused window.
- **Bars & Glass**: how solid the menu bar and the dock are.
- **Rice Mods**: scanlines, a vignette, film grain and an accent wash over
  the wallpaper. These are baked into the desktop picture when it is set up,
  so they cost nothing while drawing.

Dock & Menu Bar holds the effects: the dock bounces when an app opens, lifts
the icon under the pointer, and magnifies icons near it. The menu bar can
highlight the part under the pointer.

## Where things are

| File | What it does |
| --- | --- |
| `src/main.rs` | Boot sequence and the main event loop. |
| `src/wm.rs` | Windows, menu bar, dock and app drawer. |
| `src/lock.rs` | The passphrase screen, and first-time setup. |
| `src/volume.rs` | The encrypted volume: header, unlock, and sector encryption. |
| `src/crypto.rs` | SHA-256, HMAC, PBKDF2 and ChaCha20. |
| `src/fs.rs` | AnarchyFS: folders, files, and the inode table. |
| `src/files_app.rs` | The Files app. |
| `src/terminal.rs` | The Terminal app and its commands. |
| `src/settings_app.rs` | The Settings app and its pages. |
| `src/ui.rs` | Switches, sliders, cards and other widgets. |
| `src/gfx.rs` | Drawing: rounded shapes, blending, images, text. |
| `src/theme.rs` | Colours and the options Settings can change. |
| `src/scene.rs` | The wallpaper and its blurred copy for glass windows. |

## Credits

- [Inter](https://rsms.me/inter/) and [JetBrains Mono](https://www.jetbrains.com/lp/mono/),
  both under the SIL Open Font License.
- [Lucide](https://lucide.dev) icons, under the ISC License.
- [Limine](https://github.com/limine-bootloader/limine) bootloader.

Licence texts for the fonts and icons are in `assets/fonts/`. The folder and
app icons in `assets/icons/` come from third-party icon packs; check their
licences before you redistribute the project.
