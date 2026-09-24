#!/usr/bin/env python3
"""Packs the OtterOS initramfs (brief M2-T3 step 7): a plain POSIX ustar
archive (not GNU's extended format -- `kernel::fs::initramfs`'s own parser
only understands ustar's own `prefix` mechanism for long names, and every
path here is short enough that it never actually exercises that anyway)
containing the four userspace binaries `user/`'s build produced, plus one
deliberately non-ELF file (`etc/motd`) that `utest` spawns to confirm
`ENOEXEC`.

Usage: make-initramfs.py <user-release-dir> <output.tar>
"""
import os
import sys
import tarfile

MOTD = b"OtterOS: a plain-text file, not an ELF executable -- utest spawns this expecting -ENOEXEC.\n"


def add_file(tar, arcname, local_path=None, data=None, mode=0o755):
    info = tarfile.TarInfo(name=arcname)
    info.mode = mode
    info.uid = 0
    info.gid = 0
    info.mtime = 0
    if data is not None:
        info.size = len(data)
        import io
        tar.addfile(info, io.BytesIO(data))
    else:
        info.size = os.path.getsize(local_path)
        with open(local_path, "rb") as f:
            tar.addfile(info, f)


def main():
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} <user-release-dir> <output.tar>", file=sys.stderr)
        return 2
    release_dir, out_path = sys.argv[1], sys.argv[2]

    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    with tarfile.open(out_path, "w", format=tarfile.USTAR_FORMAT) as tar:
        for prog in ("init", "hello", "utest", "crash"):
            add_file(tar, f"bin/{prog}", local_path=os.path.join(release_dir, prog), mode=0o755)
        add_file(tar, "etc/motd", data=MOTD, mode=0o644)

    print(f"[make-initramfs] wrote {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
