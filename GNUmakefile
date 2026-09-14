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

.PHONY: all build build-test iso test bios-test panic-test fault-test df-test shot run lint clean deps \
        pmm-double-free-test pmm-free-reserved-test pmm-fault-tests \
        _iso-normal _iso-test _iso-test-panic _iso-test-pagefault _iso-test-doublefault \
        _iso-test-pmm-double-free _iso-test-pmm-free-reserved

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

# `--tests` so the `#[cfg(test)]` code (the `#[test_case]`s themselves,
# among other things) gets linted too, not just the two normal bin targets;
# `-D warnings` so a new clippy warning fails the build instead of quietly
# accumulating. Unlike `build`/`build-test` above, `--manifest-path` (not
# `cd kernel &&`) is fine here: verified (`cargo clippy --verbose`) to
# still resolve `.cargo/config.toml` and pass `--target x86_64-unknown-none`
# correctly from the repo root.
lint:
	cargo clippy --tests --manifest-path kernel/Cargo.toml -- -D warnings

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

build/limine-test-pagefault.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test pagefault\n\tprotocol: limine\n\tpath: boot():/boot/otteros-kernel\n\tcmdline: test pagefault\n' > $@

build/limine-test-doublefault.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test doublefault\n\tprotocol: limine\n\tpath: boot():/boot/otteros-kernel\n\tcmdline: test doublefault\n' > $@

build/limine-test-pmm-double-free.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test pmm-double-free\n\tprotocol: limine\n\tpath: boot():/boot/otteros-kernel\n\tcmdline: test pmm-double-free\n' > $@

build/limine-test-pmm-free-reserved.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test pmm-free-reserved\n\tprotocol: limine\n\tpath: boot():/boot/otteros-kernel\n\tcmdline: test pmm-free-reserved\n' > $@

# --- ISOs: one per mode (brief M0-T1's own suggested "simplest robust option") -
# Reassembled on every invocation (xorriso is fast) so they can never go
# stale relative to the kernel ELF/config just built above.

_iso-normal: build build/limine-normal.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel build/limine-normal.conf build/otteros.iso

_iso-test: build-test build/limine-test.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test.conf build/otteros-test.iso

_iso-test-panic: build-test build/limine-test-panic.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test-panic.conf build/otteros-test-panic.iso

_iso-test-pagefault: build-test build/limine-test-pagefault.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test-pagefault.conf build/otteros-test-pagefault.iso

_iso-test-doublefault: build-test build/limine-test-doublefault.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test-doublefault.conf build/otteros-test-doublefault.iso

_iso-test-pmm-double-free: build-test build/limine-test-pmm-double-free.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test-pmm-double-free.conf build/otteros-test-pmm-double-free.iso

_iso-test-pmm-free-reserved: build-test build/limine-test-pmm-free-reserved.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test-pmm-free-reserved.conf build/otteros-test-pmm-free-reserved.iso

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

# `cmdline: test pagefault` deliberately reads unmapped memory (brief
# M1-T1 step 7): expect a PAGE FAULT at that exact address, then the panic
# that follows it, and QEMU exiting with the isa-debug-exit failure code.
fault-test: _iso-test-pagefault
	mkdir -p artifacts
	python3 scripts/qemu.py --mode test --firmware uefi --iso build/otteros-test-pagefault.iso \
		--timeout $(TEST_TIMEOUT) --expect-failure --expect-serial 'PAGE FAULT at 0xdeadbeef000'

# `cmdline: test doublefault` deliberately corrupts RSP then `int3`s (brief
# M1-T1 step 7): #PF (can't push the #BP frame) -> #DF on IST1.
df-test: _iso-test-doublefault
	mkdir -p artifacts
	python3 scripts/qemu.py --mode test --firmware uefi --iso build/otteros-test-doublefault.iso \
		--timeout $(TEST_TIMEOUT) --expect-failure --expect-serial 'DOUBLE FAULT'

# `cmdline: test pmm-double-free` allocates a frame, frees it, then frees
# the same address again (kernel-review, M1-T2 fix #3): `free_frame`'s
# double-free check must panic before a second free can corrupt the bitmap.
pmm-double-free-test: _iso-test-pmm-double-free
	mkdir -p artifacts
	python3 scripts/qemu.py --mode test --firmware uefi --iso build/otteros-test-pmm-double-free.iso \
		--timeout $(TEST_TIMEOUT) --expect-failure --expect-serial 'double free'

# `cmdline: test pmm-free-reserved` frees an address one frame past the
# highest USABLE address Limine reported -- never inside any USABLE region
# -- so the ownership check (kernel-review, M1-T2 fix #1/#3) must panic
# instead of silently marking reserved/unlisted memory free.
pmm-free-reserved-test: _iso-test-pmm-free-reserved
	mkdir -p artifacts
	python3 scripts/qemu.py --mode test --firmware uefi --iso build/otteros-test-pmm-free-reserved.iso \
		--timeout $(TEST_TIMEOUT) --expect-failure --expect-serial 'not a usable frame'

pmm-fault-tests: pmm-double-free-test pmm-free-reserved-test

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
