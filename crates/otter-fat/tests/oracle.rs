//! Acceptance oracle for brief M3-T2a, written by the orchestrator. The crate must pass this file
//! unchanged; add your own tests elsewhere. The references are the real tools: images come from
//! mkfs.fat, mformat and mcopy, and everything otter-fat writes must pass `fsck.fat -n` and come
//! back byte-identical through `mcopy -s`. A differential test runs thousands of random operations
//! against an in-memory model of a case-insensitive, case-preserving file system.
//!
//! Path-based API this file relies on (the kernel's VFS will use the same):
//!   Fat32Fs::mount(device, clock) -> Result<Fat32Fs, FsError>
//!   stat(path) -> Result<FileInfo, FsError>            (FileInfo: name, size, is_dir, modified)
//!   read_dir(path) -> Result<Vec<FileInfo>, FsError>   (long names; no "." or "..")
//!   read_file(path, offset: u64, buf) -> Result<usize, FsError>
//!   write_file(path, offset: u64, data) -> Result<(), FsError>  (zero-fills any gap)
//!   create_file(path), mkdir(path), truncate(path, len: u64), unlink(path), rmdir(path),
//!   rename(from, to), sync(), unmount() -> Result<device, FsError>, device() -> &device
//! FsError and FatDateTime implement Debug + PartialEq. Paths are absolute, '/'-separated UTF-8;
//! lookups are case-insensitive (ASCII), names are stored as given.
//!
//! Note: mtools stores non-ASCII names that fit 8.3 (e.g. "résumé.txt") as OEM-codepage short
//! names without long-name entries and misreads them itself, and it sometimes rewrites apostrophes
//! in long names as '_' when writing; so fixtures written BY mtools use non-ASCII names too long for
//! 8.3 and no apostrophes. otter-fat must give every non-ASCII name a long-name entry.

use otter_fat::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------------------------
// Host tools and images.

const MKFS: &str = "/opt/homebrew/sbin/mkfs.fat";
const FSCK: &str = "/opt/homebrew/sbin/fsck.fat";
const MCOPY: &str = "/opt/homebrew/bin/mcopy";
const MFORMAT: &str = "/opt/homebrew/bin/mformat";
const MDIR: &str = "/opt/homebrew/bin/mdir";

fn tools_present() -> bool {
    let missing: Vec<&str> = [MKFS, FSCK, MCOPY, MFORMAT, MDIR].into_iter().filter(|t| !Path::new(t).exists()).collect();
    if missing.is_empty() {
        return true;
    }
    if std::env::var("OTTEROS_SKIP_HOST_TOOLS").as_deref() == Ok("1") {
        eprintln!("fat oracle: skipping, missing {missing:?}");
        return false;
    }
    panic!("host tools missing: {missing:?} (brew install dosfstools mtools), or set OTTEROS_SKIP_HOST_TOOLS=1");
}

fn workdir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/otter-fat-oracle").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(cmd: &str, args: &[&str]) -> (bool, String) {
    let out = Command::new(cmd)
        .args(args)
        .env("MTOOLS_SKIP_CHECK", "1")
        .env("LC_ALL", "en_US.UTF-8")
        .env("LANG", "en_US.UTF-8")
        .output()
        .unwrap_or_else(|e| panic!("{cmd}: {e}"));
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

fn mkfs(img: &Path, kib: u64) {
    let _ = std::fs::remove_file(img);
    let (ok, out) = run(MKFS, &["-F", "32", "-S", "512", "-s", "1", "-n", "OTTER", "-C", img.to_str().unwrap(), &kib.to_string()]);
    assert!(ok, "mkfs.fat: {out}");
}

fn fsck(img: &Path) {
    let (ok, out) = run(FSCK, &["-n", img.to_str().unwrap()]);
    assert!(ok && !out.contains("wrong") && !out.to_lowercase().contains("error"), "fsck.fat -n reported problems:\n{out}");
}

/// A block device over an image in memory, counting read calls and the largest request.
struct Img {
    data: Vec<u8>,
    reads: usize,
    max_read: usize,
}

impl Img {
    fn load(path: &Path) -> Img {
        Img { data: std::fs::read(path).unwrap(), reads: 0, max_read: 0 }
    }
}

impl BlockDevice for Img {
    fn block_size(&self) -> usize {
        512
    }
    fn block_count(&self) -> u64 {
        (self.data.len() / 512) as u64
    }
    fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        assert!(buf.len().is_multiple_of(512) && !buf.is_empty(), "read of {} bytes", buf.len());
        // Like a real disk, a request past the end is an I/O error (never a panic or a wrap).
        let Some(src) = (lba as usize).checked_mul(512).and_then(|s| self.data.get(s..s.checked_add(buf.len())?)) else {
            return Err(IoError::InvalidAddress);
        };
        buf.copy_from_slice(src);
        self.reads += 1;
        self.max_read = self.max_read.max(buf.len());
        Ok(())
    }
    fn write_blocks(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        assert!(buf.len().is_multiple_of(512) && !buf.is_empty(), "write of {} bytes", buf.len());
        let Some(dst) = (lba as usize).checked_mul(512).and_then(|s| self.data.get_mut(s..s.checked_add(buf.len())?)) else {
            return Err(IoError::InvalidAddress);
        };
        dst.copy_from_slice(buf);
        Ok(())
    }
}

struct TestClock;
impl Clock for TestClock {
    fn now(&self) -> FatDateTime {
        FatDateTime::new(2026, 9, 24, 12, 34, 56)
    }
}

type Fs = Fat32Fs<Img, TestClock>;
type Tree = BTreeMap<String, Option<Vec<u8>>>; // path -> contents (None = directory)

fn fs_tree(fs: &mut Fs) -> Tree {
    fn walk(fs: &mut Fs, dir: &str, out: &mut Tree) {
        for e in fs.read_dir(dir).unwrap_or_else(|err| panic!("read_dir {dir}: {err:?}")) {
            assert!(e.name != "." && e.name != "..");
            let p = if dir == "/" { format!("/{}", e.name) } else { format!("{dir}/{}", e.name) };
            if e.is_dir {
                out.insert(p.clone(), None);
                walk(fs, &p, out);
            } else {
                let mut buf = vec![0u8; e.size as usize];
                let n = fs.read_file(&p, 0, &mut buf).unwrap_or_else(|err| panic!("read {p}: {err:?}"));
                assert_eq!(n, buf.len(), "short read of {p}");
                out.insert(p, Some(buf));
            }
        }
    }
    let mut t = Tree::new();
    walk(fs, "/", &mut t);
    t
}

fn host_tree(root: &Path) -> Tree {
    fn walk(root: &Path, dir: &Path, out: &mut Tree) {
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            let rel = format!("/{}", p.strip_prefix(root).unwrap().to_str().unwrap());
            if p.is_dir() {
                out.insert(rel, None);
                walk(root, &p, out);
            } else {
                out.insert(rel, Some(std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut t = Tree::new();
    walk(root, root, &mut t);
    t
}

/// Copies a whole host tree into the image root with mcopy.
fn mcopy_in(img: &Path, src: &Path) {
    let mut args = vec!["-s".to_string(), "-i".into(), img.to_str().unwrap().into()];
    for e in std::fs::read_dir(src).unwrap() {
        args.push(e.unwrap().path().to_str().unwrap().into());
    }
    args.push("::/".into());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let (ok, out) = run(MCOPY, &refs);
    assert!(ok, "mcopy in: {out}");
}

/// Extracts the whole image with mcopy -s and returns the tree mtools sees.
fn mcopy_out(img: &Path, dir: &Path) -> Tree {
    let out = dir.join("extracted");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let (ok, log) = run(MCOPY, &["-s", "-n", "-i", img.to_str().unwrap(), "::*", &format!("{}/", out.display())]);
    let tree = host_tree(&out);
    assert!(ok || tree.is_empty(), "mcopy out: {log}");
    tree
}

fn assert_same(what: &str, got: &Tree, want: &Tree) {
    let gk: Vec<&String> = got.keys().collect();
    let wk: Vec<&String> = want.keys().collect();
    assert_eq!(gk, wk, "{what}: different paths");
    for (k, v) in want {
        assert!(got[k] == *v, "{what}: contents of {k} differ ({:?} vs {:?} bytes)", got[k].as_ref().map(Vec::len), v.as_ref().map(Vec::len));
    }
}

fn pattern(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// The fixture tree used by the read and write tests.
fn fixture() -> Tree {
    let mut t = Tree::new();
    t.insert("/HELLO.TXT".into(), Some(b"Hello, otter!\n".to_vec()));
    t.insert("/Long File Name With Spaces.txt".into(), Some(b"long".to_vec()));
    t.insert("/résumé de juillet.txt".into(), Some("é".repeat(700).into_bytes()));
    t.insert("/日本語のファイル.txt".into(), Some("日本語".as_bytes().to_vec()));
    t.insert("/empty".into(), Some(Vec::new()));
    t.insert("/one-cluster.bin".into(), Some(pattern(3, 512)));
    t.insert("/big.bin".into(), Some(pattern(5, 5 << 20)));
    t.insert("/dir".into(), None);
    t.insert("/dir/sub".into(), None);
    t.insert("/dir/sub/deep.txt".into(), Some(b"deep".to_vec()));
    t.insert("/many".into(), None);
    for i in 0..300 {
        t.insert(format!("/many/file number {i:03}.dat"), Some(pattern(i, (i as usize * 37) % 2000)));
    }
    t
}

fn write_host_tree(root: &Path, t: &Tree) {
    for (p, v) in t {
        let full = root.join(&p[1..]);
        match v {
            None => std::fs::create_dir_all(&full).unwrap(),
            Some(data) => {
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                std::fs::write(&full, data).unwrap();
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Read oracle.

#[test]
fn reads_exactly_what_mtools_wrote() {
    if !tools_present() {
        return;
    }
    let dir = workdir("read");
    let want = fixture();
    write_host_tree(&dir.join("src"), &want);
    for (name, make) in [("mkfs.img", 0), ("mformat.img", 1)] {
        let img = dir.join(name);
        if make == 0 {
            mkfs(&img, 65536);
        } else {
            std::fs::write(&img, vec![0u8; 64 << 20]).unwrap();
            let (ok, out) = run(MFORMAT, &["-i", img.to_str().unwrap(), "-F", "-T", "131072", "-h", "16", "-s", "63", "::"]);
            assert!(ok, "mformat: {out}");
        }
        mcopy_in(&img, &dir.join("src"));
        let mut fs = Fs::mount(Img::load(&img), TestClock).expect("mount");
        assert_same(name, &fs_tree(&mut fs), &want);
        // Coalescing: a whole-file read of the contiguous 5 MiB file needs few, large requests.
        let st = fs.stat("/BIG.BIN").expect("case-insensitive lookup");
        assert_eq!(st.size, 5 << 20);
        let mut buf = vec![0u8; 5 << 20];
        let r0 = fs.device().reads;
        assert_eq!(fs.read_file("/big.bin", 0, &mut buf).unwrap(), 5 << 20);
        let (r1, max) = (fs.device().reads, fs.device().max_read);
        assert!(r1 - r0 <= 64, "{name}: {} read_blocks calls for a contiguous 5 MiB file", r1 - r0);
        assert!(max >= 1 << 20, "{name}: largest read only {max} bytes");
        assert_eq!(buf, pattern(5, 5 << 20));
        // Partial reads at odd offsets.
        let mut part = vec![0u8; 1000];
        assert_eq!(fs.read_file("/big.bin", 12_345, &mut part).unwrap(), 1000);
        assert_eq!(part[..], pattern(5, 5 << 20)[12_345..13_345]);
        assert_eq!(fs.read_file("/big.bin", (5 << 20) - 10, &mut part).unwrap(), 10, "read at EOF is short");
        assert_eq!(fs.stat("/nope").err(), Some(FsError::NotFound));
        assert_eq!(fs.read_dir("/HELLO.TXT").err(), Some(FsError::NotADirectory));
    }
}

// ---------------------------------------------------------------------------------------------
// Write oracle.

#[test]
fn writes_that_fsck_and_mtools_accept() {
    if !tools_present() {
        return;
    }
    let dir = workdir("write");
    let img = dir.join("w.img");
    mkfs(&img, 65536);
    let mut fs = Fs::mount(Img::load(&img), TestClock).expect("mount");
    let mut want = fixture();
    // Create the fixture: directories first (BTreeMap order puts parents first).
    for (p, v) in &want {
        match v {
            None => fs.mkdir(p).unwrap_or_else(|e| panic!("mkdir {p}: {e:?}")),
            Some(data) => {
                fs.create_file(p).unwrap_or_else(|e| panic!("create {p}: {e:?}"));
                // Write in uneven chunks to cross cluster boundaries mid-write.
                let mut off = 0;
                let mut k = 1usize;
                while off < data.len() {
                    let n = (k * 997 % 70_000 + 1).min(data.len() - off);
                    fs.write_file(p, off as u64, &data[off..off + n]).unwrap();
                    off += n;
                    k += 1;
                }
            }
        }
    }
    // Edits: overwrite in the middle, append, grow with zeros, shrink, rename, move, delete.
    fs.write_file("/big.bin", 1_000_000, b"MIDDLE").unwrap();
    let big = want.get_mut("/big.bin").unwrap().as_mut().unwrap();
    big[1_000_000..1_000_006].copy_from_slice(b"MIDDLE");
    fs.write_file("/one-cluster.bin", 512, b"appended across a cluster boundary").unwrap();
    want.get_mut("/one-cluster.bin").unwrap().as_mut().unwrap().extend_from_slice(b"appended across a cluster boundary");
    fs.truncate("/HELLO.TXT", 3000).unwrap();
    want.get_mut("/HELLO.TXT").unwrap().as_mut().unwrap().resize(3000, 0);
    fs.truncate("/big.bin", 777).unwrap();
    want.get_mut("/big.bin").unwrap().as_mut().unwrap().truncate(777);
    fs.write_file("/empty", 5000, b"x").unwrap();
    let mut e = vec![0u8; 5000];
    e.push(b'x');
    want.insert("/empty".into(), Some(e));
    fs.rename("/Long File Name With Spaces.txt", "/dir/Renamed Long Name.TXT").unwrap();
    let v = want.remove("/Long File Name With Spaces.txt").unwrap();
    want.insert("/dir/Renamed Long Name.TXT".into(), v);
    fs.rename("/dir/sub", "/moved sub").unwrap();
    let v = want.remove("/dir/sub/deep.txt").unwrap();
    want.remove("/dir/sub");
    want.insert("/moved sub".into(), None);
    want.insert("/moved sub/deep.txt".into(), v);
    for i in (0..300).step_by(3) {
        fs.unlink(&format!("/many/FILE NUMBER {i:03}.DAT")).unwrap();
        want.remove(&format!("/many/file number {i:03}.dat"));
    }
    fs.mkdir("/gone").unwrap();
    fs.rmdir("/gone").unwrap();
    assert_eq!(fs.rmdir("/many").err(), Some(FsError::DirectoryNotEmpty));
    assert_eq!(fs.create_file("/hello.txt").err(), Some(FsError::AlreadyExists), "names are case-insensitive");
    assert_eq!(fs.unlink("/dir").err(), Some(FsError::IsADirectory));
    assert_eq!(fs.stat("/moved sub/deep.txt").unwrap().modified, TestClock.now());
    assert_same("otter-fat's own view", &fs_tree(&mut fs), &want);
    let dev = fs.unmount().expect("unmount");
    std::fs::write(&img, &dev.data).unwrap();
    fsck(&img);
    assert_same("mtools' view", &mcopy_out(&img, &dir), &want);
    let (_, listing) = run(MDIR, &["-i", img.to_str().unwrap(), "::/"]);
    assert!(listing.contains("2026-09-24") && listing.contains("12:34"), "mdir shows the test clock:\n{listing}");
    assert_fsinfo_matches_scan(&dev.data);
}

/// Independent check: FSInfo's free count equals a full scan of the FAT.
fn assert_fsinfo_matches_scan(img: &[u8]) {
    let u16_at = |o: usize| u16::from_le_bytes([img[o], img[o + 1]]) as usize;
    let u32_at = |o: usize| u32::from_le_bytes([img[o], img[o + 1], img[o + 2], img[o + 3]]) as usize;
    let (bps, spc, reserved, fats) = (u16_at(11), img[13] as usize, u16_at(14), img[16] as usize);
    let (total, fat_size, fsinfo) = (u32_at(32), u32_at(36), u16_at(48));
    let clusters = (total - reserved - fats * fat_size) / spc;
    let fat = reserved * bps;
    let free = (2..clusters + 2).filter(|c| u32_at(fat + c * 4) & 0x0fff_ffff == 0).count();
    let fsi = fsinfo * bps;
    assert_eq!(u32_at(fsi), 0x4161_5252, "FSInfo lead signature");
    assert_eq!(u32_at(fsi + 488), free, "FSInfo free count vs a full FAT scan");
}

// ---------------------------------------------------------------------------------------------
// Differential test against an in-memory model.

const NAMES: &[&str] = &[
    "a.txt", "B.TXT", "notes", "README.md", "Mixed Case Name.Md", "x", "data.bin", "UPPER", "lower",
    "Long File Name With Spaces.txt", "dotted.name.with.many.dots",
    "a-very-long-name-that-needs-several-long-name-entries-to-store-it-completely.txt",
    "日本語のファイル.txt", "résumé de juillet.txt", "same name", "SAME NAME 2",
];

#[derive(Default)]
struct Model {
    // lowercased path -> (path as created, contents or None for a directory)
    map: BTreeMap<String, (String, Option<Vec<u8>>)>,
}

fn key(p: &str) -> String {
    p.to_ascii_lowercase()
}

fn parent(p: &str) -> &str {
    match p.rfind('/') {
        Some(0) => "/",
        Some(i) => &p[..i],
        None => "/",
    }
}

impl Model {
    fn get(&self, p: &str) -> Option<&(String, Option<Vec<u8>>)> {
        self.map.get(&key(p))
    }
    fn is_dir(&self, p: &str) -> bool {
        p == "/" || matches!(self.get(p), Some((_, None)))
    }
    fn parent_ok(&self, p: &str) -> Result<(), FsError> {
        let par = parent(p);
        if par == "/" || self.is_dir(par) {
            Ok(())
        } else if self.get(par).is_some() {
            Err(FsError::NotADirectory)
        } else {
            Err(FsError::NotFound)
        }
    }
    /// The error for an entry that must exist but does not.
    fn missing(&self, p: &str) -> FsError {
        self.parent_ok(p).err().unwrap_or(FsError::NotFound)
    }
    /// Display path of p using the stored case of every existing ancestor.
    fn display(&self, p: &str) -> String {
        let par = parent(p);
        let base = &p[p.rfind('/').unwrap() + 1..];
        if par == "/" { format!("/{base}") } else { format!("{}/{base}", self.get(par).unwrap().0) }
    }
    fn children(&self, p: &str) -> Vec<String> {
        let pre = if p == "/" { "/".to_string() } else { format!("{}/", key(p)) };
        self.map.keys().filter(|k| k.starts_with(&pre) && !k[pre.len()..].contains('/')).cloned().collect()
    }
    fn tree(&self) -> Tree {
        self.map.values().map(|(d, v)| (d.clone(), v.clone())).collect()
    }
    fn total_bytes(&self) -> usize {
        self.map.values().filter_map(|(_, v)| v.as_ref().map(Vec::len)).sum()
    }
}

fn random_path(m: &Model, rng: &mut u64) -> String {
    let mut next = || {
        *rng ^= *rng << 13;
        *rng ^= *rng >> 7;
        *rng ^= *rng << 17;
        *rng
    };
    let dirs: Vec<String> = std::iter::once("/".to_string())
        .chain(m.map.values().filter(|(_, v)| v.is_none()).map(|(d, _)| d.clone()))
        .collect();
    let mut par = dirs[next() as usize % dirs.len()].clone();
    if next() % 20 == 0 {
        par = "/no such dir".into();
    }
    if next() % 20 == 0
        && let Some((d, _)) = m.map.values().find(|(_, v)| v.is_some())
    {
        par = d.clone(); // a file used as a directory
    }
    let mut name = NAMES[next() as usize % NAMES.len()].to_string();
    if next() % 3 == 0 {
        name = if next() % 2 == 0 { name.to_uppercase() } else { name.to_lowercase() };
        if !name.is_ascii() {
            name = NAMES[0].to_string();
        }
    }
    if par == "/" { format!("/{name}") } else { format!("{par}/{name}") }
}

fn differential(seed: u64) {
    let dir = workdir(&format!("diff-{seed}"));
    let img = dir.join("d.img");
    mkfs(&img, 65536);
    let mut fs = Fs::mount(Img::load(&img), TestClock).expect("mount");
    let mut m = Model::default();
    let mut rng = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    for step in 0..2000 {
        let p = random_path(&m, &mut rng);
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let r = rng;
        let op = r % 8;
        let got: Result<(), FsError>;
        let want: Result<(), FsError>;
        match op {
            0 | 1 => {
                // create_file (more often than the other ops)
                want = m.parent_ok(&p).and_then(|_| if m.get(&p).is_some() { Err(FsError::AlreadyExists) } else { Ok(()) });
                got = fs.create_file(&p);
                if want.is_ok() {
                    let d = m.display(&p);
                    m.map.insert(key(&p), (d, Some(Vec::new())));
                }
            }
            2 => {
                want = m.parent_ok(&p).and_then(|_| if m.get(&p).is_some() { Err(FsError::AlreadyExists) } else { Ok(()) });
                got = fs.mkdir(&p);
                if want.is_ok() {
                    let d = m.display(&p);
                    m.map.insert(key(&p), (d, None));
                }
            }
            3 | 4 => {
                let off = (r >> 8) as usize % 200_000;
                let len = (r >> 32) as usize % 40_000;
                let data = pattern(r, len);
                want = match m.get(&p) {
                    None => Err(m.missing(&p)),
                    Some((_, None)) => Err(FsError::IsADirectory),
                    Some(_) if m.total_bytes() + off + len > 40 << 20 => continue,
                    Some(_) => Ok(()),
                };
                got = fs.write_file(&p, off as u64, &data);
                if want.is_ok() {
                    let f = m.map.get_mut(&key(&p)).unwrap().1.as_mut().unwrap();
                    if f.len() < off + len {
                        f.resize(off + len, 0);
                    }
                    f[off..off + len].copy_from_slice(&data);
                }
            }
            5 => {
                let len = (r >> 8) as usize % 100_000;
                want = match m.get(&p) {
                    None => Err(m.missing(&p)),
                    Some((_, None)) => Err(FsError::IsADirectory),
                    Some(_) => Ok(()),
                };
                got = fs.truncate(&p, len as u64);
                if want.is_ok() {
                    m.map.get_mut(&key(&p)).unwrap().1.as_mut().unwrap().resize(len, 0);
                }
            }
            6 => {
                let is_dir = matches!(m.get(&p), Some((_, None)));
                if is_dir {
                    want = if m.children(&p).is_empty() { Ok(()) } else { Err(FsError::DirectoryNotEmpty) };
                    got = fs.rmdir(&p);
                } else {
                    want = if m.get(&p).is_some() { Ok(()) } else { Err(m.missing(&p)) };
                    got = fs.unlink(&p);
                }
                if want.is_ok() {
                    m.map.remove(&key(&p));
                }
            }
            _ => {
                let to = random_path(&m, &mut rng);
                let (kf, kt) = (key(&p), key(&to));
                if kf == kt || kt.starts_with(&format!("{kf}/")) || p == "/" {
                    continue; // no case-only renames, no moving a directory into itself
                }
                want = if m.get(&p).is_none() {
                    Err(m.missing(&p))
                } else {
                    m.parent_ok(&to).and_then(|_| if m.get(&to).is_some() { Err(FsError::AlreadyExists) } else { Ok(()) })
                };
                got = fs.rename(&p, &to);
                if want.is_ok() {
                    let moved: Vec<String> = m.map.keys().filter(|k| *k == &kf || k.starts_with(&format!("{kf}/"))).cloned().collect();
                    let new_disp = m.display(&to);
                    for k in moved {
                        // Keys are ASCII-lowercased display paths, so both have the same length and
                        // the part below the moved entry starts at the same byte offset.
                        let (disp, v) = m.map.remove(&k).unwrap();
                        let suffix = disp[kf.len()..].to_string();
                        m.map.insert(format!("{kt}{}", &k[kf.len()..]), (format!("{new_disp}{suffix}"), v));
                    }
                }
            }
        }
        assert_eq!(got, want, "seed {seed} step {step}: op {op} on {p:?}");
        if step % 200 == 199 {
            assert_same(&format!("seed {seed} after {} ops", step + 1), &fs_tree(&mut fs), &m.tree());
        }
    }
    fs.sync().unwrap();
    let dev = fs.unmount().unwrap();
    std::fs::write(&img, &dev.data).unwrap();
    fsck(&img);
    assert_same(&format!("seed {seed}: mtools' view"), &mcopy_out(&img, &dir), &m.tree());
    assert_fsinfo_matches_scan(&dev.data);
}

#[test]
fn differential_seed_1() {
    if tools_present() {
        differential(1);
    }
}

#[test]
fn differential_seed_2() {
    if tools_present() {
        differential(2);
    }
}

#[test]
fn differential_seed_3() {
    if tools_present() {
        differential(3);
    }
}

// ---------------------------------------------------------------------------------------------
// Partitions (Python-built MBR and GPT images; CRCs from zlib, independent of the crate).

const MAKE_DISK: &str = r#"
import struct, sys, uuid, zlib
out, part_img, kind, corrupt = sys.argv[1:5]
part = open(part_img, 'rb').read()
start = 2048
count = len(part) // 512
total = start + count + 2048
disk = bytearray(total * 512)
disk[start*512:start*512+len(part)] = part
def mbr_entry(ptype, lba, n):
    return bytes([0, 0, 2, 0, ptype, 0xfe, 0xff, 0xff]) + struct.pack('<II', lba, n)
if kind == 'mbr':
    disk[446:462] = mbr_entry(0x0c, start, count)
else:
    disk[446:462] = mbr_entry(0xee, 1, min(total - 1, 0xffffffff))
    entry = uuid.UUID('EBD0A0A2-B9E5-4433-87C0-68B6B72699C7').bytes_le + uuid.uuid4().bytes_le
    entry += struct.pack('<QQQ', start, start + count - 1, 0) + 'OTTER'.encode('utf-16-le').ljust(72, b'\0')
    entries = entry.ljust(128 * 128, b'\0')
    ecrc = zlib.crc32(entries) & 0xffffffff
    def header(my, alt, elba):
        h = struct.pack('<8sIIIIQQQQ16sQIII', b'EFI PART', 0x10000, 92, 0, 0, my, alt, 34, total - 34,
                        uuid.uuid4().bytes_le, elba, 128, 128, ecrc)
        return h[:16] + struct.pack('<I', zlib.crc32(h) & 0xffffffff) + h[20:]
    disk[512:512+92] = header(1, total - 1, 2)
    disk[2*512:2*512+len(entries)] = entries
    disk[(total-33)*512:(total-33)*512+len(entries)] = entries
    disk[(total-1)*512:(total-1)*512+92] = header(total - 1, 1, total - 33)
    if corrupt in ('primary', 'both'):
        disk[512 + 40] ^= 0xff
    if corrupt == 'entries':
        disk[2*512 + 40] ^= 0xff
        disk[(total-33)*512 + 40] ^= 0xff
disk[510:512] = b'\x55\xaa'
open(out, 'wb').write(disk)
"#;

#[test]
fn partitions_mbr_gpt_and_backup_header() {
    if !tools_present() {
        return;
    }
    let dir = workdir("part");
    let vol = dir.join("vol.img");
    mkfs(&vol, 65536);
    std::fs::write(dir.join("hello.txt"), b"inside a partition").unwrap();
    let (ok, out) = run(MCOPY, &["-i", vol.to_str().unwrap(), dir.join("hello.txt").to_str().unwrap(), "::/HELLO.TXT"]);
    assert!(ok, "{out}");
    let count = std::fs::metadata(&vol).unwrap().len() / 512;
    for (kind, corrupt, ok_expected) in
        [("mbr", "none", true), ("gpt", "none", true), ("gpt", "primary", true), ("gpt", "entries", false)]
    {
        let disk = dir.join(format!("{kind}-{corrupt}.img"));
        let st = Command::new("python3")
            .args(["-c", MAKE_DISK, disk.to_str().unwrap(), vol.to_str().unwrap(), kind, corrupt])
            .status()
            .unwrap();
        assert!(st.success());
        let mut dev = Img::load(&disk);
        match detect_partition(&mut dev) {
            Ok(info) if ok_expected => {
                assert_eq!((info.lba_start, info.lba_count), (2048, count), "{kind}/{corrupt}");
                let part = Partition::new(dev, info.lba_start, info.lba_count).expect("partition device");
                let mut fs = Fat32Fs::mount(part, TestClock).expect("mount the partition");
                let mut buf = [0u8; 64];
                let n = fs.read_file("/hello.txt", 0, &mut buf).unwrap();
                assert_eq!(&buf[..n], b"inside a partition");
            }
            Ok(_) => panic!("{kind}/{corrupt}: corrupted partition entries must be rejected"),
            Err(e) => assert!(!ok_expected, "{kind}/{corrupt}: {e}"),
        }
    }
    // A superfloppy (no table) is the whole device.
    let mut dev = Img::load(&vol);
    let info = detect_partition(&mut dev).expect("superfloppy");
    assert_eq!((info.lba_start, info.lba_count), (0, count));
}

// ---------------------------------------------------------------------------------------------
// Robustness.

#[test]
fn out_of_space_is_an_error_not_a_corruption() {
    if !tools_present() {
        return;
    }
    let dir = workdir("full");
    let img = dir.join("f.img");
    mkfs(&img, 34 * 1024);
    let mut fs = Fs::mount(Img::load(&img), TestClock).unwrap();
    fs.create_file("/fill").unwrap();
    let chunk = vec![0xabu8; 1 << 20];
    let mut off = 0u64;
    let err = loop {
        match fs.write_file("/fill", off, &chunk) {
            Ok(()) => off += chunk.len() as u64,
            Err(e) => break e,
        }
        assert!(off < 64 << 20, "a 34 MiB volume accepted {off} bytes");
    };
    assert_eq!(err, FsError::NoSpace);
    fs.create_file("/after").unwrap_or(());
    let dev = fs.unmount().unwrap();
    std::fs::write(&img, &dev.data).unwrap();
    fsck(&img);
    assert_fsinfo_matches_scan(&dev.data);
}

#[test]
fn corrupted_metadata_never_panics_or_hangs() {
    if !tools_present() {
        return;
    }
    let dir = workdir("fuzz");
    let img = dir.join("z.img");
    mkfs(&img, 65536);
    let mut small = Tree::new();
    small.insert("/a".into(), None);
    small.insert("/a/b.txt".into(), Some(pattern(1, 3000)));
    small.insert("/Long Name Here.txt".into(), Some(pattern(2, 100)));
    small.insert("/c.bin".into(), Some(pattern(3, 70_000)));
    write_host_tree(&dir.join("src"), &small);
    mcopy_in(&img, &dir.join("src"));
    let base = std::fs::read(&img).unwrap();
    let u16_at = |o: usize| u16::from_le_bytes([base[o], base[o + 1]]) as usize;
    let u32_at = |o: usize| u32::from_le_bytes([base[o], base[o + 1], base[o + 2], base[o + 3]]) as usize;
    // Metadata: reserved sectors, both FATs and the first 64 KiB of the data area (root dir etc.).
    let meta_end = (u16_at(14) + base[16] as usize * u32_at(36)) * 512 + 65536;
    let mut x = 0x1234_5678_9abc_def1u64;
    for round in 0..1000 {
        let mut data = base.clone();
        for _ in 0..1 + round % 4 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let i = (x as usize >> 8) % meta_end;
            data[i] ^= 1 << (x % 8);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok(mut fs) = Fs::mount(Img { data, reads: 0, max_read: 0 }, TestClock) {
                fn walk(fs: &mut Fs, dir: &str, depth: usize) {
                    if depth > 8 {
                        return;
                    }
                    let Ok(entries) = fs.read_dir(dir) else { return };
                    for e in entries.into_iter().take(1000) {
                        let p = if dir == "/" { format!("/{}", e.name) } else { format!("{dir}/{}", e.name) };
                        if e.is_dir {
                            walk(fs, &p, depth + 1);
                        } else {
                            let mut buf = vec![0u8; (e.size as usize).min(1 << 20)];
                            let _ = fs.read_file(&p, 0, &mut buf);
                        }
                    }
                }
                walk(&mut fs, "/", 0);
            }
            let _ = tx.send(());
        });
        match rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(()) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!("round {round}: mount/walk hung (loop detection?)"),
            Err(_) => panic!("round {round}: mount/walk panicked"),
        }
    }
}
