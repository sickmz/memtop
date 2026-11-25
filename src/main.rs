mod scan;
mod sys;
mod ui;

use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant},
};

use ratatui::crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseButton, MouseEventKind,
    },
    execute,
};
use sys::{Monitor, Snapshot};

const TICK: Duration = Duration::from_millis(1500);
/// How long footer messages stay: confirmations are brief, errors linger a bit.
fn status_ttl(is_error: bool) -> Duration {
    Duration::from_millis(if is_error { 3500 } else { 1500 })
}

#[derive(Clone, Copy, PartialEq)]
pub enum SortKey {
    Mem,
    Cpu,
    Pid,
    Name,
}

impl SortKey {
    fn next(self) -> Self {
        match self {
            SortKey::Mem => SortKey::Cpu,
            SortKey::Cpu => SortKey::Name,
            SortKey::Name => SortKey::Pid,
            SortKey::Pid => SortKey::Mem,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Mem => "RAM",
            SortKey::Cpu => "CPU",
            SortKey::Pid => "PID",
            SortKey::Name => "name",
        }
    }
}

/// One table row, either a process or an app group.
pub struct Row {
    pub key: String, // stable identity across refreshes: pid or app name
    pub pids: Vec<u32>,
    pub name: String,
    pub count: usize,
    pub user: String,
    pub rss: u64,
    pub cpu: f32,
    pub detail: String,
}

pub struct KillPrompt {
    pub label: String,
    pub pids: Vec<u32>,
    pub rss: u64,
}

/// Browsing the disk usage of one mount point.
pub struct Explorer {
    pub mount: String,
    /// Child index taken at each level from the root; also restores the cursor on "up".
    pub path: Vec<usize>,
    pub sel: usize,
}

pub struct App {
    pub snap: Snapshot,
    pub hide_system: bool,
    pub grouped: bool,
    pub sort: SortKey,
    pub reverse: bool,
    pub filter: String,
    pub editing_filter: bool,
    pub rows: Vec<Row>,
    pub selected: usize,
    selected_key: Option<String>,
    pub kill: Option<KillPrompt>,
    pub status: Option<(String, bool, Instant)>, // (text, is_error, when)
    pub table_area: ratatui::layout::Rect,
    pub table_offset: usize,
    pub show_help: bool,
    pub scans: HashMap<String, scan::Scan>,
    pub explorer: Option<Explorer>,
    pub disk_focus: bool,
    pub disk_sel: usize,
    pub disk_area: ratatui::layout::Rect,
}

impl App {
    fn new(snap: Snapshot) -> Self {
        let mut a = App {
            snap,
            hide_system: false,
            grouped: false,
            sort: SortKey::Mem,
            reverse: false,
            filter: String::new(),
            editing_filter: false,
            rows: Vec::new(),
            selected: 0,
            selected_key: None,
            kill: None,
            status: None,
            table_area: Default::default(),
            table_offset: 0,
            show_help: false,
            scans: HashMap::new(),
            explorer: None,
            disk_focus: false,
            disk_sel: 0,
            disk_area: Default::default(),
        };
        a.rebuild();
        a
    }

    /// Recompute visible rows and keep the cursor on the same pid/app.
    pub fn rebuild(&mut self) {
        let f = self.filter.to_lowercase();
        let matches = |s: &[&str]| f.is_empty() || s.iter().any(|x| x.to_lowercase().contains(&f));

        let mut rows: Vec<Row> = if self.grouped {
            Snapshot::groups(self.snap.procs.iter().filter(|p| !(self.hide_system && p.system)))
                .into_iter()
                .filter(|g| matches(&[&g.name, &g.users.join(" ")]))
                .map(|g| {
                    let mut pids = g.pids.clone();
                    pids.sort_unstable();
                    let detail = pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(" ");
                    Row {
                        key: format!("g:{}", g.name),
                        pids,
                        name: g.name,
                        count: g.count,
                        user: g.users.join(","),
                        rss: g.rss,
                        cpu: g.cpu,
                        detail,
                    }
                })
                .collect()
        } else {
            self.snap
                .procs
                .iter()
                .filter(|p| !(self.hide_system && p.system))
                .filter(|p| matches(&[&p.name, &p.cmd, &p.user, &p.pid.to_string()]))
                .map(|p| Row {
                    key: format!("p:{}", p.pid),
                    pids: vec![p.pid],
                    name: p.name.clone(),
                    count: 1,
                    user: p.user.clone(),
                    rss: p.rss,
                    cpu: p.cpu,
                    detail: if p.cmd.is_empty() { format!("[{}]", p.name) } else { p.cmd.clone() },
                })
                .collect()
        };

        rows.sort_by(|a, b| {
            let o = match self.sort {
                SortKey::Mem => b.rss.cmp(&a.rss),
                SortKey::Cpu => b.cpu.total_cmp(&a.cpu),
                SortKey::Pid => a.pids[0].cmp(&b.pids[0]),
                SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            };
            if self.reverse { o.reverse() } else { o }
        });

        if let Some(k) = &self.selected_key {
            if let Some(i) = rows.iter().position(|r| &r.key == k) {
                self.selected = i;
            }
        }
        self.rows = rows;
        self.clamp();
    }

    fn clamp(&mut self) {
        if self.rows.is_empty() {
            self.selected = 0;
            self.selected_key = None;
        } else {
            self.selected = self.selected.min(self.rows.len() - 1);
            self.selected_key = Some(self.rows[self.selected].key.clone());
        }
    }

    fn move_by(&mut self, d: isize) {
        let n = self.rows.len() as isize;
        if n == 0 {
            return;
        }
        self.selected = (self.selected as isize + d).clamp(0, n - 1) as usize;
        self.clamp();
    }

    fn set_status(&mut self, msg: impl Into<String>, err: bool) {
        self.status = Some((msg.into(), err, Instant::now()));
    }

    fn open_kill(&mut self) {
        if let Some(r) = self.rows.get(self.selected) {
            let label = if r.count > 1 {
                format!("{} \"{}\" processes", r.count, r.name)
            } else {
                format!("\"{}\" (PID {})", r.name, r.pids[0])
            };
            // pids captured now: a re-sort before confirming can't change the target
            self.kill = Some(KillPrompt { label, pids: r.pids.clone(), rss: r.rss });
        }
    }

    fn open_disk(&mut self, i: usize) {
        let Some(d) = self.snap.disks.get(i) else { return };
        let mount = d.mount.clone();
        self.scans.entry(mount.clone()).or_insert_with(|| scan::Scan::start(Path::new(&mount)));
        self.disk_sel = i;
        self.disk_focus = false;
        self.explorer = Some(Explorer { mount, path: Vec::new(), sel: 0 });
    }

    fn close_explorer(&mut self) {
        if let Some(e) = self.explorer.take() {
            // an unfinished scan is useless: stop it instead of burning CPU in the background
            if self.scans.get(&e.mount).is_some_and(|s| s.with(|_| ()).is_none()) {
                if let Some(s) = self.scans.remove(&e.mount) {
                    s.cancel();
                }
            }
        }
    }

    fn rescan(&mut self) {
        if let Some(e) = &mut self.explorer {
            if let Some(s) = self.scans.remove(&e.mount) {
                s.cancel();
            }
            self.scans.insert(e.mount.clone(), scan::Scan::start(Path::new(&e.mount)));
            e.path.clear();
            e.sel = 0;
        }
    }

    /// (number of entries in the current folder, is the selected entry an enterable folder)
    fn explorer_info(&self) -> Option<(usize, bool)> {
        let e = self.explorer.as_ref()?;
        self.scans.get(&e.mount)?.with(|root| {
            let n = root.at(&e.path);
            let enterable = n
                .children
                .get(e.sel)
                .is_some_and(|c| c.kind == scan::Kind::Dir && !c.children.is_empty());
            (n.children.len(), enterable)
        })
    }

    fn explorer_move(&mut self, d: isize) {
        let Some((len, _)) = self.explorer_info() else { return };
        if let Some(e) = &mut self.explorer {
            if len > 0 {
                e.sel = (e.sel as isize + d).clamp(0, len as isize - 1) as usize;
            }
        }
    }

    fn explorer_enter(&mut self) {
        if let Some((_, true)) = self.explorer_info() {
            let e = self.explorer.as_mut().unwrap();
            e.path.push(e.sel);
            e.sel = 0;
            self.table_offset = 0;
        }
    }

    fn explorer_up(&mut self) {
        if let Some(e) = &mut self.explorer {
            if let Some(i) = e.path.pop() {
                e.sel = i;
                self.table_offset = 0;
            }
        }
    }

    fn do_kill(&mut self, mon: &Monitor, force: bool) {
        let Some(k) = self.kill.take() else { return };
        let errs: Vec<String> = k.pids.iter().filter_map(|&p| mon.kill(p, force).err()).collect();
        let sig = if force { "SIGKILL" } else { "SIGTERM" };
        if errs.is_empty() {
            self.set_status(format!("{sig} sent to {}", k.label), false);
        } else if errs.len() == k.pids.len() {
            self.set_status(errs[0].clone(), true);
        } else {
            self.set_status(
                format!("{sig}: {} ok, {} failed — {}", k.pids.len() - errs.len(), errs.len(), errs[0]),
                true,
            );
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut mon = Monitor::new();
    let mut app = App::new(mon.snapshot());

    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        prev(info);
    }));

    let res = run(&mut terminal, &mut mon, &mut app);

    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    res
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    mon: &mut Monitor,
    app: &mut App,
) -> anyhow::Result<()> {
    let mut next_tick = Instant::now() + TICK;
    loop {
        if let Some((_, err, t)) = app.status {
            if t.elapsed() >= status_ttl(err) {
                app.status = None;
            }
        }
        terminal.draw(|f| ui::draw(f, app))?;

        // wake up for the next refresh, or earlier to clear the footer message
        let mut wake = next_tick;
        if let Some((_, err, t)) = app.status {
            wake = wake.min(t + status_ttl(err));
        }
        // keep the scan progress/spinner lively
        let scanning = app.explorer.as_ref().and_then(|e| app.scans.get(&e.mount)).is_some_and(|s| s.with(|_| ()).is_none());
        if scanning {
            wake = wake.min(Instant::now() + Duration::from_millis(120));
        }
        let timeout = wake.saturating_duration_since(Instant::now());
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if handle_key(app, mon, k.code, k.modifiers) {
                        return Ok(());
                    }
                    if app.status.as_ref().is_some_and(|s| s.2.elapsed() < Duration::from_millis(50)) {
                        next_tick = Instant::now() + Duration::from_millis(300); // refresh soon after a kill
                    }
                }
                Event::Mouse(m) if app.kill.is_none() && !app.show_help => {
                    handle_mouse(app, m.kind, m.column, m.row)
                }
                _ => {}
            }
        }

        if Instant::now() >= next_tick {
            app.snap = mon.snapshot();
            app.rebuild();
            next_tick = Instant::now() + TICK;
        }
    }
}

fn handle_mouse(app: &mut App, kind: MouseEventKind, col: u16, row: u16) {
    let inside = |a: ratatui::layout::Rect| col >= a.x && col < a.x + a.width && row >= a.y && row < a.y + a.height;
    let step = |app: &mut App, d: isize| {
        if app.explorer.is_some() { app.explorer_move(d) } else { app.move_by(d) }
    };
    match kind {
        MouseEventKind::ScrollDown => step(app, 3),
        MouseEventKind::ScrollUp => step(app, -3),
        MouseEventKind::Down(MouseButton::Left) => {
            let d = app.disk_area;
            if inside(d) {
                let i = (row - d.y) as usize;
                if i < app.snap.disks.len() {
                    app.close_explorer();
                    app.open_disk(i);
                }
                return;
            }
            let a = app.table_area;
            // +2: top border and header row
            if inside(a) && row >= a.y + 2 && row + 1 < a.y + a.height {
                let idx = app.table_offset + (row - a.y - 2) as usize;
                if let Some(cur) = app.explorer.as_ref().map(|e| e.sel) {
                    if cur == idx {
                        app.explorer_enter(); // click again on the selected folder opens it
                    } else {
                        app.explorer.as_mut().unwrap().sel = idx;
                        app.explorer_move(0); // clamps to the list length
                    }
                } else if idx < app.rows.len() {
                    app.selected = idx;
                    app.clamp();
                }
            }
        }
        _ => {}
    }
}

/// Returns true to quit.
fn handle_key(app: &mut App, mon: &Monitor, code: KeyCode, mods: KeyModifiers) -> bool {
    if mods.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
        return true;
    }

    if app.kill.is_some() {
        match code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('t') => app.do_kill(mon, false),
            KeyCode::Char('f') | KeyCode::Char('F') => app.do_kill(mon, true),
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => app.kill = None,
            _ => {}
        }
        return false;
    }

    if app.editing_filter {
        match code {
            KeyCode::Esc => {
                app.filter.clear();
                app.editing_filter = false;
            }
            KeyCode::Enter | KeyCode::Down | KeyCode::Up => app.editing_filter = false,
            KeyCode::Backspace => {
                app.filter.pop();
            }
            KeyCode::Char(c) => app.filter.push(c),
            _ => {}
        }
        app.selected = 0;
        app.selected_key = None;
        app.rebuild();
        return false;
    }

    if app.show_help {
        app.show_help = false;
        return false;
    }

    if app.explorer.is_some() {
        let page = app.table_area.height.saturating_sub(4) as isize;
        match code {
            KeyCode::Down | KeyCode::Char('j') => app.explorer_move(1),
            KeyCode::Up | KeyCode::Char('k') => app.explorer_move(-1),
            KeyCode::PageDown => app.explorer_move(page),
            KeyCode::PageUp => app.explorer_move(-page),
            KeyCode::Home => app.explorer_move(isize::MIN / 2),
            KeyCode::End => app.explorer_move(isize::MAX / 2),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => app.explorer_enter(),
            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => app.explorer_up(),
            KeyCode::Char('r') => app.rescan(),
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('d') => app.close_explorer(),
            KeyCode::Char('?') | KeyCode::F(1) => app.show_help = true,
            _ => {}
        }
        return false;
    }

    if app.disk_focus {
        let n = app.snap.disks.len();
        match code {
            KeyCode::Down | KeyCode::Char('j') if n > 0 => app.disk_sel = (app.disk_sel + 1).min(n - 1),
            KeyCode::Up | KeyCode::Char('k') => app.disk_sel = app.disk_sel.saturating_sub(1),
            KeyCode::Enter | KeyCode::Right => app.open_disk(app.disk_sel),
            KeyCode::Esc | KeyCode::Char('d') | KeyCode::Char('q') => app.disk_focus = false,
            _ => {}
        }
        return false;
    }

    match code {
        KeyCode::Char('q') => return true,
        KeyCode::Esc if !app.filter.is_empty() => {
            app.filter.clear();
            app.rebuild();
        }
        KeyCode::Down | KeyCode::Char('j') => app.move_by(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_by(-1),
        KeyCode::PageDown => app.move_by(app.table_area.height.saturating_sub(4) as isize),
        KeyCode::PageUp => app.move_by(-(app.table_area.height.saturating_sub(4) as isize)),
        KeyCode::Home => app.move_by(isize::MIN / 2),
        KeyCode::End => app.move_by(isize::MAX / 2),
        KeyCode::Char('x') | KeyCode::Delete | KeyCode::F(9) => app.open_kill(),
        KeyCode::Char('g') | KeyCode::Tab => {
            app.grouped = !app.grouped;
            app.selected = 0;
            app.selected_key = None;
            app.rebuild();
        }
        KeyCode::Char('/') => app.editing_filter = true,
        KeyCode::Char('d') if !app.snap.disks.is_empty() => {
            app.disk_sel = app.disk_sel.min(app.snap.disks.len() - 1);
            app.disk_focus = true;
        }
        KeyCode::Char('h') => {
            app.hide_system = !app.hide_system;
            app.rebuild();
            let msg = if app.hide_system { "System processes hidden" } else { "System processes shown" };
            app.set_status(msg, false);
        }
        KeyCode::Char('s') => {
            app.sort = app.sort.next();
            app.rebuild();
        }
        KeyCode::Char('m') => { app.sort = SortKey::Mem; app.rebuild(); }
        KeyCode::Char('c') => { app.sort = SortKey::Cpu; app.rebuild(); }
        KeyCode::Char('r') => {
            app.reverse = !app.reverse;
            app.rebuild();
        }
        KeyCode::Char('?') | KeyCode::F(1) => app.show_help = true,
        _ => {}
    }
    false
}
