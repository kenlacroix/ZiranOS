# Ziran OS build orchestration.
#
# The pipeline is deliberately explicit -- no magic build.rs, no bootimage tool.
# You can read every step of how three assembly objects and one Rust staticlib
# become a bootable, GRUB-loadable, Multiboot2 kernel image:
#
#   nasm  boot/*.asm            -> build/*.o
#   cargo build (x86_64-none)   -> target/.../libziran_kernel.a
#   ld    (linker.ld)           -> build/kernel.bin      (the multiboot2 kernel)
#   grub-mkrescue               -> build/ziran.iso       (bootable CD image)
#   qemu-system-x86_64          -> boots it
#
# Common targets:
#   make            build build/kernel.bin
#   make iso        build a bootable ISO (needs grub-mkrescue + xorriso)
#   make run        boot the ISO in a QEMU window
#   make run-headless   boot with serial->stdout, no window (what CI uses)
#   make debug      boot QEMU frozen, waiting for GDB on :1234
#   make gdb        attach GDB to a `make debug` session
#   make clean

ARCH        := x86_64
PROFILE     := release
TARGET      := x86_64-unknown-none

# Toolchain hooks. Defaults suit Linux/CI; a command-line override always wins
# over these `:=` assignments. On macOS the default `ld` is Apple's linker
# (cannot link this image: "ld: unknown option: -n"), there is no `readelf` or
# `timeout`, and GRUB ships under an `x86_64-elf-` prefix. Build on a Mac with:
#   make LD=x86_64-elf-ld READELF=x86_64-elf-readelf \
#        GRUB_MKRESCUE=x86_64-elf-grub-mkrescue TIMEOUT=gtimeout
LD            := ld
READELF       := readelf
GRUB_MKRESCUE := grub-mkrescue
TIMEOUT       := timeout
OBJCOPY       := objcopy   # macOS: OBJCOPY=x86_64-elf-objcopy (from x86_64-elf-binutils)

KERNEL      := build/kernel.bin
ISO         := build/ziran.iso
RUST_LIB    := target/$(TARGET)/$(PROFILE)/libziran_kernel.a

ASM_SRC     := $(wildcard boot/*.asm)
ASM_OBJ     := $(patsubst boot/%.asm,build/%.o,$(ASM_SRC))

# QEMU: 128 MiB RAM, no window, serial routed so we can capture it.
# -no-reboot makes a triple fault *exit* QEMU instead of silently rebooting,
# which is exactly what we want a boot test to catch.
QEMU        := qemu-system-x86_64
QEMU_FLAGS  := -m 128M -display none -no-reboot

# UEFI firmware, for booting the rescue ISO on hosts without legacy-BIOS GRUB.
# On Linux/CI grub-mkrescue embeds an i386-pc (legacy BIOS) El Torito image and
# SeaBIOS -- QEMU's default firmware -- boots the ISO directly, so no firmware
# flags are needed and QEMU_FIRMWARE stays empty. On macOS the only Homebrew
# GRUB (x86_64-elf-grub) ships just the x86_64-efi platform, so the ISO is
# UEFI-only; QEMU must be pointed at edk2/OVMF firmware via pflash. Enable that
# path with QEMU_FIRMWARE=uefi (paths assume Homebrew's qemu edk2 build):
#   make run-headless LD=x86_64-elf-ld READELF=x86_64-elf-readelf \
#        GRUB_MKRESCUE=x86_64-elf-grub-mkrescue TIMEOUT=gtimeout QEMU_FIRMWARE=uefi
QEMU_FIRMWARE      :=
UEFI_CODE          := /opt/homebrew/share/qemu/edk2-x86_64-code.fd
UEFI_VARS_TEMPLATE := /opt/homebrew/share/qemu/edk2-i386-vars.fd
UEFI_VARS          := build/uefi-vars.fd

# When QEMU_FIRMWARE=uefi, splice in the pflash pair (read-only code + a
# writable per-build copy of the vars template) and make the run targets depend
# on that writable copy. Both stay empty otherwise, so Linux/CI is untouched.
QEMU_FW_FLAGS :=
QEMU_FW_DEPS  :=
ifeq ($(QEMU_FIRMWARE),uefi)
    QEMU_FW_FLAGS := -drive if=pflash,format=raw,unit=0,readonly=on,file=$(UEFI_CODE) \
                     -drive if=pflash,format=raw,unit=1,file=$(UEFI_VARS)
    QEMU_FW_DEPS  := $(UEFI_VARS)
endif

# Serial markers the headless boot test asserts on. BOOT_MARKER proves the kernel
# reached long mode (Milestone 2); MILESTONE_MARKER proves the current milestone's
# subsystem came up. Both must appear in the captured serial output for a pass.
BOOT_MARKER      := Ziran OS booted
MILESTONE_MARKER := M15: privilege boundary

CARGO_FLAGS := --release
ifeq ($(PROFILE),debug)
    CARGO_FLAGS :=
endif

.PHONY: all iso run console run-headless debug gdb clean check-header web serve web-test

all: $(KERNEL)

# --- Assembly: each boot/*.asm -> build/*.o ----------------------------------
build/%.o: boot/%.asm
	@mkdir -p build
	nasm -f elf64 $< -o $@

# --- Rust kernel staticlib ---------------------------------------------------
# .PHONY-ish: let cargo decide what needs rebuilding.
$(RUST_LIB): $(wildcard src/*.rs) Cargo.toml
	cargo build $(CARGO_FLAGS)

# --- Link into the final multiboot2 kernel image -----------------------------
# -n         : nmagic, do not page-align sections (keeps the image compact)
# -z noexecstack : silence the executable-stack note
$(KERNEL): $(ASM_OBJ) $(RUST_LIB) linker.ld
	@mkdir -p build
	$(LD) -n -z noexecstack -T linker.ld -o $@ $(ASM_OBJ) $(RUST_LIB)
	@echo "built $@"

# Sanity-check the Multiboot2 magic without needing grub-file installed.
# The first dword of the image must be 0xe85250d6, stored little-endian.
check-header: $(KERNEL)
	@magic=$$($(READELF) -x .boot $(KERNEL) | awk 'NR==3{print $$2}'); \
	if [ "$$magic" = "d65052e8" ]; then \
		echo "multiboot2 magic OK ($$magic)"; \
	else \
		echo "BAD multiboot2 magic: $$magic (expected d65052e8)"; exit 1; \
	fi

# --- Bootable ISO via GRUB ---------------------------------------------------
$(ISO): $(KERNEL) grub/grub.cfg
	@mkdir -p build/isofiles/boot/grub
	cp $(KERNEL) build/isofiles/boot/kernel.bin
	cp grub/grub.cfg build/isofiles/boot/grub/grub.cfg
	$(GRUB_MKRESCUE) -o $(ISO) build/isofiles 2>/dev/null
	@echo "built $(ISO)"

iso: $(ISO)

# A writable, per-build copy of the UEFI vars store (the template is read-only in
# the Homebrew prefix). Only built when the run targets ask for it via UEFI mode.
$(UEFI_VARS):
	@mkdir -p build
	cp $(UEFI_VARS_TEMPLATE) $@

# --- Running -----------------------------------------------------------------
run: $(ISO) $(QEMU_FW_DEPS)
	$(QEMU) $(QEMU_FW_FLAGS) -cdrom $(ISO) -m 128M -serial stdio -no-reboot

# Interactive serial console: COM1 is wired to *this terminal*, so you drive the
# shell (help / ps / mem / pwd / cd / ls / cat) right here — no VGA window needed.
# This is the way to use it where the legacy VGA text console isn't displayed
# (e.g. macOS UEFI/OVMF). `-serial mon:stdio` multiplexes the QEMU monitor too:
# press Ctrl-A then X to quit, or Ctrl-A then C for the monitor.
console: $(ISO) $(QEMU_FW_DEPS)
	$(QEMU) $(QEMU_FW_FLAGS) -cdrom $(ISO) -m 128M -serial mon:stdio -display none -no-reboot

# Headless boot smoke test. Boot for a few seconds with serial captured to a log,
# then assert the kernel got far enough to print its long-mode marker. We do not
# make the kernel self-exit -- a real OS should keep running -- so we bound the
# run with `timeout` and judge success by what reached the serial line. This is
# the "does it still boot?" check CI runs on every push.
run-headless: $(ISO) $(QEMU_FW_DEPS)
	@mkdir -p build
	@$(TIMEOUT) 20 $(QEMU) $(QEMU_FW_FLAGS) -cdrom $(ISO) $(QEMU_FLAGS) -serial file:build/serial.log || true
	@echo "----- captured serial -----"; cat build/serial.log 2>/dev/null; echo "---------------------------"
	@if ! grep -q "$(BOOT_MARKER)" build/serial.log 2>/dev/null; then \
		echo "[boot test] FAIL -- '$(BOOT_MARKER)' never appeared on serial"; exit 1; \
	fi
	@if ! grep -q "$(MILESTONE_MARKER)" build/serial.log 2>/dev/null; then \
		echo "[boot test] FAIL -- '$(MILESTONE_MARKER)' never appeared on serial"; exit 1; \
	fi
	@echo "[boot test] PASS -- reached long mode and '$(MILESTONE_MARKER)'"

# Freeze at the first instruction and open a GDB stub on tcp::1234.
debug: $(ISO) $(QEMU_FW_DEPS)
	$(QEMU) $(QEMU_FW_FLAGS) -cdrom $(ISO) -m 128M -serial stdio -s -S

gdb:
	gdb $(KERNEL) -ex "target remote :1234"

# Stage the kernel images for the browser tutorial (web/). kernel.bin/kernel-v86.bin
# feed the (reconstruction-mode) v86 path; ziran.iso is what the real qemu-wasm
# live boot loads (see docs/planning/web-live-boot-qemu-wasm.md).
web: $(KERNEL) $(ISO)
	cp $(KERNEL) web/kernel.bin
	$(OBJCOPY) -O binary $(KERNEL) web/kernel-v86.bin
	cp $(ISO) web/ziran.iso
	@echo "staged web/kernel.bin, web/kernel-v86.bin, and web/ziran.iso (for qemu-wasm)"
	@echo "serve it:  make serve   # (COOP/COEP headers on; needed for the live boot)"

# Serve web/ with the cross-origin isolation headers the live qemu-wasm boot needs.
serve: web
	cd web && python3 serve.py

# End-to-end test of the live qemu-wasm boot in headless Chrome (needs Chrome and
# the built assets: ./web/build-qemu-wasm.sh + make web). Boots qemu-wasm.html,
# waits for the ziran:/> prompt, drives `ps`. The check native -kernel can't do.
web-test:
	python3 web/browser-test.py

clean:
	cargo clean
	rm -rf build
	rm -f web/kernel.bin web/kernel-v86.bin web/ziran.iso
