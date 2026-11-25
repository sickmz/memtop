//! Disk usage scanner (ncdu-style): walks one filesystem in parallel, in a
//! background thread, and builds a size-sorted tree to browse.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use rayon::prelude::*;

/// Files smaller than this are folded into one "small files" entry per folder,
/// so the tree stays small even with millions of files.
const SMALL: u64 = 1 << 20;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Dir,
    File,
    /// N files below [`SMALL`], summed.
    Small(u64),
    /// Folder we were not allowed to read.
    Denied,
}

pub struct Node {
    pub name: String,
    pub size: u64,
    pub kind: Kind,
    /// Files contained, recursively.
    pub files: u64,
    pub children: Vec<Node>,
}

impl Node {
    pub fn at(&self, path: &[usize]) -> &Node {
        path.iter().fold(self, |n, &i| &n.children[i])
    }
}

#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    pub denied: AtomicU64,
    cancel: AtomicBool,
}

pub struct Scan {
    pub root: PathBuf,
    pub progress: Arc<Progress>,
    result: Arc<Mutex<Option<Node>>>,
    started: Instant,
    took: Arc<Mutex<Option<Duration>>>,
}

impl Scan {
    pub fn start(root: &Path) -> Scan {
        let progress = Arc::new(Progress::default());
        let result = Arc::new(Mutex::new(None));
        let took = Arc::new(Mutex::new(None));
        let (p, r, t, path) = (progress.clone(), result.clone(), took.clone(), root.to_path_buf());
        let started = Instant::now();
        std::thread::spawn(move || {
            let ctx = Ctx { dev: dev_of(&path), progress: p, seen: Mutex::new(HashSet::new()) };
            let node = scan_dir(&path, path.to_string_lossy().into_owned(), &ctx);
            if !ctx.progress.cancel.load(Relaxed) {
                *t.lock().unwrap() = Some(started.elapsed());
                *r.lock().unwrap() = Some(node);
            }
        });
        Scan { root: root.to_path_buf(), progress, result, started, took }
    }

    pub fn cancel(&self) {
        self.progress.cancel.store(true, Relaxed);
    }

    pub fn elapsed(&self) -> Duration {
        self.took.lock().unwrap().unwrap_or_else(|| self.started.elapsed())
    }

    /// Runs `f` on the finished tree, or returns None while still scanning.
    pub fn with<R>(&self, f: impl FnOnce(&Node) -> R) -> Option<R> {
        self.result.lock().unwrap().as_ref().map(f)
    }
}

struct Ctx {
    dev: u64,
    progress: Arc<Progress>,
    /// (dev, inode) of hard-linked files already counted.
    seen: Mutex<HashSet<(u64, u64)>>,
}

#[cfg(unix)]
fn dev_of(p: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    fs::symlink_metadata(p).map(|m| m.dev()).unwrap_or(0)
}
#[cfg(not(unix))]
fn dev_of(_: &Path) -> u64 {
    0
}

/// Space actually taken on disk (blocks), not apparent length.
#[cfg(unix)]
fn usage(m: &fs::Metadata, ctx: &Ctx) -> u64 {
    use std::os::unix::fs::MetadataExt;
    if m.nlink() > 1 && !ctx.seen.lock().unwrap().insert((m.dev(), m.ino())) {
        return 0;
    }
    m.blocks() * 512
}
#[cfg(not(unix))]
fn usage(m: &fs::Metadata, _: &Ctx) -> u64 {
    m.len()
}

#[cfg(unix)]
fn same_fs(m: &fs::Metadata, ctx: &Ctx) -> bool {
    use std::os::unix::fs::MetadataExt;
    m.dev() == ctx.dev
}
#[cfg(not(unix))]
fn same_fs(_: &fs::Metadata, _: &Ctx) -> bool {
    true
}

fn scan_dir(path: &Path, name: String, ctx: &Ctx) -> Node {
    let mut node = Node { name, size: 0, kind: Kind::Dir, files: 0, children: Vec::new() };
    if ctx.progress.cancel.load(Relaxed) {
        return node;
    }
    let Ok(rd) = fs::read_dir(path) else {
        ctx.progress.denied.fetch_add(1, Relaxed);
        node.kind = Kind::Denied;
        return node;
    };

    let mut subdirs = Vec::new();
    let (mut small_n, mut small_size) = (0u64, 0u64);
    for e in rd.flatten() {
        let Ok(m) = e.metadata() else { continue }; // does not follow symlinks
        let name = e.file_name().to_string_lossy().into_owned();
        if m.is_dir() {
            if same_fs(&m, ctx) {
                subdirs.push((e.path(), name)); // other mounts (/proc, /boot…) are skipped
            }
            continue;
        }
        let size = usage(&m, ctx);
        ctx.progress.files.fetch_add(1, Relaxed);
        ctx.progress.bytes.fetch_add(size, Relaxed);
        node.files += 1;
        node.size += size;
        if size >= SMALL {
            node.children.push(Node { name, size, kind: Kind::File, files: 1, children: Vec::new() });
        } else {
            small_n += 1;
            small_size += size;
        }
    }

    let dirs: Vec<Node> = subdirs.into_par_iter().map(|(p, n)| scan_dir(&p, n, ctx)).collect();
    for d in dirs {
        node.size += d.size;
        node.files += d.files;
        node.children.push(d);
    }
    if small_n > 0 {
        node.children.push(Node {
            name: String::new(),
            size: small_size,
            kind: Kind::Small(small_n),
            files: small_n,
            children: Vec::new(),
        });
    }
    node.children.sort_by(|a, b| b.size.cmp(&a.size));
    node
}
