KERNEL  := target/x86_64-unknown-none/debug/anarchy-kernel
ISO_DIR := build/iso_root
ISO     := build/anarchyos.iso
LIMINE  := third_party/limine

.PHONY: all kernel iso run clean

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

run: iso
	qemu-system-x86_64 -M q35 -m 512M -cdrom $(ISO) \
		-serial stdio -no-reboot -no-shutdown

clean:
	cargo clean
	rm -rf build

