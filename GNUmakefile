# Nuke built-in rules and variables.
MAKEFLAGS += -rR
PROJ_DIR := $(shell pwd)
.SUFFIXES:

# Convenience macro to reliably declare user overridable variables.
override USER_VARIABLE = $(if $(filter $(origin $(1)),default undefined),$(eval override $(1) := $(2)))

# Target architecture to build for. Default to x86_64.
$(call USER_VARIABLE,KARCH,x86_64)

# Default user QEMU flags. These are appended to the QEMU command calls.
$(call USER_VARIABLE,QEMUFLAGS,-m 2G)

# On musl-based hosts (e.g. Alpine), rustup may select the musl toolchain for the pinned nightly,
# but that can fail to start if the system's libgcc_s/libc compatibility is mismatched.
# If a glibc loader is available, prefer the GNU toolchain unless the user overrides it.
ifneq ($(wildcard /lib/ld-musl-x86_64.so.1),)
ifneq ($(wildcard /lib64/ld-linux-x86-64.so.2),)
$(call USER_VARIABLE,RUSTUP_TOOLCHAIN,nightly-2025-06-01-x86_64-unknown-linux-gnu)
endif
endif
export RUSTUP_TOOLCHAIN

override IMAGE_NAME := twilight-os
override INITRAMFS_IMAGE := initramfs.cpio
override SYSTEM_IMAGE := system.tfs
override INITRAMFS_STAGE := build/initramfs
override ISO_REPRO_FLAGS := -iso-level 3 -uid 0 -gid 0 --modification-date=1970010100000000

.PHONY: all
all: $(IMAGE_NAME).iso

.PHONY: all-hdd
all-hdd: $(IMAGE_NAME).hdd

.PHONY: run
run: run-$(KARCH)

.PHONY: run-hdd
run-hdd: run-hdd-$(KARCH)

.PHONY: run-x86_64-uefi
run-x86_64-uefi: ovmf/ovmf-code-$(KARCH).fd ovmf/ovmf-vars-$(KARCH).fd $(IMAGE_NAME).iso
	qemu-system-$(KARCH) \
		-drive if=pflash,unit=0,format=raw,file=ovmf/ovmf-code-$(KARCH).fd,readonly=on \
		-drive if=pflash,unit=1,format=raw,file=ovmf/ovmf-vars-$(KARCH).fd \
		-m 400 \
		-device rtl8139 \
		-netdev user,id=e0,hostfwd=tcp::8000-:80 \
		-smp 4 \
		-usb \
		-device usb-mouse \
		-device piix3-usb-uhci \
		-drive file=hdd.img,format=raw,if=ide \
		-cdrom $(IMAGE_NAME).iso \
		-serial stdio

.PHONY: run-x86_64
run-x86_64: $(IMAGE_NAME).iso
	@if [ ! -f hdd.img ]; then \
		echo "Creating hdd.img..."; \
		qemu-img create -f raw hdd.img 1G; \
	fi
	qemu-system-$(KARCH) \
		-m 1024 \
		-netdev user,id=net0,hostfwd=tcp::8000-:80 -device rtl8139,netdev=net0 \
		-smp 4 \
		-drive file=hdd.img,format=raw,if=ide \
		-cdrom $(IMAGE_NAME).iso \
		-serial stdio \
		-d int,guest_errors,unimp \
	  	-D qemu.log \
		-vga std

.PHONY: run-hdd-x86_64
run-hdd-x86_64: ovmf/ovmf-code-$(KARCH).fd ovmf/ovmf-vars-$(KARCH).fd $(IMAGE_NAME).hdd
	qemu-system-$(KARCH) \-m 1024 \
		-netdev user,id=net0,hostfwd=tcp::8000-:80 -device rtl8139,netdev=net0 \
		-smp 4 \
		-usb \
		-device usb-mouse \
		-device piix3-usb-uhci \
		-serial stdio \
		-d int,guest_errors,unimp \
	  	-D qemu.log \
		-vga std \
		-drive if=pflash,unit=0,format=raw,file=ovmf/ovmf-code-$(KARCH).fd,readonly=on \
		-drive if=pflash,unit=1,format=raw,file=ovmf/ovmf-vars-$(KARCH).fd \
		-hda $(IMAGE_NAME).hdd

.PHONY: run-blk-bios
run-blk-bios: $(IMAGE_NAME).iso
	@if [ ! -f hdd.img ]; then \
		echo "Creating hdd.img..."; \
		qemu-img create -f raw hdd.img 1G; \
	fi
	qemu-system-$(KARCH) \
		-m 1024 \
		-smp 4 \
		-boot d \
  		-netdev user,id=net0,hostfwd=tcp::8000-:80 \
  		-device rtl8139,netdev=net0 \
		-device qemu-xhci \
		-cdrom $(IMAGE_NAME).iso \
  		-drive file=hdd.img,if=none,format=raw,id=vd0 \
  		-device virtio-blk-pci,drive=vd0 \
		-serial stdio \
		-d int,guest_errors,unimp \
	  	-D qemu.log \
		-vga std

.PHONY: run-aarch64
run-aarch64: ovmf/ovmf-code-$(KARCH).fd ovmf/ovmf-vars-$(KARCH).fd $(IMAGE_NAME).iso
	qemu-system-$(KARCH) \
		-M virt \
		-cpu cortex-a72 \
		-device ramfb \
		-device usb-kbd \
		-device usb-mouse \
		-drive if=pflash,unit=0,format=raw,file=ovmf/ovmf-code-$(KARCH).fd,readonly=on \
		-drive if=pflash,unit=1,format=raw,file=ovmf/ovmf-vars-$(KARCH).fd \
		-cdrom $(IMAGE_NAME).iso \
		$(QEMUFLAGS)

.PHONY: run-hdd-aarch64
run-hdd-aarch64: ovmf/ovmf-code-$(KARCH).fd ovmf/ovmf-vars-$(KARCH).fd $(IMAGE_NAME).hdd
	qemu-system-$(KARCH) \
		-M virt \
		-cpu cortex-a72 \
		-device ramfb \
		-device qemu-xhci \
		-device usb-kbd \
		-device usb-mouse \
		-drive if=pflash,unit=0,format=raw,file=ovmf/ovmf-code-$(KARCH).fd,readonly=on \
		-drive if=pflash,unit=1,format=raw,file=ovmf/ovmf-vars-$(KARCH).fd \
		-hda $(IMAGE_NAME).hdd \
		$(QEMUFLAGS)

.PHONY: run-riscv64
run-riscv64: ovmf/ovmf-code-$(KARCH).fd ovmf/ovmf-vars-$(KARCH).fd $(IMAGE_NAME).iso
	qemu-system-$(KARCH) \
		-M virt \
		-cpu rv64 \
		-device ramfb \
		-device qemu-xhci \
		-device usb-kbd \
		-device usb-mouse \
		-drive if=pflash,unit=0,format=raw,file=ovmf/ovmf-code-$(KARCH).fd,readonly=on \
		-drive if=pflash,unit=1,format=raw,file=ovmf/ovmf-vars-$(KARCH).fd \
		-cdrom $(IMAGE_NAME).iso \
		$(QEMUFLAGS)

.PHONY: run-hdd-riscv64
run-hdd-riscv64: ovmf/ovmf-code-$(KARCH).fd ovmf/ovmf-vars-$(KARCH).fd $(IMAGE_NAME).hdd
	qemu-system-$(KARCH) \
		-M virt \
		-cpu rv64 \
		-device ramfb \
		-device qemu-xhci \
		-device usb-kbd \
		-device usb-mouse \
		-drive if=pflash,unit=0,format=raw,file=ovmf/ovmf-code-$(KARCH).fd,readonly=on \
		-drive if=pflash,unit=1,format=raw,file=ovmf/ovmf-vars-$(KARCH).fd \
		-hda $(IMAGE_NAME).hdd \
		$(QEMUFLAGS)

.PHONY: run-loongarch64
run-loongarch64: ovmf/ovmf-code-$(KARCH).fd ovmf/ovmf-vars-$(KARCH).fd $(IMAGE_NAME).iso
	qemu-system-$(KARCH) \
		-M virt \
		-cpu la464 \
		-device ramfb \
		-device qemu-xhci \
		-device usb-kbd \
		-device usb-mouse \
		-drive if=pflash,unit=0,format=raw,file=ovmf/ovmf-code-$(KARCH).fd,readonly=on \
		-drive if=pflash,unit=1,format=raw,file=ovmf/ovmf-vars-$(KARCH).fd \
		-cdrom $(IMAGE_NAME).iso \
		$(QEMUFLAGS)

.PHONY: run-hdd-loongarch64
run-hdd-loongarch64: ovmf/ovmf-code-$(KARCH).fd ovmf/ovmf-vars-$(KARCH).fd $(IMAGE_NAME).hdd
	qemu-system-$(KARCH) \
		-M virt \
		-cpu la464 \
		-device ramfb \
		-device qemu-xhci \
		-device usb-kbd \
		-device usb-mouse \
		-drive if=pflash,unit=0,format=raw,file=ovmf/ovmf-code-$(KARCH).fd,readonly=on \
		-drive if=pflash,unit=1,format=raw,file=ovmf/ovmf-vars-$(KARCH).fd \
		-hda $(IMAGE_NAME).hdd \
		$(QEMUFLAGS)


.PHONY: run-bios
run-bios: $(IMAGE_NAME).iso
	qemu-system-$(KARCH) \
		-M q35 \
		-cdrom $(IMAGE_NAME).iso \
		-boot d \
		$(QEMUFLAGS)

.PHONY: run-hdd-bios
run-hdd-bios: $(IMAGE_NAME).hdd
	qemu-system-$(KARCH) \
		-m 1024 \
		-device piix3-usb-uhci,id=uhci \
		-device usb-kbd,bus=uhci.0 \
		-device usb-mouse,bus=uhci.0 \
		-netdev user,id=net0,hostfwd=tcp::8000-:80 -device rtl8139,netdev=net0 \
		-smp 4 \
		-hda $(IMAGE_NAME).hdd \
		-serial stdio \
		-d int,guest_errors,unimp \
	  	-D qemu.log \
		-vga std

ovmf/ovmf-code-$(KARCH).fd:
	mkdir -p ovmf
	curl -Lo $@ https://github.com/osdev0/edk2-ovmf-nightly/releases/latest/download/ovmf-code-$(KARCH).fd
	case "$(KARCH)" in \
		aarch64) dd if=/dev/zero of=$@ bs=1 count=0 seek=67108864 2>/dev/null;; \
		loongarch64) dd if=/dev/zero of=$@ bs=1 count=0 seek=5242880 2>/dev/null;; \
		riscv64) dd if=/dev/zero of=$@ bs=1 count=0 seek=33554432 2>/dev/null;; \
	esac

ovmf/ovmf-vars-$(KARCH).fd:
	mkdir -p ovmf
	curl -Lo $@ https://github.com/osdev0/edk2-ovmf-nightly/releases/latest/download/ovmf-vars-$(KARCH).fd
	case "$(KARCH)" in \
		aarch64) dd if=/dev/zero of=$@ bs=1 count=0 seek=67108864 2>/dev/null;; \
		loongarch64) dd if=/dev/zero of=$@ bs=1 count=0 seek=5242880 2>/dev/null;; \
		riscv64) dd if=/dev/zero of=$@ bs=1 count=0 seek=33554432 2>/dev/null;; \
	esac

limine/limine:
	rm -rf limine
	git clone https://github.com/limine-bootloader/limine.git --branch=v9.x-binary --depth=1
	$(MAKE) -C limine

.PHONY: kernel
kernel:
	$(MAKE) -C kernel/twilight_kernel

.PHONY: userspace
userspace:
	cd userspace && RUSTUP_TOOLCHAIN=stable cargo build --release

$(INITRAMFS_IMAGE): userspace
	rm -rf $(INITRAMFS_STAGE)
	mkdir -p $(INITRAMFS_STAGE)/bin $(INITRAMFS_STAGE)/dev $(INITRAMFS_STAGE)/proc $(INITRAMFS_STAGE)/home $(INITRAMFS_STAGE)/run $(INITRAMFS_STAGE)/tmp $(INITRAMFS_STAGE)/etc
	install -m 0755 rootfs/bin/init $(INITRAMFS_STAGE)/bin/init
	install -m 0755 rootfs/bin/oksh $(INITRAMFS_STAGE)/bin/oksh
	install -m 0755 rootfs/bin/install $(INITRAMFS_STAGE)/bin/install
	find $(INITRAMFS_STAGE) -exec touch -h -d @0 {} +
	cd $(INITRAMFS_STAGE) && find . -print | LC_ALL=C sort | cpio --quiet --reproducible --owner=0:0 -o -H newc > ../../$(INITRAMFS_IMAGE)
	test $$(stat -c '%s' $(INITRAMFS_IMAGE)) -le 4194304

.PHONY: initramfs
initramfs: $(INITRAMFS_IMAGE)

$(SYSTEM_IMAGE): userspace tools/mktfs/Cargo.toml tools/mktfs/src/main.rs
	cargo run --release --manifest-path tools/mktfs/Cargo.toml -- rootfs $(SYSTEM_IMAGE)

.PHONY: system-image
system-image: $(SYSTEM_IMAGE)

$(IMAGE_NAME).iso: limine/limine kernel $(INITRAMFS_IMAGE) $(SYSTEM_IMAGE)
	rm -rf iso_root
	mkdir -p iso_root/boot
	cp -v kernel/twilight_kernel/kernel iso_root/boot/
	cp -v $(INITRAMFS_IMAGE) iso_root/boot/
	cp -v $(SYSTEM_IMAGE) iso_root/SYSTEM.TFS
	mkdir -p iso_root/boot/limine
	cp -v limine.conf iso_root/boot/limine/
	mkdir -p iso_root/EFI/BOOT
ifeq ($(KARCH),x86_64)
	cp -v limine/limine-bios.sys limine/limine-bios-cd.bin limine/limine-uefi-cd.bin iso_root/boot/limine/
	cp -v limine/BOOTX64.EFI iso_root/EFI/BOOT/
	cp -v limine/BOOTIA32.EFI iso_root/EFI/BOOT/
	find iso_root -exec touch -h -d @0 {} +
	xorriso -as mkisofs -b boot/limine/limine-bios-cd.bin \
		-no-emul-boot -boot-load-size 4 -boot-info-table \
		--efi-boot boot/limine/limine-uefi-cd.bin \
		-efi-boot-part --efi-boot-image --protective-msdos-label \
		$(ISO_REPRO_FLAGS) iso_root -o $(IMAGE_NAME).iso
	./limine/limine bios-install $(IMAGE_NAME).iso
endif
ifeq ($(KARCH),aarch64)
	cp -v limine/limine-uefi-cd.bin iso_root/boot/limine/
	cp -v limine/BOOTAA64.EFI iso_root/EFI/BOOT/
	find iso_root -exec touch -h -d @0 {} +
	xorriso -as mkisofs \
		--efi-boot boot/limine/limine-uefi-cd.bin \
		-efi-boot-part --efi-boot-image --protective-msdos-label \
		$(ISO_REPRO_FLAGS) iso_root -o $(IMAGE_NAME).iso
endif
ifeq ($(KARCH),riscv64)
	cp -v limine/limine-uefi-cd.bin iso_root/boot/limine/
	cp -v limine/BOOTRISCV64.EFI iso_root/EFI/BOOT/
	find iso_root -exec touch -h -d @0 {} +
	xorriso -as mkisofs \
		--efi-boot boot/limine/limine-uefi-cd.bin \
		-efi-boot-part --efi-boot-image --protective-msdos-label \
		$(ISO_REPRO_FLAGS) iso_root -o $(IMAGE_NAME).iso
endif
ifeq ($(KARCH),loongarch64)
	cp -v limine/limine-uefi-cd.bin iso_root/boot/limine/
	cp -v limine/BOOTLOONGARCH64.EFI iso_root/EFI/BOOT/
	find iso_root -exec touch -h -d @0 {} +
	xorriso -as mkisofs \
		--efi-boot boot/limine/limine-uefi-cd.bin \
		-efi-boot-part --efi-boot-image --protective-msdos-label \
		$(ISO_REPRO_FLAGS) iso_root -o $(IMAGE_NAME).iso
endif
	rm -rf iso_root

$(IMAGE_NAME).hdd: limine/limine kernel $(INITRAMFS_IMAGE) $(SYSTEM_IMAGE)
	rm -f $(IMAGE_NAME).hdd
	dd if=/dev/zero bs=1M count=0 seek=200 of=$(IMAGE_NAME).hdd
	PATH=$$PATH:/usr/sbin:/sbin sgdisk $(IMAGE_NAME).hdd -n 1:2048 -t 1:ef00 -m 1
	./limine/limine bios-install $(IMAGE_NAME).hdd
ifeq ($(KARCH),x86_64)
	./limine/limine bios-install $(IMAGE_NAME).hdd
endif
	mformat -F -i $(IMAGE_NAME).hdd@@1M
	mmd -i $(IMAGE_NAME).hdd@@1M ::/EFI ::/EFI/BOOT ::/boot ::/boot/limine
	mcopy -i $(IMAGE_NAME).hdd@@1M kernel/twilight_kernel/kernel ::/boot
	mcopy -i $(IMAGE_NAME).hdd@@1M $(INITRAMFS_IMAGE) ::/boot
	mcopy -i $(IMAGE_NAME).hdd@@1M $(SYSTEM_IMAGE) ::/SYSTEM.TFS
	mcopy -i $(IMAGE_NAME).hdd@@1M limine.conf ::/boot/limine
ifeq ($(KARCH),x86_64)
	mcopy -i $(IMAGE_NAME).hdd@@1M limine/limine-bios.sys ::/boot/limine
	mcopy -i $(IMAGE_NAME).hdd@@1M limine/BOOTX64.EFI ::/EFI/BOOT
	mcopy -i $(IMAGE_NAME).hdd@@1M limine/BOOTIA32.EFI ::/EFI/BOOT
endif
ifeq ($(KARCH),aarch64)
	mcopy -i $(IMAGE_NAME).hdd@@1M limine/BOOTAA64.EFI ::/EFI/BOOT
endif
ifeq ($(KARCH),riscv64)
	mcopy -i $(IMAGE_NAME).hdd@@1M limine/BOOTRISCV64.EFI ::/EFI/BOOT
endif
ifeq ($(KARCH),loongarch64)
	mcopy -i $(IMAGE_NAME).hdd@@1M limine/BOOTLOONGARCH64.EFI ::/EFI/BOOT
endif

.PHONY: test-time
test-time: $(IMAGE_NAME).iso
	# Non-destructive: boots the live ISO headless, never touches hdd.img.
	tools/time-regression/run.sh $(IMAGE_NAME).iso

.PHONY: clean
clean:
	$(MAKE) -C kernel/twilight_kernel clean
	rm -rf iso_root build rootfs.cpio $(INITRAMFS_IMAGE) $(SYSTEM_IMAGE) $(IMAGE_NAME).iso $(IMAGE_NAME).hdd

.PHONY: distclean
distclean: clean
	$(MAKE) -C kernel distclean
	rm -rf limine ovmf
