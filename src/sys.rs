//! Data layer: everything that talks to the OS lives here, so a future GUI
//! (Tauri, egui, SwiftUI via FFI…) can reuse it unchanged.

use std::collections::{HashMap, HashSet};

use sysinfo::{
    CpuRefreshKind, Disks, MemoryRefreshKind, Pid, ProcessRefreshKind, ProcessesToUpdate,
    RefreshKind, Signal, System, UpdateKind, Users,
};

#[derive(Clone, Copy, Default)]
pub struct MemInfo {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

#[derive(Clone)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    pub cmd: String,
    pub user: String,
    pub rss: u64,
    pub cpu: f32,
    /// Owned by root / a service account rather than a human user.
    pub system: bool,
}

#[derive(Clone)]
pub struct DiskInfo {
    pub mount: String,
    pub total: u64,
    pub avail: u64,
}

impl DiskInfo {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.avail)
    }
}

/// Processes summed up by name: answers "which app is eating my RAM".
#[derive(Clone)]
pub struct AppGroup {
    pub name: String,
    pub count: usize,
    pub rss: u64,
    pub cpu: f32,
    pub users: Vec<String>,
    pub pids: Vec<u32>,
}

pub struct Snapshot {
    pub mem: MemInfo,
    pub cpu: f32,
    pub ncpu: usize,
    pub load: [f64; 3],
    pub uptime: u64,
    pub procs: Vec<ProcInfo>,
    pub disks: Vec<DiskInfo>,
}

impl Snapshot {
    pub fn groups<'a>(procs: impl Iterator<Item = &'a ProcInfo>) -> Vec<AppGroup> {
        let mut map: HashMap<&str, AppGroup> = HashMap::new();
        for p in procs {
            let g = map.entry(p.name.as_str()).or_insert_with(|| AppGroup {
                name: p.name.clone(),
                count: 0,
                rss: 0,
                cpu: 0.0,
                users: Vec::new(),
                pids: Vec::new(),
            });
            g.count += 1;
            g.rss += p.rss;
            g.cpu += p.cpu;
            g.pids.push(p.pid);
            if !g.users.contains(&p.user) {
                g.users.push(p.user.clone());
            }
        }
        map.into_values().collect()
    }
}

pub struct Monitor {
    sys: System,
    disks: Disks,
    users: Users,
    self_pid: u32,
}

const PSEUDO_FS: &[&str] = &[
    "tmpfs", "devtmpfs", "overlay", "squashfs", "proc", "sysfs", "ramfs", "efivarfs",
    "autofs", "devfs", "nullfs", "fuse.snapfuse", "fuse.portal",
];

/// Human accounts start at uid 1000 on Linux and 500 on macOS; below that
/// (root, daemons) and `nobody` are system accounts.
fn is_system_user(uid: Option<String>, name: &str) -> bool {
    const FIRST_HUMAN_UID: u32 = if cfg!(target_os = "macos") { 500 } else { 1000 };
    match uid.and_then(|u| u.parse::<u32>().ok()) {
        Some(n) => n < FIRST_HUMAN_UID || n >= 65534,
        // Windows SIDs / unknown owner
        None => matches!(name, "?" | "SYSTEM" | "LOCAL SERVICE" | "NETWORK SERVICE"),
    }
}

/// Linux `comm` is truncated to 15 chars and often renamed ("MainThread"):
/// prefer the executable's basename from argv[0] when available.
fn display_name(comm: &str, cmd: &[std::ffi::OsString], exe: Option<&std::path::Path>) -> String {
    let Some(first) = cmd.first().map(|a| a.to_string_lossy()) else {
        return comm.to_string();
    };
    // Processes that rewrite argv into one string ("sshd: /usr/sbin/sshd -D …",
    // "postgres: checkpointer"): trust the real executable, else the first word.
    if cmd.len() == 1 && first.contains(' ') {
        if let Some(name) = exe.and_then(|e| e.file_name()) {
            return name.to_string_lossy().into_owned();
        }
    }
    let first: &str = if cmd.len() == 1 && first.contains(' ') {
        first.split_whitespace().next().unwrap_or(&first)
    } else {
        &first
    };
    let base = first.rsplit(['/', '\\']).next().unwrap_or(first).trim_end_matches(':');
    if base.is_empty() { comm.to_string() } else { base.to_string() }
}

impl Monitor {
    pub fn new() -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_memory(MemoryRefreshKind::everything())
                .with_cpu(CpuRefreshKind::nothing().with_cpu_usage()),
        );
        let mut m = Monitor {
            sys,
            disks: Disks::new_with_refreshed_list(),
            users: Users::new_with_refreshed_list(),
            self_pid: std::process::id(),
        };
        // CPU usage is a delta between two samples: prime it so the first
        // frame doesn't show 0% everywhere.
        m.refresh_procs();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        m
    }

    fn refresh_procs(&mut self) {
        self.sys.refresh_memory();
        self.sys.refresh_cpu_usage();
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_memory()
                .with_cpu()
                .with_user(UpdateKind::OnlyIfNotSet)
                .with_cmd(UpdateKind::OnlyIfNotSet)
                .with_exe(UpdateKind::OnlyIfNotSet)
                .without_tasks(),
        );
    }

    pub fn snapshot(&mut self) -> Snapshot {
        self.refresh_procs();
        self.disks.refresh(true);

        let s = &self.sys;
        let mem = MemInfo {
            total: s.total_memory(),
            used: s.used_memory(),
            available: s.available_memory(),
            swap_total: s.total_swap(),
            swap_used: s.used_swap(),
        };

        let procs = s
            .processes()
            .values()
            .filter(|p| p.thread_kind().is_none()) // skip threads
            .map(|p| {
                let user = p
                    .user_id()
                    .map(|uid| {
                        self.users
                            .get_user_by_id(uid)
                            .map(|u| u.name().to_string())
                            .unwrap_or_else(|| uid.to_string())
                    })
                    .unwrap_or_else(|| "?".into());
                let cmd = p
                    .cmd()
                    .iter()
                    .map(|a| a.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" ");
                let system = is_system_user(p.user_id().map(|u| u.to_string()), &user);
                ProcInfo {
                    system,
                    pid: p.pid().as_u32(),
                    name: display_name(p.name().to_string_lossy().as_ref(), p.cmd(), p.exe()),
                    cmd,
                    user,
                    rss: p.memory(),
                    cpu: p.cpu_usage(),
                }
            })
            .collect();

        let mut seen = HashSet::new();
        let mut disks: Vec<DiskInfo> = self
            .disks
            .list()
            .iter()
            .filter(|d| d.total_space() > 0)
            .filter(|d| !PSEUDO_FS.contains(&d.file_system().to_string_lossy().as_ref()))
            .map(|d| {
                (
                    d.name().to_string_lossy().into_owned(),
                    DiskInfo {
                        mount: d.mount_point().to_string_lossy().into_owned(),
                        total: d.total_space(),
                        avail: d.available_space(),
                    },
                )
            })
            .collect::<Vec<_>>()
            .into_iter()
            // same device mounted many times (bind mounts, snaps): keep the shortest path
            .fold(Vec::<(String, DiskInfo)>::new(), |mut acc, (dev, d)| {
                if let Some(e) = acc.iter_mut().find(|(n, _)| *n == dev) {
                    if d.mount.len() < e.1.mount.len() {
                        e.1 = d;
                    }
                } else {
                    acc.push((dev, d));
                }
                acc
            })
            .into_iter()
            .map(|(_, d)| d)
            .filter(|d| seen.insert(d.mount.clone()))
            .collect();
        disks.sort_by(|a, b| a.mount.cmp(&b.mount));

        let l = System::load_average();
        Snapshot {
            mem,
            cpu: s.global_cpu_usage(),
            ncpu: s.cpus().len(),
            load: [l.one, l.five, l.fifteen],
            uptime: System::uptime(),
            procs,
            disks,
        }
    }

    /// Sends SIGTERM (graceful) or SIGKILL (force). Returns a human message on error.
    pub fn kill(&self, pid: u32, force: bool) -> Result<(), String> {
        if pid == self.self_pid {
            return Err("can't kill myself".into());
        }
        let p = self
            .sys
            .process(Pid::from_u32(pid))
            .ok_or_else(|| format!("PID {pid} no longer exists"))?;
        let sig = if force { Signal::Kill } else { Signal::Term };
        match p.kill_with(sig) {
            Some(true) => Ok(()),
            Some(false) => Err(format!("PID {pid}: permission denied (try sudo)")),
            None => {
                if p.kill() {
                    Ok(())
                } else {
                    Err(format!("PID {pid}: could not kill"))
                }
            }
        }
    }
}
