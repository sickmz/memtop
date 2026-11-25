use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Cell, Clear, Padding, Paragraph, Row as TRow, Table,
        TableState, Wrap,
    },
};

use crate::{App, SortKey, scan::Kind};

// Soft, low-contrast-chrome palette: data pops, borders recede.
const FG: Color = Color::Rgb(214, 219, 230);
const DIM: Color = Color::Rgb(118, 126, 145);
const BORDER: Color = Color::Rgb(62, 68, 86);
const TRACK: Color = Color::Rgb(42, 46, 58);
const BLUE: Color = Color::Rgb(122, 162, 247);
const AVAIL: Color = Color::Rgb(70, 76, 94);
const GREEN: Color = Color::Rgb(152, 200, 120);
const AMBER: Color = Color::Rgb(230, 180, 100);
const RED: Color = Color::Rgb(244, 120, 140);
const SEL_BG: Color = Color::Rgb(44, 52, 76);

pub fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", U[i])
    } else {
        format!("{v:.1} {}", U[i])
    }
}

fn level_color(frac: f64) -> Color {
    if frac >= 0.9 {
        RED
    } else if frac >= 0.75 {
        AMBER
    } else {
        GREEN
    }
}

/// Single-colour bar with 1/8-cell precision.
fn bar(frac: f64, width: u16, color: Color) -> Vec<Span<'static>> {
    const PARTS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let w = width as usize;
    let eighths = ((frac.clamp(0.0, 1.0) * w as f64 * 8.0).round()) as usize;
    let full = eighths / 8;
    let rem = eighths % 8;
    let mut s = "█".repeat(full);
    if full < w {
        s.push_str(PARTS[rem]);
    }
    let used = full + usize::from(rem > 0 && full < w);
    vec![
        Span::styled(s, Style::new().fg(color).bg(TRACK)),
        Span::styled(" ".repeat(w.saturating_sub(used)), Style::new().bg(TRACK)),
    ]
}

/// Multi-segment bar (used | cache | free).
fn segmented(parts: &[(u64, Color)], total: u64, width: u16) -> Line<'static> {
    let w = width as usize;
    let mut spans = Vec::new();
    let mut acc = 0usize;
    let mut cum = 0u64;
    for (v, c) in parts {
        cum += v;
        let end = ((cum as f64 / total.max(1) as f64) * w as f64).round() as usize;
        let n = end.saturating_sub(acc).min(w - acc);
        spans.push(Span::styled("█".repeat(n), Style::new().fg(*c)));
        acc += n;
    }
    spans.push(Span::styled(" ".repeat(w - acc), Style::new().bg(TRACK)));
    Line::from(spans)
}

fn panel(title: impl Into<Line<'static>>) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER))
        .title(title.into())
        .padding(Padding::horizontal(1))
}

fn title(t: &str) -> Span<'static> {
    Span::styled(format!(" {t} "), Style::new().fg(FG).bold())
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let narrow = area.width < 100;
    let disk_rows = app.snap.disks.len().clamp(1, if narrow { 3 } else { 6 }) as u16;
    // Narrow terminals: stack memory above disks instead of side by side.
    let top_h = if narrow { 6 + disk_rows + 2 } else { (disk_rows + 2).max(6) };

    let [header, top, main, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(top_h),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(f, app, header);

    let [mem_a, disk_a] = if narrow {
        Layout::vertical([Constraint::Length(6), Constraint::Min(0)]).areas(top)
    } else {
        Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)]).areas(top)
    };
    draw_memory(f, app, mem_a);
    draw_disks(f, app, disk_a);
    if app.explorer.is_some() {
        draw_explorer(f, app, main);
    } else {
        draw_table(f, app, main);
    }
    draw_footer(f, app, footer);

    if app.kill.is_some() {
        draw_kill(f, app, area);
    } else if app.show_help {
        draw_help(f, area);
    }
}

fn draw_header(f: &mut Frame, app: &App, a: Rect) {
    let s = &app.snap;
    let up = s.uptime;
    let up = if up >= 86400 {
        format!("{}d {}h", up / 86400, (up % 86400) / 3600)
    } else {
        format!("{}h {}m", up / 3600, (up % 3600) / 60)
    };
    let host = sysinfo::System::host_name().unwrap_or_default();
    let cpu_frac = s.cpu as f64 / 100.0;

    let mut spans = vec![
        Span::styled(" memtop ", Style::new().fg(Color::Rgb(20, 22, 30)).bg(BLUE).bold()),
        Span::styled(format!("  {host}"), Style::new().fg(FG)),
    ];
    if a.width >= 100 {
        spans.push(Span::styled(
            format!("  ·  up {up}  ·  load {:.2} {:.2} {:.2}", s.load[0], s.load[1], s.load[2]),
            Style::new().fg(DIM),
        ));
    }
    spans.push(Span::styled("  ·  CPU ", Style::new().fg(DIM)));
    spans.extend(bar(cpu_frac, 10, level_color(cpu_frac)));
    spans.push(Span::styled(format!(" {:>3.0}% ", s.cpu), Style::new().fg(FG)));
    spans.push(Span::styled(format!("{} cores", s.ncpu), Style::new().fg(DIM)));
    f.render_widget(Paragraph::new(Line::from(spans)), a);
}

fn draw_memory(f: &mut Frame, app: &App, a: Rect) {
    let m = app.snap.mem;
    let frac = m.used as f64 / m.total.max(1) as f64;
    let block = panel(Line::from(vec![title("Memory")]));
    let inner = block.inner(a);
    f.render_widget(block, a);

    let [l1, l2, l3, l4] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(human(m.used), Style::new().fg(level_color(frac)).bold()),
            Span::styled(format!(" / {}", human(m.total)), Style::new().fg(FG)),
            Span::styled(format!("  ({:.0}%)", frac * 100.0), Style::new().fg(DIM)),
        ])),
        l1,
    );

    let used_c = if frac >= 0.75 { level_color(frac) } else { BLUE };
    f.render_widget(
        Paragraph::new(segmented(&[(m.used, used_c), (m.available, AVAIL)], m.total, l2.width)),
        l2,
    );

    let dot = |c: Color| Span::styled("● ", Style::new().fg(c));
    f.render_widget(
        Paragraph::new(Line::from(vec![
            dot(AVAIL),
            Span::styled(format!("Available {}", human(m.available)), Style::new().fg(FG)),
        ])),
        l3,
    );

    let mut swap = vec![Span::styled("Swap  ", Style::new().fg(DIM))];
    if m.swap_total == 0 {
        swap.push(Span::styled("not configured", Style::new().fg(DIM).italic()));
    } else {
        let sf = m.swap_used as f64 / m.swap_total as f64;
        swap.extend(bar(sf, 16, level_color(sf)));
        swap.push(Span::styled(
            format!("  {} / {}", human(m.swap_used), human(m.swap_total)),
            Style::new().fg(FG),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(swap)), l4);

}

fn draw_disks(f: &mut Frame, app: &mut App, a: Rect) {
    let active = app.disk_focus || app.explorer.is_some();
    let block = panel(Line::from(vec![
        title("Disks"),
        Span::styled(if active { "" } else { "click or d to explore " }, Style::new().fg(DIM)),
    ]))
    .border_style(Style::new().fg(if app.disk_focus { BLUE } else { BORDER }));
    let inner = block.inner(a);
    f.render_widget(block, a);
    app.disk_area = inner;

    let w = inner.width as usize;
    let mount_w = 12.min(w / 3);
    let size_w = 15;
    let pct_w = 5;
    let bar_w = w.saturating_sub(mount_w + size_w + pct_w + 3).max(4) as u16;

    let max = inner.height as usize;
    let disks = &app.snap.disks;
    let mut lines: Vec<Line> = Vec::new();
    for (i, d) in disks.iter().enumerate() {
        if i + 1 == max && disks.len() > max {
            lines.push(Line::styled(format!("+{} more", disks.len() - i), Style::new().fg(DIM)));
            break;
        }
        let frac = d.used() as f64 / d.total.max(1) as f64;
        let mut mount = d.mount.clone();
        if mount.chars().count() > mount_w {
            let tail: String = mount.chars().rev().take(mount_w - 1).collect::<Vec<_>>().into_iter().rev().collect();
            mount = format!("…{tail}");
        }
        let hl = active && i == app.disk_sel;
        let mut spans = vec![Span::styled(
            format!("{mount:<mount_w$} "),
            if hl { Style::new().fg(BLUE).bg(SEL_BG).bold() } else { Style::new().fg(FG) },
        )];
        spans.extend(bar(frac, bar_w, level_color(frac)));
        spans.push(Span::styled(format!(" {:>3.0}%", frac * 100.0), Style::new().fg(level_color(frac))));
        spans.push(Span::styled(
            format!(" {:>size_w$}", format!("{}/{}", human(d.used()), human(d.total))),
            Style::new().fg(DIM),
        ));
        lines.push(Line::from(spans));
    }
    if disks.is_empty() {
        lines.push(Line::styled("no disks found", Style::new().fg(DIM)));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_table(f: &mut Frame, app: &mut App, a: Rect) {
    let total = app.snap.mem.total.max(1);
    let arrow = if app.reverse { "↑" } else { "↓" };
    let mut t = vec![
        title(if app.grouped { "Apps" } else { "Processes" }),
        Span::styled(format!("{} ", app.rows.len()), Style::new().fg(DIM)),
        Span::styled(format!("· sorted by {} {arrow} ", app.sort.label()), Style::new().fg(DIM)),
    ];
    if app.hide_system {
        t.push(Span::styled("· system hidden ", Style::new().fg(AMBER)));
    }
    if app.editing_filter || !app.filter.is_empty() {
        t.push(Span::styled("· search: ", Style::new().fg(DIM)));
        t.push(Span::styled(app.filter.clone(), Style::new().fg(AMBER).bold()));
        if app.editing_filter {
            t.push(Span::styled("▏", Style::new().fg(AMBER)));
        }
        t.push(Span::raw(" "));
    }
    let block = panel(Line::from(t)).padding(Padding::ZERO);

    let hdr = |s: &str, k: Option<SortKey>| {
        let active = k == Some(app.sort);
        let st = if active { Style::new().fg(BLUE).bold() } else { Style::new().fg(DIM) };
        Cell::from(Span::styled(if active { format!("{s} {arrow}") } else { s.to_string() }, st))
    };

    let bar_w: u16 = if a.width < 100 { 6 } else { 14 };
    let (header, widths): (Vec<Cell>, Vec<Constraint>) = if app.grouped {
        (
            vec![
                hdr(" App", Some(SortKey::Name)),
                hdr("Proc", None),
                hdr("Users", None),
                hdr("RAM", Some(SortKey::Mem)),
                hdr("% RAM", None),
                hdr("CPU", Some(SortKey::Cpu)),
                hdr("PID", Some(SortKey::Pid)),
            ],
            vec![
                Constraint::Length(if a.width < 100 { 16 } else { 22 }),
                Constraint::Length(5),
                Constraint::Length(if a.width < 100 { 8 } else { 12 }),
                Constraint::Length(9),
                Constraint::Length(bar_w + 7),
                Constraint::Length(7),
                Constraint::Min(10),
            ],
        )
    } else {
        (
            vec![
                hdr(" PID", Some(SortKey::Pid)),
                hdr("Name", Some(SortKey::Name)),
                hdr("User", None),
                hdr("RAM", Some(SortKey::Mem)),
                hdr("% RAM", None),
                hdr("CPU", Some(SortKey::Cpu)),
                hdr("Command", None),
            ],
            vec![
                Constraint::Length(9),
                Constraint::Length(18),
                Constraint::Length(10),
                Constraint::Length(9),
                Constraint::Length(bar_w + 7),
                Constraint::Length(7),
                Constraint::Min(10),
            ],
        )
    };

    let rows = app.rows.iter().map(|r| {
        let frac = r.rss as f64 / total as f64;
        let mem_c = if frac >= 0.25 { RED } else if frac >= 0.10 { AMBER } else if frac >= 0.01 { FG } else { DIM };
        let bar_c = if frac >= 0.25 { RED } else if frac >= 0.10 { AMBER } else { BLUE };
        let mut pct = bar((frac * 4.0).min(1.0), bar_w, bar_c); // ×4: 25% of RAM fills the bar
        pct.push(Span::styled(format!(" {:>4.1}%", frac * 100.0), Style::new().fg(mem_c)));
        let cpu_c = if r.cpu >= 80.0 { RED } else if r.cpu >= 20.0 { AMBER } else if r.cpu >= 1.0 { FG } else { DIM };

        let cells = if app.grouped {
            vec![
                Cell::from(format!(" {}", r.name)).style(Style::new().fg(FG).bold()),
                Cell::from(format!("{:>4}", r.count)).style(Style::new().fg(DIM)),
                Cell::from(r.user.clone()).style(Style::new().fg(DIM)),
                Cell::from(format!("{:>8}", human(r.rss))).style(Style::new().fg(mem_c).bold()),
                Cell::from(Line::from(pct)),
                Cell::from(format!("{:>5.1}%", r.cpu)).style(Style::new().fg(cpu_c)),
                Cell::from(r.detail.clone()).style(Style::new().fg(DIM)),
            ]
        } else {
            vec![
                Cell::from(format!(" {:>7}", r.pids[0])).style(Style::new().fg(DIM)),
                Cell::from(r.name.clone()).style(Style::new().fg(FG).bold()),
                Cell::from(r.user.clone()).style(Style::new().fg(if r.user == "root" { AMBER } else { DIM })),
                Cell::from(format!("{:>8}", human(r.rss))).style(Style::new().fg(mem_c).bold()),
                Cell::from(Line::from(pct)),
                Cell::from(format!("{:>5.1}%", r.cpu)).style(Style::new().fg(cpu_c)),
                Cell::from(r.detail.clone()).style(Style::new().fg(DIM)),
            ]
        };
        TRow::new(cells)
    });

    let table = Table::new(rows, widths)
        .header(TRow::new(header).style(Style::new().add_modifier(Modifier::UNDERLINED)))
        .block(block)
        .column_spacing(1)
        .row_highlight_style(Style::new().bg(SEL_BG))
        .highlight_symbol(Span::styled("▌", Style::new().fg(BLUE)));

    let mut st = TableState::default().with_offset(app.table_offset).with_selected(Some(app.selected));
    f.render_stateful_widget(table, a, &mut st);
    app.table_offset = st.offset();
    app.table_area = a;

    if app.rows.is_empty() {
        let msg = Paragraph::new(Line::styled("No results", Style::new().fg(DIM)))
            .alignment(Alignment::Center);
        f.render_widget(msg, Rect { y: a.y + a.height / 2, height: 1, ..a });
    }
}

fn draw_footer(f: &mut Frame, app: &App, a: Rect) {
    let key = |k: &str, d: &str| {
        vec![
            Span::styled(format!(" {k} "), Style::new().fg(Color::Rgb(20, 22, 30)).bg(DIM)),
            Span::styled(format!(" {d}  "), Style::new().fg(DIM)),
        ]
    };
    let narrow = a.width < 100;
    let mut spans: Vec<Span> = if app.editing_filter {
        [key("Enter", "confirm"), key("Esc", "cancel")].concat()
    } else if app.explorer.is_some() {
        [
            key("↑↓", "move"),
            key("Enter →", "open"),
            key("← ⌫", "back"),
            key("r", "rescan"),
            key("Esc", "back to processes"),
        ]
        .concat()
    } else if app.disk_focus {
        [key("↑↓", "pick disk"), key("Enter", "explore"), key("Esc", "cancel")].concat()
    } else {
        [
            key("↑↓", "move"),
            key("x", "kill"),
            key("g", if app.grouped { "by process" } else { "by app" }),
            key("/", "search"),
            key("h", if app.hide_system { "show system" } else { "hide system" }),
            key("d", "disk usage"),
            if narrow { vec![] } else { key("s", "sort") },
            key("?", "help"),
            key("q", "quit"),
        ]
        .concat()
    };
    if let Some((msg, err, _)) = &app.status {
        spans = vec![Span::styled(
            format!(" {} {msg} ", if *err { "✗" } else { "✓" }),
            Style::new().fg(Color::Rgb(20, 22, 30)).bg(if *err { RED } else { GREEN }).bold(),
        )];
    }
    f.render_widget(Paragraph::new(Line::from(spans)), a);
}

fn centered(a: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(a.width);
    let h = h.min(a.height);
    Rect { x: a.x + (a.width - w) / 2, y: a.y + (a.height - h) / 2, width: w, height: h }
}

fn draw_kill(f: &mut Frame, app: &App, area: Rect) {
    let Some(k) = &app.kill else { return };
    let r = centered(area, 58, 8);
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(RED))
        .title(Span::styled(" Kill process ", Style::new().fg(RED).bold()))
        .padding(Padding::uniform(1));
    let text = vec![
        Line::from(vec![
            Span::styled("Kill ", Style::new().fg(FG)),
            Span::styled(k.label.clone(), Style::new().fg(FG).bold()),
            Span::styled("?", Style::new().fg(FG)),
        ]),
        Line::styled(format!("Would free about {} of RAM", human(k.rss)), Style::new().fg(DIM)),
        Line::raw(""),
        Line::from(vec![
            Span::styled(" Enter ", Style::new().fg(Color::Rgb(20, 22, 30)).bg(AMBER).bold()),
            Span::styled(" terminate   ", Style::new().fg(FG)),
            Span::styled(" F ", Style::new().fg(Color::Rgb(20, 22, 30)).bg(RED).bold()),
            Span::styled(" force   ", Style::new().fg(FG)),
            Span::styled(" Esc ", Style::new().fg(Color::Rgb(20, 22, 30)).bg(DIM).bold()),
            Span::styled(" cancel", Style::new().fg(FG)),
        ]),
    ];
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }).block(block), r);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let r = centered(area, 60, 18);
    f.render_widget(Clear, r);
    let rows = [
        ("↑ ↓  j k", "move selection (or mouse wheel)"),
        ("PgUp PgDn", "page up / down"),
        ("x  Del  F9", "kill selected process / app"),
        ("g  Tab", "group by app / by process"),
        ("/", "search name, command, user, PID"),
        ("h", "hide / show system processes"),
        ("d  click", "explore what fills a disk"),
        ("s", "change sort order"),
        ("m  c", "sort by RAM / by CPU"),
        ("r", "reverse sort"),
        ("Esc", "clear search"),
        ("q", "quit"),
    ];
    let mut lines: Vec<Line> = rows
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(format!("{k:<13}"), Style::new().fg(BLUE).bold()),
                Span::styled(*d, Style::new().fg(FG)),
            ])
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "RAM = resident memory (RSS). Shared memory may be",
        Style::new().fg(DIM),
    ));
    lines.push(Line::styled("counted in more than one process.", Style::new().fg(DIM)));
    let block = panel(Line::from(title("Keys"))).border_style(Style::new().fg(BLUE));
    f.render_widget(Paragraph::new(lines).block(block), r);
}

fn draw_explorer(f: &mut Frame, app: &mut App, a: Rect) {
    let Some(e) = &app.explorer else { return };
    let Some(scan) = app.scans.get(&e.mount) else { return };
    let disk_used = app.snap.disks.iter().find(|d| d.mount == e.mount).map(|d| d.used());
    let denied = scan.progress.denied.load(std::sync::atomic::Ordering::Relaxed);

    // Still scanning: show live progress.
    let done = scan.with(|_| ()).is_some();
    if !done {
        let files = scan.progress.files.load(std::sync::atomic::Ordering::Relaxed);
        let bytes = scan.progress.bytes.load(std::sync::atomic::Ordering::Relaxed);
        let t = scan.elapsed();
        const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let spin = SPIN[(t.as_millis() / 100) as usize % SPIN.len()];
        let block = panel(Line::from(vec![title("Disk usage"), Span::styled(format!("{} ", e.mount), Style::new().fg(DIM))]));
        let inner = block.inner(a);
        f.render_widget(block, a);
        let mut lines = vec![
            Line::from(vec![
                Span::styled(format!("{spin} Scanning "), Style::new().fg(BLUE).bold()),
                Span::styled(e.mount.clone(), Style::new().fg(FG).bold()),
            ]),
            Line::styled(
                format!("{} files · {} · {}s", group_digits(files), human(bytes), t.as_secs()),
                Style::new().fg(FG),
            ),
        ];
        if let Some(u) = disk_used {
            let frac = bytes as f64 / u.max(1) as f64;
            let mut spans = bar(frac.min(1.0), 30.min(inner.width.saturating_sub(2)), BLUE);
            spans.push(Span::styled(format!(" ~{:.0}%", (frac * 100.0).min(99.0)), Style::new().fg(DIM)));
            lines.push(Line::from(spans));
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled("Esc to cancel", Style::new().fg(DIM)));
        let h = lines.len() as u16;
        let r = Rect { y: inner.y + inner.height.saturating_sub(h) / 2, height: h.min(inner.height), ..inner };
        f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), r);
        return;
    }

    let (path, sel) = (e.path.clone(), e.sel);
    let mount = e.mount.clone();
    let narrow = a.width < 100;
    let bar_w: u16 = if narrow { 8 } else { 20 };

    let (title_line, rows, n, bottom) = scan
        .with(|root| {
            let node = root.at(&path);
            // breadcrumb: /  ›  home  ›  ubuntu
            let mut crumbs = vec![title("Disk usage"), Span::styled(mount.clone(), Style::new().fg(BLUE).bold())];
            let mut cur = root;
            for &i in &path {
                cur = &cur.children[i];
                crumbs.push(Span::styled(" › ", Style::new().fg(DIM)));
                crumbs.push(Span::styled(cur.name.clone(), Style::new().fg(FG).bold()));
            }
            crumbs.push(Span::styled(
                format!("  {} · {} files ", human(node.size), group_digits(node.files)),
                Style::new().fg(DIM),
            ));

            let total = node.size.max(1);
            let rows: Vec<TRow> = node
                .children
                .iter()
                .map(|c| {
                    let frac = c.size as f64 / total as f64;
                    let c_col = if frac >= 0.5 { RED } else if frac >= 0.2 { AMBER } else { BLUE };
                    let (name, name_style) = match c.kind {
                        Kind::Dir => (format!(" ▸ {}/", c.name), Style::new().fg(FG).bold()),
                        Kind::File => (format!("   {}", c.name), Style::new().fg(FG)),
                        Kind::Small(k) => (format!("   ({} smaller files)", group_digits(k)), Style::new().fg(DIM).italic()),
                        Kind::Denied => (format!(" ✗ {}/  no access", c.name), Style::new().fg(DIM)),
                    };
                    let mut pct = bar(frac, bar_w, c_col);
                    pct.push(Span::styled(format!(" {:>5.1}%", frac * 100.0), Style::new().fg(DIM)));
                    let files = match c.kind {
                        Kind::Dir => group_digits(c.files),
                        _ => String::new(),
                    };
                    TRow::new(vec![
                        Cell::from(Span::styled(name, name_style)),
                        Cell::from(format!("{:>9}", human(c.size))).style(Style::new().fg(if frac >= 0.2 { c_col } else { FG }).bold()),
                        Cell::from(Line::from(pct)),
                        Cell::from(format!("{files:>10}")).style(Style::new().fg(DIM)),
                    ])
                })
                .collect();

            let mut bottom = vec![Span::styled(format!(" scanned in {:.1}s", scan.elapsed().as_secs_f64()), Style::new().fg(DIM))];
            if path.is_empty() {
                if let Some(u) = disk_used {
                    if u > node.size + (1 << 30) {
                        bottom.push(Span::styled(
                            format!(" · {} of {} used found", human(node.size), human(u)),
                            Style::new().fg(DIM),
                        ));
                    }
                }
            }
            if denied > 0 {
                bottom.push(Span::styled(
                    format!(" · {} folders unreadable: run with sudo to see everything ", group_digits(denied)),
                    Style::new().fg(AMBER),
                ));
            }
            (Line::from(crumbs), rows, node.children.len(), Line::from(bottom))
        })
        .unwrap();

    let block = panel(title_line).title_bottom(bottom).padding(Padding::ZERO);
    let header = TRow::new(vec![
        Cell::from(Span::styled(" Name", Style::new().fg(DIM))),
        Cell::from(Span::styled("     Size ↓", Style::new().fg(BLUE).bold())),
        Cell::from(Span::styled("% of folder", Style::new().fg(DIM))),
        Cell::from(Span::styled("     Files", Style::new().fg(DIM))),
    ])
    .style(Style::new().add_modifier(Modifier::UNDERLINED));
    let widths = [
        Constraint::Min(20),
        Constraint::Length(11),
        Constraint::Length(bar_w + 8),
        Constraint::Length(10),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(1)
        .row_highlight_style(Style::new().bg(SEL_BG))
        .highlight_symbol(Span::styled("▌", Style::new().fg(BLUE)));
    let mut st = TableState::default()
        .with_offset(app.table_offset)
        .with_selected(if n == 0 { None } else { Some(sel.min(n - 1)) });
    f.render_stateful_widget(table, a, &mut st);
    app.table_offset = st.offset();
    app.table_area = a;
    if n == 0 {
        let msg = Paragraph::new(Line::styled("Empty folder", Style::new().fg(DIM))).alignment(Alignment::Center);
        f.render_widget(msg, Rect { y: a.y + a.height / 2, height: 1, ..a });
    }
}

fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}
