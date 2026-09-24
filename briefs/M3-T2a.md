# Brief M3-T2a — `otter-fat`: partitions and FAT32 read/write as a pure crate

## Goal
All of the partition and FAT32 logic from briefs/M3-T2.md, as a no_std + alloc crate over a
`BlockDevice` trait, proven on the Mac against the real tools: images made by `mkfs.fat` and
mtools read back exactly, and everything OtterOS writes passes `fsck.fat -n` and is read back
exactly by mtools. The kernel task (M3-T2, later) then only wires the virtio-blk device, the RTC
and the mount into the kernel.

## Applies
D2 (no `fatfs` or similar crates), D7, D27 (pure crate, zero external dependencies including
dev-dependencies; tests may use std and spawn host tools).

## Design (crates/otter-fat)
1. `BlockDevice` trait: `block_size()`, `block_count()`, `read_blocks(lba, buf)`,
   `write_blocks(lba, buf)` (buf length a multiple of the block size; multi-block requests are how
   contiguous runs are read), `flush()`. Errors are an `IoError` wrapped into `FsError`.
2. Partitions (`part.rs`): MBR (four primary entries, types 0x0B/0x0C, 0xEE protective → GPT), GPT
   (header CRC32 and entry-array CRC32 verified; backup header used when the primary is corrupt;
   Microsoft basic data and EFI system GUIDs recognised), superfloppy (BPB at LBA 0). A
   `Partition<D>` is itself a `BlockDevice` (offset + length, bounds-checked). CRC-32 (IEEE) in the
   crate, tested against the standard check value 0xCBF43926 for "123456789".
3. FAT32 exactly as briefs/M3-T2.md Design items 2–4 (BPB validation, FSInfo as validated hints,
   both FATs updated, top 4 bits preserved, loop-detecting chain walk, allocation from the
   next-free hint, zeroed directory clusters, 8.3 names with NT case flags, LFN with UCS-2 ↔ UTF-8
   and checksum validation, orphan LFN ignored, `~N` alias generation with collision numbering,
   directory growth, `.`/`..`, and the `Fat32Fs` API: `lookup`, `readdir`, `read`, `write`,
   `truncate`, `create`, `mkdir`, `unlink`, `rmdir` (empty only), `rename` (same volume, incl.
   between directories, updating `..` of a moved directory), `stat`, `sync`). Paths are UTF-8 with
   `/`; lookup is case-insensitive per FAT rules while names are stored as given.
4. Time comes from a `Clock` trait (`now() -> FatDateTime`); writes set mtime (and ctime on
   create). Encode/decode FAT date/time exactly (2-second resolution, years 1980–2107).
5. Sequential reads coalesce contiguous clusters into one `read_blocks` call per run; writes of
   whole clusters likewise. A small write-back cache is NOT required here (the kernel has one).
6. `FsError` enum: NotFound, NotADirectory, IsADirectory, AlreadyExists, DirectoryNotEmpty, NoSpace,
   InvalidName, NameTooLong, Corrupt(&'static str), Io. Never panic on a corrupt image.

## Tests (host; `cargo test -p otter-fat`; >= 40 tests)
Host tools: `/opt/homebrew/sbin/mkfs.fat`, `/opt/homebrew/sbin/fsck.fat`, mtools (`mcopy`,
`mdir`, `mmd`, `mtype`, `mformat`), Python 3. Image files go in a temp directory under
`crates/target/` (never the internal disk's home directory, never committed). Tests that need a
host tool FAIL with a clear message when it is missing (never silently skip), unless
`OTTEROS_SKIP_HOST_TOOLS=1` is set.
- Unit: CRC-32; MBR and GPT parse from Python-built images (GPT with valid CRCs, a corrupted
  primary falling back to the backup, a corrupted entry array rejected); BPB rejects nonsense
  (fields zero, non-power-of-two cluster size, FAT count 0); 8.3 alias generation cases
  (`Long File Name.txt` → `LONGFI~1.TXT`, collisions to `~2`…`~9` and then `LONGF~10`), LFN
  checksum, UTF-8 ↔ UCS-2 incl. `résumé.txt` and `日本語.txt`, FAT date/time round trip.
- Read oracle: build images with `mkfs.fat -F 32` (superfloppy and inside an MBR partition) and
  with `mformat -F`, populate with mtools from a fixture tree (HELLO.TXT; `Long File Name With
  Spaces.txt`; `résumé.txt`; `dir/sub/deep.txt`; a 5 MiB deterministic `big.bin`; a directory with
  300 files; an empty file; a file of exactly one cluster), then assert otter-fat's `readdir` names
  and sizes and every file's bytes equal the fixture tree exactly. Assert `big.bin` is read with
  at most 1 `read_blocks` call per contiguous run (count calls through a wrapper device).
- Write oracle: on a fresh `mkfs.fat` image, otter-fat creates files and directories (incl. LFN,
  Unicode, 300 files, nested dirs), writes, appends across cluster boundaries, overwrites in the
  middle, truncates (grow with zeros and shrink), renames (file and directory across parents),
  unlinks and rmdirs. Then: `fsck.fat -n` exits 0 with no "differences"/"errors" output, and
  `mcopy -s` of the whole volume yields a tree byte-identical to the expected tree; `mdir` shows the
  mtime from the test clock.
- Differential: 2,000 seeded random operations (create/write at random offset/truncate/mkdir/
  unlink/rmdir/rename) applied to otter-fat and to an in-memory model; after every 100 ops the full
  otter-fat tree equals the model; at the end `fsck.fat -n` is clean and the mtools-extracted tree
  equals the model. Run with 3 seeds. Free-cluster count (full FAT scan) equals FSInfo's after
  `sync`.
- Robustness: 1,000 byte-flip mutations of the metadata region (BPB, FAT, directory clusters) of a
  populated image; mounting and walking the whole tree returns values or errors, never panics or
  loops forever (chain-loop detection). Out-of-space: filling the volume returns NoSpace and the
  image still passes fsck.

## Acceptance
- `scripts/verify-crate.sh otter-fat` → tests ≥ 40 passed, 0 failed, clippy ok, no_std ok,
  external deps 0.
- The fsck and mtools checks above run inside `cargo test` (not only by hand).

## Report
<=10 lines: the verify-crate line, the differential and fsck results, deviations. Do not commit.
