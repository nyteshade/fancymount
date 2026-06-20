//! ┌──────────────────────────────────────────────────────────────┐
//! │  clipboard.rs — copy text to the system pasteboard          │
//! └──────────────────────────────────────────────────────────────┘
//!
//! We shell out to the OS-native clipboard tool.  No extra crate
//! dependencies, no C library linking, just a process spawn.
//!
//! # Platform coverage
//!
//! | OS            | Command                         |
//! |---------------|---------------------------------|
//! | macOS         | `pbcopy` (always available)     |
//! | Linux (X11)   | `xclip -selection clipboard`    |
//! | Linux (X11)   | `xsel -ib` (fallback)           |
//! | Linux (WL)    | `wl-copy`                       |
//!
//! # Rust concepts on display
//!
//! * **`std::process::Command` with `.stdin(Stdio::piped())`** — we
//!   write bytes into the child's stdin instead of passing arguments,
//!   which avoids shell-escaping issues with special characters.
//!
//! * **`use std::io::Write`** — brings the `.write_all()` and `.flush()`
//!   methods into scope for `ChildStdin`.  Traits must be `use`d
//!   explicitly; Rust won't import them automatically.
//!
//! * **Method-chaining on `Command`** — `.stdin(…)`, `.stdout(…)`,
//!   `.stderr(…)`, `.spawn()` are all builder-pattern methods that
//!   return `&mut Command` for ergonomic chaining.

use std::io::Write;
use std::process::{Command, Stdio};

/// Copy `text` to the system clipboard.
///
/// Returns `true` if a known clipboard tool accepted the text
/// (exit code 0), `false` otherwise.
///
/// # Why piped stdin?
///
/// `echo "text" | pbcopy` works, but if `text` contains `'`, `"`,
/// `$`, or backticks the shell may mangle it.  Writing to the child's
/// stdin is shell-safe by construction — no escaping needed.
pub fn copy(text: &str) -> bool {
  // Each tool reads from stdin.  xclip and xsel need an extra flag
  // to target the clipboard rather than the primary selection.
  try_piped("pbcopy", &[], text)
    || try_piped("xclip", &["-selection", "clipboard"], text)
    || try_piped("xsel", &["-ib"], text)
    || try_piped("wl-copy", &[], text)
}

// ── helpers ───────────────────────────────────────────────────────────

/// Spawn `cmd` with `args`, write `text` to its stdin, wait for exit.
///
/// `.stderr(Stdio::null())` silences "command not found" noise on
/// platforms where the tool isn't installed.
fn try_piped(cmd: &str, args: &[&str], text: &str) -> bool {
  let mut command = Command::new(cmd);
  command
    .stdin(Stdio::piped())
    .stdout(Stdio::null())
    .stderr(Stdio::null());
  for a in args {
    command.arg(a);
  }

  let mut child = match command.spawn() {
    Ok(c) => c,
    Err(_) => return false,
  };

  if let Some(mut stdin) = child.stdin.take() {
    if stdin.write_all(text.as_bytes()).is_err() {
      return false;
    }
    let _ = stdin.flush();
  }
  // `stdin` dropped → pipe closes → child gets EOF

  match child.wait() {
    Ok(s) => s.success(),
    Err(_) => false,
  }
}
