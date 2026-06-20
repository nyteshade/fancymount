# FancyMount — Design Choices

_Read this over your morning coffee._ ☕

---

## Language: Rust (not Swift)

| Criterion | Rust | Swift |
|-----------|------|-------|
| **macOS Intel** | ✅ first-class | ✅ |
| **macOS ARM** | ✅ first-class | ✅ |
| **Linux** | ✅ first-class | ⚠️ works, but ecosystem thin |
| **TUI libraries** | ✅ ratatui (mature, 10k+ stars) | ⚠️ Swift curses bindings exist but are niche |
| **Single binary** | ✅ static link, no runtime | ❌ needs Swift runtime libs |
| **Package manager** | ✅ cargo ubiquitous | ⚠️ SPM okay but less universal |
| **FFI to POSIX** | ✅ libc crate, trivial | ✅ Darwin/Linux overlays |

**Verdict:** Swift on Linux is _possible_ but the TUI story is 5+ years behind Rust.  ratatui (née tui-rs) is battle-tested in tools like `bottom`, `gitui`, `spotify-tui`.  Rust also gives a single statically-linked binary — no shared-lib dance on Linux.

---

## TUI Framework: ratatui + crossterm

- **ratatui** — immediate-mode retained-state widget library.  We build `Paragraph`, `List`, `Block` widgets each frame.  The framework handles diffing and minimal terminal updates.
- **crossterm** — cross-platform terminal manipulation (raw mode, colors, events).  Chosen over `termion` because `termion` is Linux-only.

---

## Mount Discovery

| Platform | Method |
|----------|--------|
| **macOS** | `getmntinfo(3)` via `libc` crate — returns `statfs` structs directly from the kernel.  No process spawning. |
| **Linux** | Parse `/proc/mounts` — a virtual file the kernel maintains.  Octal-unescape for spaces-in-paths (`\040`).  Fallback to `/proc/self/mounts`. |

Both backends compile into the same binary via `#[cfg(target_os = "...")]` — only one path exists in the final executable.

**Why not `df` or `mount` command?** Parsing human-readable output is fragile (crowdstrike Falcon injects lines, locales change headers, column widths vary).  Reading kernel data structures is deterministic.

---

## Disk Usage: `statvfs(2)` (POSIX)

Every mountpoint gets a `statvfs` call.  Returns `f_blocks`, `f_bavail` (available to unprivileged users), and `f_bsize`.  We multiply to get bytes and compute usage percentage.  Pseudo-filesystems (`devfs`, `proc`) return errors — those show a greyed-out placeholder.

### Usage bars

All bars are a **fixed width** (10 chars in the list, 30 in the detail pane) so you can compare usage across mount points at a glance.

- **Track**: `▒` (medium shade 50%, U+2592) fills the entire bar width as a visible background.  No invisible gaps.
- **Fill**: `▇` (lower 7/8 block, U+2587) painted up to the appropriate percentage, coloured with a usage gradient: emerald green (<50%) → amber (<75%) → deep orange (<90%) → crimson (>90%).
- **Simple rounding**: filled slots = `round(fraction × width)`.  No partial/eighth-block characters at the boundary — every bar is composed of whole `▇` and `▒` only, giving a clean consistent look.
- **True-colour aware**: all RGB colours degrade automatically to 256- and 16-colour terminals via ratatui's colour quantisation.

### Mount point display

The **list pane** (63% width) shows only the last path component.  The **detail pane** (37%) shows the full path.  List columns use fixed widths derived from the data: the device column is padded to the width of the longest device basename so every mount point starts at the same horizontal column — no ragged edges when scrolling.

---

## Mount / Unmount: `sudo -S` + in-TUI password prompt

Instead of suspending the TUI to show the raw terminal for `[sudo] password:`, we show a modal password dialog **inside** the TUI.  The password is typed masked (`••••`), then piped to `sudo -S` via stdin.  The password is never logged or stored — it lives only in the `PasswordPrompt` mode's String field and is moved into the pipe write before being dropped.

**macOS `diskutil` fallback:** when `umount` fails on macOS (common for CoreSimulator volumes, APFS system volumes, etc.), we automatically retry with `diskutil unmount [force]`.  This is transparent to the user — no "Resource busy" errors to deal with manually.

---

## Key Bindings

| Key | Action |
|-----|--------|
| `↑` / `↓` or `j` / `k` | Navigate mount list |
| `PgUp` / `PgDn` | Jump by 10 |
| `Home` / `End` | First / last |
| `Tab` | Switch focus between list and detail pane |
| `m` | Mount — opens dialog prefilled with selected device |
| `u` | Unmount selected |
| `f` | Force-unmount (`-l` on Linux, `-f` on macOS) |
| `n` | New mount — blank dialog |
| `r` | Refresh mount list |
| `?` | Help overlay |
| `q` / `Esc` | Quit |

**vim-style `j`/`k`** — because the target audience uses terminals heavily.

**Two-pane layout** — list on the left (55%), detail on the right (45%).  Arrows navigate whichever pane has focus (Tab toggles).

---

## Mount Dialog

Shows four fields:
1. **Device** — free text (e.g. `/dev/disk4s2`, `//server/share`)
2. **Mount point** — free text; directory is auto-created if missing
3. **Filesystem** — dropdown cycled with Left/Right arrows (preset list of 25 common types)
4. **Options** — free text, comma-separated (e.g. `rw,noatime,nosuid`)

If the mountpoint doesn't exist, `create_dir_all` (equivalent to `mkdir -p`) creates it before mounting.

---

## Directory Creation on Mount

Before running `sudo mount`, the code checks `Path::new(mountpoint).exists()`.  If the path doesn't exist, `fs::create_dir_all(mountpoint)` creates it and all parent directories.  This is equivalent to `mkdir -p`.  If creation fails (permissions, etc.), the mount is aborted with an error message.

---

## Platform Differences Handled

| Feature | macOS | Linux |
|---------|-------|-------|
| Mount discovery | `getmntinfo(3)` | `/proc/mounts` |
| Force unmount | `umount -f` | `umount -l` (lazy) |
| Colour scheme | Same palette both platforms | Same palette |
| Binary name | `fm` | `fm` |

---

## Build & Install

```bash
# Debug build
cargo build

# Optimized release build
cargo build --release

# The binary is at target/release/fm
# Copy it somewhere on your PATH:
sudo cp target/release/fm /usr/local/bin/

# Run (needs sudo for mount/unmount, but browsing works unprivileged):
fm
```

Cross-compilation (from macOS to Linux, or vice versa) would need a cross-compiler toolchain — not set up here, but Rust supports it via `--target x86_64-unknown-linux-gnu`.

---

## What's NOT Implemented (Room to Grow)

- **fstab editing** — permanent mounts
- **Backgrounding** — the TUI runs in the foreground
- **Mouse support** — ratatui supports it, but keystrokes are faster
- **Themes** — hardcoded dark theme; could be configurable
- **Sorting** — entries appear in kernel order
- **Filter/search** — type to filter the mount list
- **Disk utility operations** — `fsck`, format, etc. (out of scope for a mount tool)

---

## File Structure

```
fancymount/
├── Cargo.toml          # Dependencies + binary config
├── CHOICES.md          # This file
├── README.md           # User-facing docs
└── src/
    ├── main.rs         # Entry point, terminal init, event loop
    ├── app.rs          # App state, UI rendering, input dispatch
    ├── mount_info.rs   # MountEntry struct + OS-specific parsing
    └── mount_ops.rs    # Mount/unmount/create operations
```
