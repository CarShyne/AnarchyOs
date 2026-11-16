# AnarchyOS

An operating system written from scratch in Rust: its own kernel, drivers,
window manager and desktop. It boots on x86_64 through the Limine bootloader.

## What is in it

- **Kernel:** memory management, interrupts, a preemptive scheduler, a heap.
- **Drivers:** PS/2 keyboard and mouse, virtio disk and network, the CMOS clock.
- **Storage:** AnarchyFS, a small flat filesystem. Settings are saved to it.
- **Desktop:** frosted-glass windows, a menu bar with the date and time, a
  dock, an app drawer, a Terminal, and a Settings app.

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

## Artwork, wallpapers and fonts

Everything in `assets/` is baked into the kernel when it is built. `build.rs`
does the converting, so the kernel needs no image or font decoder.

| Path | Used for |
| --- | --- |
| `assets/wallpaper/` | Every PNG or JPEG here shows up in Settings > Wallpaper. |
| `assets/App-Drawer.png` | The logo in the menu bar and the app drawer button. |
| `assets/AnarchyOs-logo.png` | The wordmark in the Welcome window and Settings > About. |
| `assets/fonts/` | Interface fonts and the icon font. |

To add a wallpaper, drop the picture into `assets/wallpaper/` and run
`make run` again.

## Where things are

| File | What it does |
| --- | --- |
| `src/main.rs` | Boot sequence and the main event loop. |
| `src/wm.rs` | Windows, menu bar, dock and app drawer. |
| `src/settings_app.rs` | The Settings app and its pages. |
| `src/ui.rs` | Switches, sliders, cards and other widgets. |
| `src/gfx.rs` | Drawing: rounded shapes, blending, images, text. |
| `src/theme.rs` | Colours and the options Settings can change. |
| `src/scene.rs` | The wallpaper and its blurred copy for glass windows. |
| `src/terminal.rs` | The Terminal app and its commands. |

## Credits

- [Inter](https://rsms.me/inter/) and [JetBrains Mono](https://www.jetbrains.com/lp/mono/),
  both under the SIL Open Font License.
- [Lucide](https://lucide.dev) icons, under the ISC License.
- [Limine](https://github.com/limine-bootloader/limine) bootloader.

Licence texts for the fonts and icons are in `assets/fonts/`.
