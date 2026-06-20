//! ┌──────────────────────────────────────────────────────────────┐
//! │  mount_info.rs — platform-specific mount-point discovery     │
//! └──────────────────────────────────────────────────────────────┘
//!
//! # What this module does
//!
//! It gathers every mounted filesystem on the machine into a Vec of
//! `MountEntry` structs.  The heavy lifting is done by two different
//! platform backends selected at compile-time with `#[cfg(...)]`.
//!
//! # Rust concepts on display
//!
//! * **Conditional compilation** — `#[cfg(target_os = "macos")]` and
//!   friends let us ship ONE codebase that compiles natively on macOS
//!   (Intel + Apple Silicon) and Linux without runtime branches.
//!
//! * **FFI with `libc`** — we call POSIX C functions (`getmntinfo`,
//!   `statvfs`) through Rust's `libc` crate.  The `unsafe` keyword is
//!   required because the compiler can't verify C memory safety.
//!
//! * **Type coercion & raw pointers** — `*const i8` ↔ `&[u8]` ↔ `&str`
//!   conversion is explicit and always checked (`.to_string_lossy()`).
//!
//! * **`impl` blocks on structs** — methods like `usage_fraction()` are
//!   attached to `MountEntry` via `impl MountEntry { … }`.
//!
//! * **The `Option` enum** — disk-usage fields are `Option<u64>` because
//!   `statvfs` can fail (e.g. on pseudo-filesystems like `devfs`).
//!   Pattern matching with `match`/`if let` handles the `Some`/`None`
//!   variants safely.
//!
//! * **Zero-cost abstractions** — `Vec::with_capacity(n)` pre-allocates
//!   the exact buffer we need, avoiding reallocations as we push.

use std::fmt;

// ── Public data type ────────────────────────────────────────────────────

/// Every field in this struct is **owned** (`String`, not `&str`).
/// That means the struct holds its own copy of the data and doesn't
/// borrow from anything else — it can be freely passed around,
/// stored in a Vec, sent between threads (if we derived `Send`), etc.
///
/// `#[derive(Clone, Debug)]` auto-generates:
/// * `Clone` — a `.clone()` method that deep-copies every field.
/// * `Debug` — a `{:?}` formatter for `println!("{:?}", entry)`.
#[derive(Clone, Debug)]
pub struct MountEntry {
  /// The block-device or pseudo-filesystem source, e.g. `/dev/disk3s1`.
  pub device: String,
  /// Directory where the filesystem is attached, e.g. `/Volumes/Data`.
  pub mount_point: String,
  /// Filesystem driver name: `"apfs"`, `"ext4"`, `"nfs"`, `"tmpfs"` …
  pub fs_type: String,
  /// Comma-separated mount options: `"rw,noatime,nosuid"`.
  pub options: String,
  /// Total filesystem size in bytes, if `statvfs` succeeded.
  pub total_bytes: Option<u64>,
  /// Bytes available to unprivileged users (`f_bavail`).
  pub avail_bytes: Option<u64>,
  /// Approximation of used space (`total − free`).
  pub used_bytes: Option<u64>,
}

// ── Methods on MountEntry ──────────────────────────────────────────────

/// `impl MountEntry { … }` adds methods that operate on `self`.
/// Methods with `&self` take an immutable reference — they can read
/// but not mutate.  Methods with `&mut self` would allow mutation.
impl MountEntry {
  /// Return the fraction of space used, as a float in `[0.0, 1.0]`.
  ///
  /// `-> Option<f64>` means "either `Some(0.77)` or `None`".
  /// We return `None` when usage data isn't available (pseudo-fs).
  pub fn usage_fraction(&self) -> Option<f64> {
    // `match` destructures a tuple of two Options simultaneously.
    // The `if t > 0` guard prevents division by zero.
    match (self.total_bytes, self.used_bytes) {
      (Some(t), Some(u)) if t > 0 => Some(u as f64 / t as f64),
      _ => None,
    }
  }
}

// ── Display trait ──────────────────────────────────────────────────────

/// Implementing `fmt::Display` means we can do:
///   `println!("{}", entry);`   // or `format!("{entry}")`
/// and it will call this function to produce the string.
///
/// `for<'a>` is higher-ranked lifetime syntax — it says "this impl
/// works for *any* lifetime `'a`" which is what `fmt::Display` expects.
impl fmt::Display for MountEntry {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      f,
      "{} on {} ({})",
      self.device, self.mount_point, self.fs_type
    )
  }
}

// ═══════════════════════════════════════════════════════════════════════
//  PLATFORM-SPECIFIC IMPLEMENTATIONS
// ═══════════════════════════════════════════════════════════════════════
//
// Rust's `#[cfg(target_os = "...")]` is a **compile-time** switch.
// Only ONE of the three `mod platform` blocks below will be compiled
// into the final binary.  The others are dead-code-eliminated.
// This is NOT an `if` statement — there is zero runtime overhead.
//
// Each platform module is private, but we re-export its `gather_mounts`
// function at the bottom of this file as a public item.

// ── macOS backend ──────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
mod platform {
  use super::MountEntry;

  // `libc::*` brings in the raw C bindings.  `use libc::statvfs` would
  // import just that struct; using `libc::` as a prefix keeps it explicit.
  use libc::{getmntinfo, statvfs, MNT_NOWAIT};
  use std::ffi::CStr;          // wrapper for `*const c_char` → `&str`
  use std::mem;                // `mem::zeroed()` for safe zero-init
       // type alias for C's `int`
  use std::ptr;                // `ptr::null_mut()` for null pointer sentinels

  /// Call `getmntinfo(3)` — the macOS / BSD syscall that returns an
  /// array of `statfs` structs describing every mounted filesystem.
  ///
  /// # Safety
  ///
  /// The `unsafe` block is needed because we're calling a C function
  /// through FFI.  `getmntinfo` allocates memory internally; the
  /// returned pointer is valid until the next call.  We immediately
  /// copy the data we need into Rust-owned `String`s so the C memory
  /// can be freed on the next call.
  pub fn gather_mounts() -> Vec<MountEntry> {
    // `ptr::null_mut()` creates a raw mutable pointer initialized to null.
    // `getmntinfo` will overwrite it with a heap pointer.
    let mut mntbuf: *mut libc::statfs = ptr::null_mut();

    // `MNT_NOWAIT` tells the kernel "don't wait for stale NFS info".
    // The return value is the number of entries (negative on error).
    let count = unsafe { getmntinfo(&mut mntbuf, MNT_NOWAIT) };

    // Early return: if count ≤ 0 or pointer is still null, there's nothing.
    if count <= 0 || mntbuf.is_null() {
      return Vec::new();   // `Vec::new()` doesn't allocate heap memory
    }

    let count = count as usize;   // `as` is Rust's type-cast operator

    // `std::slice::from_raw_parts` builds a borrow-checked `&[statfs]`
    // from a raw pointer + length.  This is safe as long as:
    //   * the pointer is valid for `count` elements
    //   * the memory doesn't get freed while the slice is alive
    //   * the data isn't mutated concurrently
    // We uphold all three by copying out immediately.
    let slice = unsafe { std::slice::from_raw_parts(mntbuf, count) };

    // Pre-allocate the Vec so `.push()` never reallocates.
    // This is a micro-optimisation that matters when you have >100 mounts.
    let mut mounts: Vec<MountEntry> = Vec::with_capacity(count);

    for s in slice {
      // Convert C char arrays to Rust Strings.
      // `CStr::from_ptr` wraps the raw `*const i8`, and
      // `.to_string_lossy()` handles non-UTF-8 bytes by replacing
      // them with the Unicode replacement character (�).
      let device = cstr_to_string(&s.f_mntfromname as *const i8);
      let mount_point = cstr_to_string(&s.f_mntonname as *const i8);
      let fs_type = cstr_to_string(&s.f_fstypename as *const i8);

      // Decode the bits in `f_flags` into human-readable option strings.
      // Each `MNT_*` constant is a power of two; we test with bitwise AND.
      let mut opts: Vec<&str> = Vec::new();  // Vec of borrowed string slices

      // `f_flags` is `u32` on macOS but libc constants are `c_int` (i32).
      // `as u32` converts safely since these are small positive bitflags.
      if s.f_flags & (libc::MNT_RDONLY as u32) != 0 { opts.push("ro"); }
      else                                         { opts.push("rw"); }
      if s.f_flags & (libc::MNT_NOATIME as u32) != 0 { opts.push("noatime"); }
      if s.f_flags & (libc::MNT_NOSUID  as u32) != 0 { opts.push("nosuid"); }
      if s.f_flags & (libc::MNT_NODEV   as u32) != 0 { opts.push("nodev"); }
      if s.f_flags & (libc::MNT_NOEXEC  as u32) != 0 { opts.push("noexec"); }
      if s.f_flags & (libc::MNT_SYNCHRONOUS as u32) != 0 { opts.push("sync"); }
      if s.f_flags & (libc::MNT_UNION   as u32) != 0 { opts.push("union"); }
      if s.f_flags & (libc::MNT_LOCAL   as u32) == 0 { opts.push("network"); }

      // `join(",")` concatenates the vec elements with commas:
      // `["rw","noatime"]` becomes `"rw,noatime"`
      let options = opts.join(",");

      // Get disk usage via statvfs (may return None for pseudo-fs)
      let (total, avail, used) = get_usage(&mount_point);

      // Struct literal — field names match the struct definition.
      // `device` is shorthand for `device: device` (field init shorthand).
      mounts.push(MountEntry {
        device,
        mount_point,
        fs_type,
        options,
        total_bytes: total,
        avail_bytes: avail,
        used_bytes: used,
      });
    }

    mounts   // implicit return (no semicolon = return value)
  }

  // ── helper: null-terminated C string → Rust String ──────────────

  /// Convert a `*const i8` (C string pointer) to an owned Rust `String`.
  /// `-> String` returns an owned value; the caller takes ownership.
  fn cstr_to_string(ptr: *const i8) -> String {
    if ptr.is_null() {
      return String::new();   // `String::new()` is an empty, heap-allocated String
    }
    // SAFETY: we trust the kernel to give us valid null-terminated strings.
    let cstr = unsafe { CStr::from_ptr(ptr) };
    // `.to_string_lossy()` returns a `Cow<str>` — Copy-On-Write.
    // `.into_owned()` extracts the `String` (only allocates if non-UTF-8).
    cstr.to_string_lossy().into_owned()
  }

  // ── helper: disk usage via POSIX statvfs ────────────────────────

  /// Query total, available, and used bytes for a mountpoint.
  /// Returns a 3-tuple of `Option<u64>` — `None` means statvfs failed
  /// (common for pseudo-filesystems like devfs or proc).
  fn get_usage(path: &str) -> (Option<u64>, Option<u64>, Option<u64>) {
    // `CString::new` creates a null-terminated byte sequence.
    // It fails if `path` contains an interior null byte.
    let cpath = std::ffi::CString::new(path).unwrap_or_default();

    // `mem::zeroed()` is the safe way to get a zeroed struct.
    // It works for any type that implements no-padding guarantees.
    let mut vfs: statvfs = unsafe { mem::zeroed() };

    // SAFETY: `cpath` is a valid null-terminated string.
    // `&mut vfs` passes a mutable reference — Rust's borrow checker
    // guarantees no aliasing at compile time.
    let rc = unsafe { libc::statvfs(cpath.as_ptr(), &mut vfs) };
    if rc != 0 {
      return (None, None, None);   // statvfs failed (pseudo-fs, permissions)
    }

    // `f_frsize` is the "fundamental" block size (may differ from `f_bsize`).
    // We multiply blocks × block_size to get bytes.
    let block_size = vfs.f_frsize as u64;
    // `saturating_mul` prevents integer overflow — it caps at u64::MAX
    // instead of wrapping around (which would be a security bug).
    let total = (vfs.f_blocks  as u64).saturating_mul(block_size);
    let avail = (vfs.f_bavail  as u64).saturating_mul(block_size);
    let free  = (vfs.f_bfree   as u64).saturating_mul(block_size);
    let used  = total.saturating_sub(free);  // `total - free`, clamped at 0

    (Some(total), Some(avail), Some(used))
  }
}

// ── Linux backend ──────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
mod platform {
  use super::MountEntry;
  use std::fs;      // `fs::read_to_string` — reads an entire file into a String
  use std::mem;

  /// Read and parse `/proc/mounts`.
  ///
  /// `/proc/mounts` is a virtual file maintained by the kernel.  Each
  /// line has 6 space-separated fields:
  ///   `device mountpoint fstype options dump_freq pass_num`
  ///
  /// Spaces and special chars in device/mountpoint are octal-escaped
  /// (e.g. `\040` = space), so we decode those.
  pub fn gather_mounts() -> Vec<MountEntry> {
    // `or_else` chains fallbacks: try `/proc/mounts`, then `/proc/self/mounts`.
    // `unwrap_or_default()` gives `""` if both fail — `String::default()` is `""`.
    let content = fs::read_to_string("/proc/mounts")
      .or_else(|_| fs::read_to_string("/proc/self/mounts"))
      .unwrap_or_default();

    // `.lines()` returns an iterator over `&str` slices, borrowing from `content`.
    // Each `&str` is valid only while `content` is alive.
    let mut mounts = Vec::new();

    for line in content.lines() {
      // `split_whitespace()` splits on any whitespace, returning an iterator.
      // `.collect()` gathers iterator items into a collection — here `Vec<&str>`.
      let parts: Vec<&str> = line.split_whitespace().collect();
      if parts.len() < 4 {
        continue;   // skip malformed lines (shouldn't happen, but be safe)
      }

      let device      = unescape_mount(parts[0]);
      let mount_point = unescape_mount(parts[1]);
      let fs_type     = parts[2].to_string();   // `to_string()` copies a `&str` into an owned `String`
      let options     = parts[3].to_string();

      let (total, avail, used) = get_usage(&mount_point);

      mounts.push(MountEntry {
        device,
        mount_point,
        fs_type,
        options,
        total_bytes: total,
        avail_bytes: avail,
        used_bytes: used,
      });
    }
    mounts
  }

  /// Decode Linux's octal escape sequences: `\040` → ` `, `\134` → `\\`.
  ///
  /// We iterate over raw bytes (`u8`).  When we see `\` followed by
  /// three octal digits, we parse and convert.  Otherwise we copy
  /// the byte through as a char.
  fn unescape_mount(s: &str) -> String {
    let mut out = String::with_capacity(s.len());  // pre-allocate for speed
    let bytes = s.as_bytes();  // `&str` → `&[u8]` without copying
    let mut i = 0;

    while i < bytes.len() {
      // `bytes[i]` is a `u8` — Rust arrays/slices are indexed by `usize`.
      // `b'\\'` is a byte literal (the ASCII code for backslash).
      if bytes[i] == b'\\' && i + 3 < bytes.len() {
        // `s[i+1..i+4]` is a slice of the original `&str`.
        // `u8::from_str_radix(…, 8)` parses base-8 (octal).
        // The `if let Ok(oct) = …` pattern only executes if parsing succeeds.
        if let Ok(oct) = u8::from_str_radix(&s[i + 1..i + 4], 8) {
          out.push(oct as char);
          i += 4;
          continue;   // skip to next iteration of the loop
        }
      }
      out.push(bytes[i] as char);
      i += 1;
    }
    out
  }

  /// Linux `statvfs` call — identical logic to the macOS version
  /// but uses `libc::statvfs` directly (same POSIX function).
  fn get_usage(path: &str) -> (Option<u64>, Option<u64>, Option<u64>) {
    let cpath = std::ffi::CString::new(path).unwrap_or_default();
    let mut vfs: libc::statvfs = unsafe { mem::zeroed() };
    let rc = unsafe { libc::statvfs(cpath.as_ptr(), &mut vfs) };
    if rc != 0 {
      return (None, None, None);
    }
    let block_size = vfs.f_frsize as u64;
    let total = (vfs.f_blocks  as u64).saturating_mul(block_size);
    let avail = (vfs.f_bavail  as u64).saturating_mul(block_size);
    let free  = (vfs.f_bfree   as u64).saturating_mul(block_size);
    let used  = total.saturating_sub(free);
    (Some(total), Some(avail), Some(used))
  }
}

// ── Fallback for other Unixes (BSD, Solaris, etc.) ─────────────────────

/// If we're on neither macOS nor Linux we compile an empty stub.
/// The app will still run but show no mounts.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
  use super::MountEntry;
  pub fn gather_mounts() -> Vec<MountEntry> {
    Vec::new()   // empty vec — no mounts shown
  }
}

// ── Re-export ─────────────────────────────────────────────────────────
//
// This is the ONLY public item from this module that other modules see.
// `pub use platform::gather_mounts` takes the private function from
// whichever `mod platform` was compiled and makes it available as
// `mount_info::gather_mounts`.
pub use platform::gather_mounts;
