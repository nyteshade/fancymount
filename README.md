# ⛰ FancyMount

> A terminal UI mount manager — browse, inspect, mount, and unmount filesystems
> with vim-style keyboard navigation. Works on macOS (Intel + Apple Silicon) and
> Linux.

```
┌─ ⛰ FancyMount v0.1.0  42 mounts  macos 🔓 ──────────────────────────────┐
│  Mount Points (42)                 │  /Volumes/Data                      │
│                                    │                                     │
│  apfs    ▇▇▇▇▇▇▇▒▒▒  73%  disk3s1 │  Identity                          │
│  apfs    ▇▇▒▒▒▒▒▒▒▒  18%  disk3s1 │    #:             12 of 42          │
│  devfs   ▒▒▒▒▒▒▒▒▒▒    -  devfs   │    Device:        /dev/disk5s1      │
│  nfs     ▇▇▇▇▇▇▒▒▒▒  62%  server │    Mount Point:   /Volumes/Data     │
│  ...                               │    Filesystem:    apfs              │
│                                    │                                     │
│                                    │  Options                           │
│                                    │    • rw                             │
│                                    │    • noatime                        │
│                                    │                                     │
│                                    │  Usage                             │
│                                    │    Total:      926.4 GB             │
│                                    │    Used:       789.1 GB             │
│                                    │    Available:  137.3 GB             │
│                                    │    ▇▇▇▇▇▇▇▇▒▒  73.2%              │
├────────────────────────────────────┴─────────────────────────────────────┤
│  q:Quit  ↑↓/j,k:Nav  Tab:Switch Pane  m:Mount  u:Unmount  f:Force  ...  │
└──────────────────────────────────────────────────────────────────────────┘
```

## Features

- **Two-pane TUI** — mount list (63%) on the left, detailed info (37%) on the right
- **Uniform usage bars** — every bar is exactly 10 characters wide with a visible `▒`
  track and coloured `▇` fill so you can compare usage at a glance
- **Colour-coded filesystem types** — apfs is cyan, ext4 is green, nfs is orange, etc.
  True-colour RGB that automatically degrades to 256- and 16-colour terminals
- **vim-style navigation** — `j`/`k` in addition to arrow keys
- **In-TUI sudo password prompt** — no terminal suspend.  Password is cached after
  the first successful operation so you only type it once per session
- **Column-aligned layout** — index, FS type, bar, device, and mount point all line
  up vertically.  Device column is padded to the longest name
- **Basename display** — list shows just the volume/folder name; full path lives
  in the detail pane
- **Mount dialog** — type device path, mountpoint, pick FS type from a 25-entry
  dropdown, set comma-separated options
- **Auto-creates mountpoints** — runs `mkdir -p` if the target directory doesn't exist
- **macOS `diskutil` fallback** — when `umount` fails with "Resource busy", automatically
  retries with `diskutil unmount [force]`
- **Clipboard copy** — `y` copies the mount point path, `Y` copies the device path
- **Toast notifications** — clipboard confirmations pop up as a green overlay that
  auto-dismisses after 2 seconds
- **Transparent `mount` wrapper** — when called *with* arguments, `fm` passes them
  through to the real `/sbin/mount`.  You can `alias mount=fm` without breaking scripts
- **Cross-platform** — one codebase compiles for macOS ARM, macOS Intel, and Linux

## Key Bindings

### Navigation
| Key | Action |
|-----|--------|
| `↑` `↓` / `j` `k` | Move selection up/down |
| `PgUp` / `PgDn` | Jump 10 entries |
| `Home` / `End` | First / last entry |
| `Tab` | Switch focus between list and detail pane |

### Actions
| Key | Action |
|-----|--------|
| `m` | Mount — opens dialog prefilled with selected device |
| `u` | Unmount selected mount point |
| `f` | Force-unmount selected (`-l` on Linux, `diskutil unmount force` on macOS) |
| `n` | New mount — blank dialog |
| `r` | Refresh mount list |
| `y` | Copy **mount point** path to clipboard (toast notification) |
| `Y` | Copy **device** path to clipboard |

### System
| Key | Action |
|-----|--------|
| `q` / `Esc` | Quit |
| `Ctrl+C` | Force quit |
| `Ctrl+L` | Clear cached sudo password (re-lock) |
| `?` | Help screen |

## Install

### macOS (Homebrew — coming soon)

```bash
# Build from source for now (see below)
```

### Build from source

```bash
# Prerequisites: Rust 1.74+ (https://rustup.rs)
git clone https://github.com/your-username/fancymount.git
cd fancymount
cargo build --release

# Binary is at target/release/fm
sudo cp target/release/fm /usr/local/bin/
```

### Build universal macOS binary (ARM + Intel)

```bash
cargo build --release                         # native arch
cargo build --release --target x86_64-apple-darwin  # Intel
lipo -create -output fm-universal \
  target/release/fm \
  target/x86_64-apple-darwin/release/fm
```

### Cross-compile for Linux

```bash
rustup target add x86_64-unknown-linux-gnu
cargo build --release --target x86_64-unknown-linux-gnu
```

## Usage

```bash
# Launch the TUI
fm

# Pass arguments through to the real mount command (safe to alias)
fm -t apfs /dev/disk4s2 /Volumes/MyDrive
fm --help    # shows real mount's help

# Alias mount to fm for daily use
alias mount='fm'
```

No arguments → TUI.  Any arguments → transparent passthrough to `/sbin/mount`.

Browsing works without privileges.  Mount/unmount prompts for your sudo password
once, then caches it for the session (`Ctrl+L` to clear).

## Platform Support

| Feature | macOS | Linux |
|---------|-------|-------|
| Mount discovery | `getmntinfo(3)` | `/proc/mounts` |
| Unmount | `umount` → `diskutil unmount` fallback | `umount` |
| Force unmount | `diskutil unmount force` | `umount -l` (lazy) |
| Clipboard | `pbcopy` | `xclip` / `xsel` / `wl-copy` |
| Sudo passthrough | `/sbin/mount` | `/bin/mount` |

## How It Works

- **Mount discovery**: `getmntinfo(3)` (macOS) or `/proc/mounts` (Linux) — no
  brittle CLI output parsing
- **Disk usage**: POSIX `statvfs(2)` via `libc` — total, used, available bytes
  with `f_frsize` block multiplication
- **Privileged operations**: `sudo -S` with password piped through stdin — no
  terminal suspend, everything stays inside the TUI
- **macOS diskutil**: automatically retries with `diskutil unmount [force]` when
  plain `umount` fails (CoreSimulator volumes, APFS snapshots, etc.)
- **Error output**: both stdout and stderr are captured — nothing leaks onto the TUI

## Project Structure

```
fancymount/
├── Cargo.toml          # Rust project manifest
├── CHOICES.md          # Design decisions & tradeoffs
├── README.md           # This file
└── src/
    ├── main.rs         # Entry point, terminal init, event loop, passthrough
    ├── app.rs          # App state, input dispatch, full TUI rendering
    ├── mount_info.rs   # MountEntry struct + OS-specific mount discovery
    ├── mount_ops.rs    # Mount/unmount/create operations + diskutil fallback
    └── clipboard.rs    # Cross-platform clipboard (pbcopy / xclip / wl-copy)
```

## License

MIT
