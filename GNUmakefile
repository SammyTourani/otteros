# OtterOS build/test harness. See CLAUDE.md for the command summary and
# briefs/M0-T1.md for the full spec these targets implement.
#
# Run `source scripts/env.sh` first so `cargo`/`rustup` are on PATH.

MAKEFLAGS += -rR
.SUFFIXES:

LIMINE_DIR     := third_party/limine
LIMINE_TOOL    := $(LIMINE_DIR)/limine
TEST_TIMEOUT   := 90
SHOT_TIMEOUT   := 25

.PHONY: all build build-test iso test bios-test panic-test shot run clean deps \
        _iso-normal _iso-test _iso-test-panic

all: build

# --- Fetch Limine (binary release branch) -----------------------------------

deps: $(LIMINE_TOOL)

$(LIMINE_DIR)/limine.c:
	rm -rf $(LIMINE_DIR)
	git clone --depth=1 --branch v11.x-binary https://github.com/limine-bootloader/limine $(LIMINE_DIR) \
		|| git clone --depth=1 --branch v11.x-binary https://codeberg.org/Limine/Limine $(LIMINE_DIR)

$(LIMINE_TOOL): $(LIMINE_DIR)/limine.c
	$(MAKE) -C $(LIMINE_DIR)

# --- Kernel -------------------------------------------------------------------
# These always invoke cargo (they're .PHONY): cargo's own incremental build
# is what makes repeated invocations fast, not Make's file-mtime tracking.

## NOTE: these `cd kernel &&` (rather than `--manifest-path kernel/Cargo.toml`
## from the repo root) are required: cargo resolves .cargo/config.toml (our
## default target + rustflags) relative to the current *working directory*,
## not relative to --manifest-path, so invoking from the root would silently
## build for the host (aarch64-apple-darwin) instead of x86_64-unknown-none.
build:
	mkdir -p build/bin
	cd kernel && cargo build --bin otteros-kernel --artifact-dir ../build/bin -Z unstable-options

build-test:
	./scripts/build-test-kernel.sh

# --- Limine configs: one tiny static file per boot mode -----------------------

build/limine-normal.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS\n\tprotocol: limine\n\tpath: boot():/boot/otteros-kernel\n' > $@

build/limine-test.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test\n\tprotocol: limine\n\tpath: boot():/boot/otteros-kernel\n\tcmdline: test\n' > $@

build/limine-test-panic.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test panic\n\tprotocol: limine\n\tpath: boot():/boot/otteros-kernel\n\tcmdline: test panic\n' > $@

# --- ISOs: one per mode (brief M0-T1's own suggested "simplest robust option") -
# Reassembled on every invocation (xorriso is fast) so they can never go
# stale relative to the kernel ELF/config just built above.

_iso-normal: build build/limine-normal.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel build/limine-normal.conf build/otteros.iso

_iso-test: build-test build/limine-test.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test.conf build/otteros-test.iso

_iso-test-panic: build-test build/limine-test-panic.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test-panic.conf build/otteros-test-panic.iso

iso: _iso-normal

# --- QEMU-driven targets --------------------------------------------------------

test: _iso-test
	mkdir -p artifacts
	python3 scripts/qemu.py --mode test --firmware uefi --iso build/otteros-test.iso --timeout $(TEST_TIMEOUT)

bios-test: _iso-test
	mkdir -p artifacts
	python3 scripts/qemu.py --mode test --firmware bios --iso build/otteros-test.iso --timeout $(TEST_TIMEOUT)

panic-test: _iso-test-panic
	mkdir -p artifacts
	python3 scripts/qemu.py --mode test --firmware uefi --iso build/otteros-test-panic.iso \
		--timeout $(TEST_TIMEOUT) --expect-failure

shot: _iso-normal
	mkdir -p artifacts
	python3 scripts/qemu.py --mode shot --firmware uefi --iso build/otteros.iso --timeout $(SHOT_TIMEOUT)

run: _iso-normal
	python3 scripts/qemu.py --mode run --firmware uefi --iso build/otteros.iso

# --- Housekeeping ----------------------------------------------------------------

clean:
	cd kernel && cargo clean
	rm -rf build
	rm -f artifacts/*.log artifacts/*.png
