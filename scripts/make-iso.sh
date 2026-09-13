#!/usr/bin/env bash
# Assemble a hybrid BIOS+UEFI Limine ISO from a kernel ELF and a limine.conf.
#
# Usage: make-iso.sh <kernel-elf> <limine.conf> <output.iso>
set -euo pipefail

if [[ $# -ne 3 ]]; then
    echo "usage: $0 <kernel-elf> <limine.conf> <output.iso>" >&2
    exit 2
fi

KERNEL="$1"
CONF="$2"
OUT="$3"
LIMINE_DIR="third_party/limine"
ROOT="build/iso_root.$$"

rm -rf "$ROOT"
mkdir -p "$ROOT/boot/limine" "$ROOT/EFI/BOOT"

cp "$KERNEL" "$ROOT/boot/otteros-kernel"
cp "$CONF" "$ROOT/boot/limine/limine.conf"
cp "$LIMINE_DIR/limine-bios.sys" "$LIMINE_DIR/limine-bios-cd.bin" "$LIMINE_DIR/limine-uefi-cd.bin" "$ROOT/boot/limine/"
cp "$LIMINE_DIR/BOOTX64.EFI" "$LIMINE_DIR/BOOTIA32.EFI" "$ROOT/EFI/BOOT/"

xorriso -as mkisofs -R -J \
    -b boot/limine/limine-bios-cd.bin -no-emul-boot -boot-load-size 4 -boot-info-table \
    --efi-boot boot/limine/limine-uefi-cd.bin -efi-boot-part --efi-boot-image --protective-msdos-label \
    "$ROOT" -o "$OUT" >/dev/null

"$LIMINE_DIR/limine" bios-install "$OUT" >/dev/null

rm -rf "$ROOT"
