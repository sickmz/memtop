# memtop

A clean, native terminal system monitor focused on **who is using your RAM and disk** —
like htop, but easier to read.

- **Memory**: used vs available at a glance, swap.
- **Processes** sorted by RAM, with per-process bars; group by app (`g`) to sum all
  processes of the same program; hide system processes (`h`).
- **Kill** with confirmation (`x`): Enter = SIGTERM, F = SIGKILL; works on whole app groups.
- **Disk usage explorer** (`d` or click a disk): ncdu-style, folders sorted by size.

Single static-ish binary, no runtime. Built with Rust, [ratatui](https://ratatui.rs) and
[sysinfo](https://crates.io/crates/sysinfo); runs on Linux and macOS (Windows untested).

## Build & run

```bash
# install Rust: https://rustup.rs
cargo build --release
./target/release/memtop        # sudo ./target/release/memtop to see every folder's size
```

## Keys

| Key | Action |
|---|---|
| `↑ ↓` `j k`, wheel | move |
| `x` `Del` `F9` | kill selected process / app |
| `g` `Tab` | group by app / by process |
| `/` | search name, command, user, PID |
| `h` | hide / show system processes |
| `d`, click a disk | explore disk usage (`Enter` open, `←` back, `r` rescan, `Esc` exit) |
| `s` `m` `c` `r` | sort: cycle / RAM / CPU / reverse |
| `?` | help |
| `q` | quit |

## Layout

- `src/sys.rs` — OS data layer (memory, processes, disks, kill); UI-independent.
- `src/scan.rs` — parallel disk usage scanner.
- `src/ui.rs` — rendering.
- `src/main.rs` — app state, input handling, event loop.
