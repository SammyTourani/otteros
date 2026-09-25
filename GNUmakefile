# OtterOS build/test harness. See CLAUDE.md for the command summary and
# briefs/M0-T1.md for the full spec these targets implement.
#
# Run `source scripts/env.sh` first so `cargo`/`rustup` are on PATH.

MAKEFLAGS += -rR
.SUFFIXES:

LIMINE_DIR     := third_party/limine
LIMINE_TOOL    := $(LIMINE_DIR)/limine
TEST_TIMEOUT   := 180
# brief M2-T2b: `test`/`bios-test` run the *entire* in-kernel suite (136+
# tests, including the M2-T3 1000-thread/200-process ones), not one small
# negative-test payload -- under host CPU contention (another process
# compiling on all cores, say) both the suite itself and the QMP
# keyboard-injection wait it depends on can take much longer than the 90s
# every other, single-purpose `--expect-failure` target still uses. A
# generous, separate budget here doesn't slow down a healthy run (`gmake
# test` normally finishes in ~15s); it only matters when the host is
# already under load, which is exactly when a tight timeout would
# otherwise turn "slower" into "flaky."
SUITE_TIMEOUT  := 300
SHOT_TIMEOUT   := 120

.PHONY: all build build-user build-test iso test bios-test test-max panic-test fault-test df-test stackoverflow-test thread-stackoverflow-test shell-test shot run lint clean deps check \
        pmm-double-free-test pmm-free-reserved-test pmm-fault-tests \
        heap-double-free-test heap-bad-class-test heap-fault-tests \
        _iso-normal _iso-test _iso-test-panic _iso-test-pagefault _iso-test-doublefault _iso-test-stackoverflow _iso-test-thread-stackoverflow _iso-test-shelltest \
        _iso-test-pmm-double-free _iso-test-pmm-free-reserved \
        _iso-test-heap-double-free _iso-test-heap-bad-class

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
build: build-user
	mkdir -p build/bin
	cd kernel && cargo build --bin otteros-kernel --artifact-dir ../build/bin -Z unstable-options

build-test:
	./scripts/build-test-kernel.sh

# --- Userspace (brief M2-T3) ---------------------------------------------------
# Same "cd user &&" reasoning as the kernel's own build/build-test targets
# above: user/.cargo/config.toml (custom target spec, build-std, the
# linker-script rustflag) only resolves relative to the invocation
# directory, not --manifest-path.
build-user:
	cd user && cargo build --release

# Reassembled on every invocation (fast: a handful of small files) so it
# can never go stale relative to a fresh `build-user` -- same stance
# `_iso-*`'s own "always reassemble" comment takes for the ISOs themselves.
build/initramfs.tar: build-user
	python3 scripts/make-initramfs.py user/target/x86_64-otter/release build/initramfs.tar

# Build the test disk image (brief M3-T1 §6): 131,079 sectors raw disk where sector n
# starts with "OTTERDISK n" followed by a deterministic pattern. Regenerated
# before each test boot. Depends on mkdisk.py so the image is rebuilt when the script changes.
build/data.img: scripts/mkdisk.py
	mkdir -p build
	python3 scripts/mkdisk.py

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
	printf 'timeout: 0\n/OtterOS\n\tprotocol: limine\n\tkaslr: no\n\tpath: boot():/boot/otteros-kernel\n\tmodule_path: boot():/boot/initramfs.tar\n\tmodule_cmdline: initramfs\n' > $@

build/limine-test.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test\n\tprotocol: limine\n\tkaslr: no\n\tpath: boot():/boot/otteros-kernel\n\tmodule_path: boot():/boot/initramfs.tar\n\tmodule_cmdline: initramfs\n\tcmdline: test\n' > $@

build/limine-test-panic.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test panic\n\tprotocol: limine\n\tkaslr: no\n\tpath: boot():/boot/otteros-kernel\n\tmodule_path: boot():/boot/initramfs.tar\n\tmodule_cmdline: initramfs\n\tcmdline: test panic\n' > $@

build/limine-test-pagefault.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test pagefault\n\tprotocol: limine\n\tkaslr: no\n\tpath: boot():/boot/otteros-kernel\n\tmodule_path: boot():/boot/initramfs.tar\n\tmodule_cmdline: initramfs\n\tcmdline: test pagefault\n' > $@

build/limine-test-doublefault.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test doublefault\n\tprotocol: limine\n\tkaslr: no\n\tpath: boot():/boot/otteros-kernel\n\tmodule_path: boot():/boot/initramfs.tar\n\tmodule_cmdline: initramfs\n\tcmdline: test doublefault\n' > $@

build/limine-test-stackoverflow.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test stackoverflow\n\tprotocol: limine\n\tkaslr: no\n\tpath: boot():/boot/otteros-kernel\n\tmodule_path: boot():/boot/initramfs.tar\n\tmodule_cmdline: initramfs\n\tcmdline: test stackoverflow\n' > $@

build/limine-test-thread-stackoverflow.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test thread-stackoverflow\n\tprotocol: limine\n\tkaslr: no\n\tpath: boot():/boot/otteros-kernel\n\tmodule_path: boot():/boot/initramfs.tar\n\tmodule_cmdline: initramfs\n\tcmdline: test thread-stackoverflow\n' > $@

build/limine-test-pmm-double-free.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test pmm-double-free\n\tprotocol: limine\n\tkaslr: no\n\tpath: boot():/boot/otteros-kernel\n\tmodule_path: boot():/boot/initramfs.tar\n\tmodule_cmdline: initramfs\n\tcmdline: test pmm-double-free\n' > $@

build/limine-test-pmm-free-reserved.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test pmm-free-reserved\n\tprotocol: limine\n\tkaslr: no\n\tpath: boot():/boot/otteros-kernel\n\tmodule_path: boot():/boot/initramfs.tar\n\tmodule_cmdline: initramfs\n\tcmdline: test pmm-free-reserved\n' > $@

build/limine-test-heap-double-free.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test heap-double-free\n\tprotocol: limine\n\tkaslr: no\n\tpath: boot():/boot/otteros-kernel\n\tmodule_path: boot():/boot/initramfs.tar\n\tmodule_cmdline: initramfs\n\tcmdline: test heap-double-free\n' > $@

build/limine-test-heap-bad-class.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test heap-bad-class\n\tprotocol: limine\n\tkaslr: no\n\tpath: boot():/boot/otteros-kernel\n\tmodule_path: boot():/boot/initramfs.tar\n\tmodule_cmdline: initramfs\n\tcmdline: test heap-bad-class\n' > $@

build/limine-test-shelltest.conf:
	mkdir -p build
	printf 'timeout: 0\n/OtterOS test shelltest\n\tprotocol: limine\n\tkaslr: no\n\tpath: boot():/boot/otteros-kernel\n\tmodule_path: boot():/boot/initramfs.tar\n\tmodule_cmdline: initramfs\n\tcmdline: shelltest\n' > $@

# --- ISOs: one per mode (brief M0-T1's own suggested "simplest robust option") -
# Reassembled on every invocation (xorriso is fast) so they can never go
# stale relative to the kernel ELF/config just built above.

_iso-normal: build build/initramfs.tar build/limine-normal.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel build/limine-normal.conf build/otteros.iso

_iso-test: build-test build/initramfs.tar build/data.img build/limine-test.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test.conf build/otteros-test.iso

_iso-test-panic: build-test build/initramfs.tar build/limine-test-panic.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test-panic.conf build/otteros-test-panic.iso

_iso-test-pagefault: build-test build/initramfs.tar build/limine-test-pagefault.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test-pagefault.conf build/otteros-test-pagefault.iso

_iso-test-doublefault: build-test build/initramfs.tar build/limine-test-doublefault.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test-doublefault.conf build/otteros-test-doublefault.iso

_iso-test-stackoverflow: build-test build/initramfs.tar build/limine-test-stackoverflow.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test-stackoverflow.conf build/otteros-test-stackoverflow.iso

_iso-test-thread-stackoverflow: build-test build/initramfs.tar build/limine-test-thread-stackoverflow.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test-thread-stackoverflow.conf build/otteros-test-thread-stackoverflow.iso

_iso-test-pmm-double-free: build-test build/initramfs.tar build/limine-test-pmm-double-free.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test-pmm-double-free.conf build/otteros-test-pmm-double-free.iso

_iso-test-pmm-free-reserved: build-test build/initramfs.tar build/limine-test-pmm-free-reserved.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test-pmm-free-reserved.conf build/otteros-test-pmm-free-reserved.iso

_iso-test-heap-double-free: build-test build/initramfs.tar build/limine-test-heap-double-free.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test-heap-double-free.conf build/otteros-test-heap-double-free.iso

_iso-test-heap-bad-class: build-test build/initramfs.tar build/limine-test-heap-bad-class.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test-heap-bad-class.conf build/otteros-test-heap-bad-class.iso

_iso-test-shelltest: build-test build/initramfs.tar build/limine-test-shelltest.conf $(LIMINE_TOOL)
	./scripts/make-iso.sh build/bin/otteros-kernel-test build/limine-test-shelltest.conf build/otteros-test-shelltest.iso

iso: _iso-normal

# --- QEMU-driven targets --------------------------------------------------------

# `--send-keys hello,caps_lock,h,caps_lock` (brief M1-T6, kernel-review
# fix 6) drives `keyboard_e2e` (the first 5 characters, "hello") and then
# `keyboard_e2e_caps_lock_led` (Caps Lock on, `h` decoding as `H`, Caps
# Lock back off) end to end over QMP. `bios-test` boots the exact same
# test ISO/cmdline (just a different firmware) and so runs both tests
# too -- it gets the same `--send-keys` sequence so they pass rather than
# timing out with nothing ever injected.
test: _iso-test
	mkdir -p artifacts
	python3 scripts/qemu.py --mode test --firmware uefi --iso build/otteros-test.iso --timeout $(SUITE_TIMEOUT) --send-keys hello,caps_lock,h,caps_lock

test-max: _iso-test
	mkdir -p artifacts
	python3 scripts/qemu.py --mode test --firmware uefi --iso build/otteros-test.iso --timeout $(SUITE_TIMEOUT) --send-keys hello,caps_lock,h,caps_lock --cpu max

bios-test: _iso-test
	mkdir -p artifacts
	python3 scripts/qemu.py --mode test --firmware bios --iso build/otteros-test.iso --timeout $(SUITE_TIMEOUT) --send-keys hello,caps_lock,h,caps_lock

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

# `cmdline: test stackoverflow` (brief M1-T4 step 5) recurses, touching a
# 1 KiB local each level, until it runs into the guard page below the
# kernel's own guard-paged stack: the resulting #PF-while-pushing-#PF's-
# own-frame is a double fault, which `trap::trap_dispatch`'s vector-8 arm
# recognises (via `mm::kstack::find_guard(CR2)`) and reports distinctly.
stackoverflow-test: _iso-test-stackoverflow
	mkdir -p artifacts
	python3 scripts/qemu.py --mode test --firmware uefi --iso build/otteros-test-stackoverflow.iso \
		--timeout $(TEST_TIMEOUT) --expect-failure --expect-serial 'kernel stack overflow'

# `cmdline: test thread-stackoverflow` (brief M2-T1 step 10): the same
# unbounded recursion as `stackoverflow-test` above, but run inside a
# *spawned kernel thread*'s own stack instead of the boot stack -- proves
# guard pages work for a per-thread stack too (`sched::spawn`, via
# `mm::kstack::allocate`), not just the one boot-time stack every M1 test
# exercised.
thread-stackoverflow-test: _iso-test-thread-stackoverflow
	mkdir -p artifacts
	python3 scripts/qemu.py --mode test --firmware uefi --iso build/otteros-test-thread-stackoverflow.iso \
		--timeout $(TEST_TIMEOUT) --expect-failure --expect-serial 'kernel stack overflow'

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

# `cmdline: test heap-double-free` deliberately frees the same heap pointer
# twice (kernel-review, M1-T3 fix #1): the slab allocator's per-slot
# `allocated` bitmap must catch it before the free list is corrupted.
heap-double-free-test: _iso-test-heap-double-free
	mkdir -p artifacts
	python3 scripts/qemu.py --mode test --firmware uefi --iso build/otteros-test-heap-double-free.iso \
		--timeout $(TEST_TIMEOUT) --expect-failure --expect-serial 'heap: double free'

# `cmdline: test heap-bad-class` deliberately frees a heap pointer with a
# layout from a different size class than the one it was allocated with
# (kernel-review, M1-T3 fix #1): must be caught, not silently run the
# wrong class's free-list logic on it.
heap-bad-class-test: _iso-test-heap-bad-class
	mkdir -p artifacts
	python3 scripts/qemu.py --mode test --firmware uefi --iso build/otteros-test-heap-bad-class.iso \
		--timeout $(TEST_TIMEOUT) --expect-failure --expect-serial 'heap: class mismatch'

heap-fault-tests: heap-double-free-test heap-bad-class-test

shell-test: _iso-test-shelltest
	mkdir -p artifacts
	python3 scripts/qemu.py --mode test --firmware uefi --iso build/otteros-test-shelltest.iso \
		--timeout $(TEST_TIMEOUT) --send-keys echo,space,hello,space,otter,enter,ps,enter,hello,space,x,space,y,enter,crash,space,null,enter,nosuchcmd,enter,up,enter,exit,space,0,enter \
		--expect-serial '(?m)^\s*1\s+0\s+\S+\s+init\s*$$' --expect-serial '(?m)^\s*\d+\s+1\s+\S+\s+sh\s*$$' --expect-serial 'hello otter' --expect-serial 'argv=\[x, y\]' --expect-serial 'killed' --expect-serial 'command not found[\s\S]*command not found'

shot: _iso-normal
	mkdir -p artifacts
	python3 scripts/qemu.py --mode shot --firmware uefi --iso build/otteros.iso --timeout $(SHOT_TIMEOUT)

run: _iso-normal
	python3 scripts/qemu.py --mode run --firmware uefi --iso build/otteros.iso

# --- Milestone gate --------------------------------------------------------------
# brief M1-T7: the one target that must be green to call a milestone done.
# Runs every acceptance target below in sequence (never stopping early --
# see scripts/check.sh) and prints a PASS/FAIL table; exits non-zero if any
# of them failed.
check:
	./scripts/check.sh

# --- Housekeeping ----------------------------------------------------------------

clean:
	cd kernel && cargo clean
	cd user && cargo clean
	rm -rf build
	rm -f artifacts/*.log artifacts/*.png
