KERNEL  := target/x86_64-unknown-none/debug/anarchy-kernel
ISO_DIR := build/iso_root
ISO     := build/anarchyos.iso
LIMINE  := third_party/limine
DISK    := build/disk.img

MUSL_CC ?= x86_64-linux-musl-gcc

.PHONY: all kernel iso disk run clean reset-disk userland disk-programs

all: iso

kernel:
	cargo build

iso: kernel
	rm -rf $(ISO_DIR) && mkdir -p $(ISO_DIR)/boot/limine $(ISO_DIR)/EFI/BOOT
	cp $(KERNEL) $(ISO_DIR)/boot/anarchy-kernel
	cp limine.conf $(ISO_DIR)/boot/limine/limine.conf
	cp $(LIMINE)/limine-bios.sys $(LIMINE)/limine-bios-cd.bin \
	   $(LIMINE)/limine-uefi-cd.bin $(ISO_DIR)/boot/limine/
	cp $(LIMINE)/BOOTX64.EFI $(ISO_DIR)/EFI/BOOT/
	xorriso -as mkisofs -b boot/limine/limine-bios-cd.bin \
		-no-emul-boot -boot-load-size 4 -boot-info-table \
		--efi-boot boot/limine/limine-uefi-cd.bin \
		-efi-boot-part --efi-boot-image --protective-msdos-label \
		$(ISO_DIR) -o $(ISO)
	$(LIMINE)/limine bios-install $(ISO)

disk:
	@mkdir -p build
	@[ -f $(DISK) ] || dd if=/dev/zero of=$(DISK) bs=1m count=1 2>/dev/null

run: iso disk
	qemu-system-x86_64 -M q35 -m 512M -cdrom $(ISO) \
		-drive file=$(DISK),if=none,format=raw,id=disk0 \
		-device virtio-blk-pci,drive=disk0,disable-modern=on \
		-netdev user,id=net0 -device virtio-net-pci,netdev=net0,disable-modern=on \
		-rtc base=localtime \
		-display cocoa,zoom-to-fit=on -serial stdio -no-reboot -no-shutdown

clean:
	cargo clean
	rm -rf $(ISO_DIR) $(ISO)

reset-disk:
	rm -f $(DISK)

# Static programs for the drive. Needs a musl cross compiler on the build
# machine, for example: brew install filosottile/musl-cross/musl-cross
userland:
	@mkdir -p userland/build
	$(MUSL_CC) -static -O2 -Wall -o userland/build/hello userland/hello/hello.c

# A new drive with the programs already inside, encrypted with ANARCHY_PASS.
# It never overwrites a drive that already holds data: run reset-disk first.
disk-programs: userland disk
	@[ -n "$$ANARCHY_PASS" ] || { echo "set ANARCHY_PASS to the drive passphrase (at least 8 characters)"; exit 1; }
	RUSTFLAGS= cargo run --quiet --release --manifest-path tools/mkdisk/Cargo.toml \
		--target x86_64-unknown-linux-gnu -- $(DISK) \
		/Applications/hello userland/build/hello \
		/Documents/hello.txt userland/hello/hello.txt
