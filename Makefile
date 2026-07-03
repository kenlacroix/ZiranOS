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

# The line the kernel prints over serial once it reaches long mode. The headless
# boot test passes iff this appears in the captured serial output.
BOOT_MARKER := Ziran OS booted

CARGO_FLAGS := --release
ifeq ($(PROFILE),debug)
    CARGO_FLAGS :=
endif

.PHONY: all iso run run-headless debug gdb clean check-header web

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
	ld -n -z noexecstack -T linker.ld -o $@ $(ASM_OBJ) $(RUST_LIB)
	@echo "built $@"

# Sanity-check the Multiboot2 magic without needing grub-file installed.
# The first dword of the image must be 0xe85250d6, stored little-endian.
check-header: $(KERNEL)
	@magic=$$(readelf -x .boot $(KERNEL) | awk 'NR==3{print $$2}'); \
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
	grub-mkrescue -o $(ISO) build/isofiles 2>/dev/null
	@echo "built $(ISO)"

iso: $(ISO)

# --- Running -----------------------------------------------------------------
run: $(ISO)
	$(QEMU) -cdrom $(ISO) -m 128M -serial stdio -no-reboot

# Headless boot smoke test. Boot for a few seconds with serial captured to a log,
# then assert the kernel got far enough to print its long-mode marker. We do not
# make the kernel self-exit -- a real OS should keep running -- so we bound the
# run with `timeout` and judge success by what reached the serial line. This is
# the "does it still boot?" check CI runs on every push.
run-headless: $(ISO)
	@mkdir -p build
	@timeout 20 $(QEMU) -cdrom $(ISO) $(QEMU_FLAGS) -serial file:build/serial.log || true
	@echo "----- captured serial -----"; cat build/serial.log 2>/dev/null; echo "---------------------------"
	@if grep -q "$(BOOT_MARKER)" build/serial.log 2>/dev/null; then \
		echo "[boot test] PASS -- kernel reached long mode"; \
	else \
		echo "[boot test] FAIL -- '$(BOOT_MARKER)' never appeared on serial"; exit 1; \
	fi

# Freeze at the first instruction and open a GDB stub on tcp::1234.
debug: $(ISO)
	$(QEMU) -cdrom $(ISO) -m 128M -serial stdio -s -S

gdb:
	gdb $(KERNEL) -ex "target remote :1234"

# Stage the kernel image for the browser tutorial (web/). Live mode loads
# web/kernel.bin; see web/README.md for serving and self-hosting the v86 files.
web: $(KERNEL)
	cp $(KERNEL) web/kernel.bin
	@echo "copied $(KERNEL) -> web/kernel.bin"
	@echo "serve it:  cd web && python3 -m http.server 8000  # then open localhost:8000"

clean:
	cargo clean
	rm -rf build
	rm -f web/kernel.bin
