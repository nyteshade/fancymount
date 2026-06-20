//! ┌──────────────────────────────────────────────────────────────┐
//! │  mount_ops.rs — mount / unmount / create actions             │
//! └──────────────────────────────────────────────────────────────┘
//!
//! # What this module does
//!
//! Provides three core operations the TUI calls when the user presses
//! `m` (mount), `u` (unmount), or `f` (force-unmount).  Because
//! mounting requires root, we shell out to `sudo`.
//!
//! # Rust concepts on display
//!
//! * **`std::process::Command`** — the safe, cross-platform way to spawn
//!   child processes.  Builder pattern: `Command::new("sudo").arg("mount")`.
//!
//! * **`std::fs`** — filesystem operations like `create_dir_all`, which
//!   mirrors `mkdir -p` — creates all missing parent directories.
//!
//! * **`std::path::Path`** — a platform-aware path type.  `Path::new(s)`
//!   doesn't allocate; it just borrows the string slice.
//!
//! * **Error handling with `Result` and `match`** — `Command::status()`
//!   returns `io::Result<ExitStatus>`.  We match on `Ok(s)` / `Err(e)`.
//!
//! * **`const` slices** — `FS_TYPES` is a compile-time constant array
//!   of `&str` references.  `&[&str]` means "borrowed slice of borrowed
//!   string slices" — no heap allocation for the list itself.
//!
//! * **Formatting macros** — `format!("{:.1} {}", …)` does decimal
//!   formatting at runtime.  The `:.1` means "one decimal place".

use std::fs;
use std::path::Path;
use std::process::{Command, ExitStatus};

// ── Public types ───────────────────────────────────────────────────────

/// The outcome of a mount/unmount operation that the TUI displays
/// in the status bar or a transient notification.
///
/// We use a plain struct instead of `Result<(), String>` because both
/// success and failure carry a human-readable message.
#[derive(Debug, Clone)]
pub struct OpResult {
  /// `true` if the operation completed without error.
  pub success: bool,
  /// User-facing description of what happened (or what went wrong).
  pub message: String,
}

// ── Filesystem type presets ────────────────────────────────────────────

/// A curated list of common filesystem types for the "New Mount" dialog's
/// dropdown.  `&'static` means these string literals live in the binary's
/// read-only data section and are valid for the entire program lifetime.
pub static FS_TYPES: &[&str] = &[
  "auto",    // let the kernel probe
  "apfs",    // Apple File System (macOS)
  "hfs",     // Hierarchical File System (older macOS)
  "exfat",   // Extended FAT (cross-platform flash drives)
  "fat32",   // FAT32 (legacy, max 4 GB files)
  "ntfs",    // Windows NTFS
  "ext2",    // Second Extended FS (no journal)
  "ext3",    // Third Extended FS (journal)
  "ext4",    // Fourth Extended FS (modern Linux default)
  "xfs",     // SGI XFS (RHEL default)
  "btrfs",   // B-tree FS (copy-on-write, snapshots)
  "zfs",     // Zettabyte FS (advanced, Solaris/BSD/Linux)
  "nfs",     // Network File System v3
  "nfs4",    // Network File System v4
  "smbfs",   // SMB / CIFS (Windows shares)
  "tmpfs",   // RAM-backed temporary filesystem
  "devfs",   // Device filesystem
  "proc",    // Process information pseudo-fs
  "sysfs",   // Kernel objects pseudo-fs
  "cifs",    // Common Internet File System
  "fuse",    // Filesystem in Userspace (generic)
  "sshfs",   // FUSE over SSH
  "iso9660", // CD/DVD-ROM filesystem
  "udf",     // Universal Disk Format (DVD/Blu-ray)
  "msdos",   // FAT12/16 (very old)
];

// ═══════════════════════════════════════════════════════════════════════
//  PUBLIC API
// ═══════════════════════════════════════════════════════════════════════

/// Mount a device at a specified mountpoint.
///
/// # Arguments
/// * `device`     — source, e.g. `/dev/disk4s2` or `//server/share`
/// * `mountpoint` — target directory; created via `mkdir -p` if missing
/// * `fs_type`    — filesystem type (`"auto"` or `""` to let kernel guess)
/// * `options`    — comma-separated mount options (`"rw,noatime"`)
///
/// # Returns
/// `OpResult` with `.success = true` if `sudo mount` exited with code 0.
pub fn mount_device(
  device: &str,
  mountpoint: &str,
  fs_type: &str,
  options: &str,
  password: &str,
) -> OpResult {
  // `Path::new` is zero-cost — it just wraps the &str, no heap alloc.
  // `.exists()` does a `stat(2)` syscall under the hood.
  if !Path::new(mountpoint).exists() {
    // `create_dir_all` = `mkdir -p`.  Returns `io::Result<()>`.
    // `if let Err(e) = …` is pattern matching on the Result enum.
    if let Err(e) = fs::create_dir_all(mountpoint) {
      return OpResult {
        success: false,
        message: format!("Failed to create mountpoint {}: {}", mountpoint, e),
      };
    }
  }

  // Build the command with the builder pattern.
  // `Command::new` returns a `Command` struct on the stack.
  // Each `.arg()` borrows `&mut self` and returns `&mut Command`,
  // so we chain them.  This is a common Rust API design.
  let mut cmd = Command::new("sudo");
  cmd.arg("mount");

  // `is_empty()` checks length == 0 for both String and &str.
  if !fs_type.is_empty() && fs_type != "auto" {
    cmd.arg("-t").arg(fs_type);
  }
  if !options.is_empty() {
    cmd.arg("-o").arg(options);
  }
  cmd.arg(device).arg(mountpoint);

  run_privileged(cmd, "mount", password)
}

/// Unmount a filesystem by its mountpoint path.
///
/// Equivalent to: `sudo umount /Volumes/Data`
pub fn unmount_device(mountpoint: &str, password: &str) -> OpResult {
  let mut cmd = Command::new("sudo");
  cmd.arg("umount").arg(mountpoint);
  let result = run_privileged(cmd, "unmount", password);

  // ── macOS diskutil fallback ──────────────────────────────────
  // When umount fails with "Resource busy", macOS tells the user
  // to try `diskutil unmount`.  We do that automatically so the
  // user doesn't have to retry manually.
  #[cfg(target_os = "macos")]
  if !result.success {
    return diskutil_unmount(mountpoint, password, false);
  }

  result
}

/// Force-unmount.  Uses:
/// * Linux:   `umount -l` (lazy — detaches immediately, cleans up later)
/// * macOS:   `umount -f` (force — sends SIGKILL to processes using the fs)
///
/// The `#[cfg]` attributes mean only ONE of these arg lines is compiled.
/// There's no runtime `if` check for the OS.
pub fn force_unmount(mountpoint: &str, password: &str) -> OpResult {
  let mut cmd = Command::new("sudo");
  cmd.arg("umount");

  #[cfg(target_os = "linux")]
  cmd.arg("-l");  // lazy unmount — immediate detach, deferred cleanup

  #[cfg(target_os = "macos")]
  cmd.arg("-f");  // force unmount — aggressive, may interrupt processes

  cmd.arg(mountpoint);
  let result = run_privileged(cmd, "force unmount", password);

  // macOS diskutil fallback for force-unmount.
  #[cfg(target_os = "macos")]
  if !result.success {
    return diskutil_unmount(mountpoint, password, true);
  }

  result
}

// ── Utility: human-readable byte sizes ────────────────────────────────

/// Format a byte count into a compact human string.
///
/// # Examples
/// ```
/// format_bytes(0)         → "0 B"
/// format_bytes(1023)       → "1023 B"
/// format_bytes(1536)       → "1.5 KB"
/// format_bytes(1_610_612_736) → "1.5 GB"
/// ```
///
/// Uses `f64` to get fractional units.  `as` is Rust's cast operator —
/// `bytes as f64` converts a u64 to an IEEE 754 float (lossless up to 2⁵³).
pub fn format_bytes(bytes: u64) -> String {
  // `&[&str]` is a slice reference — const array of string slices.
  const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
  let mut size = bytes as f64;
  let mut unit_idx = 0usize;  // `usize` is the pointer-sized unsigned integer

  // Keep dividing by 1024 until the number is < 1024 or we run out of units.
  while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
    size /= 1024.0;
    unit_idx += 1;
  }

  if unit_idx == 0 {
    // For bytes, show integers only.  `format!` is like `printf` but
    // type-safe: `{}` uses Display, `{:?}` uses Debug.
    format!("{} {}", bytes, UNITS[unit_idx])
  } else {
    // For KB+, show one decimal place.
    format!("{:.1} {}", size, UNITS[unit_idx])
  }
}

// ═══════════════════════════════════════════════════════════════════════
//  INTERNAL HELPERS
// ═══════════════════════════════════════════════════════════════════════

/// Run a command that needs root.  When `password` is non-empty we
/// use `sudo -S` (read password from stdin) and pipe the password
/// in — no terminal suspend needed.  When empty we fall back to
/// normal sudo (reads from /dev/tty).
fn run_privileged(mut cmd: Command, label: &str, password: &str) -> OpResult {
  use std::io::{Read, Write};
  use std::process::Stdio;

  if !password.is_empty() {
    let program = cmd.get_program().to_os_string();
    let args: Vec<_> = cmd.get_args().map(|a| a.to_os_string()).collect();
    cmd = Command::new(program);
    cmd.arg("-S");
    for a in &args {
      cmd.arg(a);
    }
    cmd.stdin(Stdio::piped());
  }
  // Silence both stdout and stderr so nothing leaks onto the TUI.
  // We capture stderr separately to include in error messages.
  cmd.stdout(Stdio::null());
  cmd.stderr(Stdio::piped());

  let mut child = match cmd.spawn() {
    Ok(c) => c,
    Err(e) => {
      return OpResult {
        success: false,
        message: format!("{} error: {} (is sudo installed?)", label, e),
      };
    }
  };

  if !password.is_empty() {
    if let Some(mut stdin) = child.stdin.take() {
      let _ = stdin.write_all(password.as_bytes());
      let _ = stdin.write_all(b"\n");
      let _ = stdin.flush();
    }
  }

  // Read stderr into a String so we can show it on failure.
  let mut stderr_str = String::new();
  if let Some(mut stderr_pipe) = child.stderr.take() {
    let _ = stderr_pipe.read_to_string(&mut stderr_str);
  }

  let status = child.wait();

  match status {
    Ok(s) if s.success() => OpResult {
      success: true,
      message: format!("{} succeeded", label),
    },
    Ok(s) => {
      let code = exit_code_to_string(&s);
      let detail = if stderr_str.trim().is_empty() {
        String::new()
      } else {
        format!(": {}", stderr_str.trim())
      };
      OpResult {
        success: false,
        message: format!("{} failed with exit code {}{}", label, code, detail),
      }
    }
    Err(e) => OpResult {
      success: false,
      message: format!("{} error: {}", label, e),
    },
  }
}

/// macOS-specific: try `diskutil unmount [force]` as a fallback when
/// plain `umount` fails (common for CoreSimulator volumes, APFS
/// system volumes, etc.).
#[cfg(target_os = "macos")]
fn diskutil_unmount(mountpoint: &str, password: &str, force: bool) -> OpResult {
  use std::io::{Read, Write};
  use std::process::Stdio;

  let mut cmd = Command::new("sudo");
  if !password.is_empty() {
    cmd.arg("-S");
    cmd.stdin(Stdio::piped());
  }
  cmd.arg("diskutil").arg("unmount");
  if force {
    cmd.arg("force");
  }
  cmd.arg(mountpoint);
  cmd.stdout(Stdio::null());
  cmd.stderr(Stdio::piped());   // capture errors, don't leak to TUI

  let label = if force { "diskutil unmount force" } else { "diskutil unmount" };

  let mut child = match cmd.spawn() {
    Ok(c) => c,
    Err(e) => {
      return OpResult {
        success: false,
        message: format!("{} error: {}", label, e),
      };
    }
  };

  if !password.is_empty() {
    if let Some(mut stdin) = child.stdin.take() {
      let _ = stdin.write_all(password.as_bytes());
      let _ = stdin.write_all(b"\n");
      let _ = stdin.flush();
    }
  }

  let mut stderr_str = String::new();
  if let Some(mut stderr_pipe) = child.stderr.take() {
    let _ = stderr_pipe.read_to_string(&mut stderr_str);
  }

  match child.wait() {
    Ok(s) if s.success() => OpResult {
      success: true,
      message: format!("{} succeeded (via diskutil)", label),
    },
    Ok(s) => {
      let detail = if stderr_str.trim().is_empty() {
        String::new()
      } else {
        format!(": {}", stderr_str.trim())
      };
      OpResult {
        success: false,
        message: format!(
          "{} failed with exit code {}{}",
          label,
          exit_code_to_string(&s),
          detail,
        ),
      }
    }
    Err(e) => OpResult {
      success: false,
      message: format!("{} error: {}", label, e),
    },
  }
}

/// Convert an `ExitStatus` to a displayable string.
///
/// `.code()` returns `Option<i32>` — `Some(n)` for a normal exit,
/// `None` if the process was killed by a signal.
fn exit_code_to_string(s: &ExitStatus) -> String {
  // `match` on an Option: `Some(c)` binds the value to `c`,
  // `None` is the signal-killed case.
  match s.code() {
    Some(c) => c.to_string(),   // i32 → String via Display trait
    None    => "killed by signal".into(),  // `.into()` infers String from return type
  }
}
