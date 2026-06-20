//! ┌──────────────────────────────────────────────────────────────┐
//! │  main.rs — entry point, terminal init, event loop            │
//! └──────────────────────────────────────────────────────────────┘
//!
//! # How the event loop works
//!
//! 1. Enter **raw mode** (crossterm): the terminal sends every keystroke
//!    directly to us, no line buffering.  This lets us react to arrow
//!    keys instantly.
//! 2. Enter the **alternate screen**: ratatui's `Terminal` manages an
//!    off-screen buffer and swaps it atomically (no flicker).
//! 3. Loop:
//!    a. Poll for a keyboard event with a 100ms timeout.
//!    b. If a key arrived, feed it to `app.handle_key()`.
//!    c. If `handle_key` returns `Some(OpKind)`, we **suspend** the TUI:
//!       - disable raw mode
//!       - clear the alternate screen
//!       - run `sudo mount …` (user sees password prompt normally)
//!       - restore raw mode + alternate screen
//!       - feed the `OpResult` back to `app.on_op_result()`
//!    d. Call `app.tick()` for status-message countdown.
//!    e. Call `app.render()` to draw the frame.
//! 4. On `app.should_quit == true`, clean up and exit.
//!
//! # Rust concepts on display
//!
//! * **`mod` declarations** — `mod app;` tells Rust to look for
//!   `src/app.rs` (or `src/app/mod.rs`).  `mod` is NOT like C's
//!   `#include` — it creates a real module with privacy boundaries.
//!
//! * **`use` paths** — `use crate::app::App` brings the struct into
//!   scope.  `crate::` is the root of our project.
//!
//! * **`std::io`** — the standard I/O module.  `io::stdout()` returns a
//!   `Stdout` handle.  `.lock()` gives a `StdoutLock<'_>` for buffered
//!   writing (released when the lock goes out of scope — RAII).
//!
//! * **`match` with guard clauses** — `Event::Key(key) if …` filters
//!   events inline.
//!
//! * **`crossterm::event::poll`** — non-blocking check with timeout.
//!   `Duration::from_millis(100)` creates a `std::time::Duration`.
//!
//! * **`crossterm::execute!` macro** — a macro that takes a writer and
//!   one or more terminal commands.  Macro because each command is a
//!   different type, and we want variadic arguments without boxing.
//!
//! * **Drop-based cleanup** — `Terminal`'s `Drop` impl restores the
//!   original terminal state even if we panic.  Ratatui recommends an
//!   explicit `restore()` call for clarity, but `Drop` is the safety net.

// `mod` declarations create the module tree.  Each one looks for a
// corresponding file: `mod app` → `src/app.rs`, etc.
mod app;
mod clipboard;
mod mount_info;
mod mount_ops;

// `use` imports the items we need.  `crate::` is the root namespace.
use crate::app::{execute_privileged_op, App};

use crossterm::{
  event::{self, Event, KeyEventKind},
  execute,
  terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use std::io::{self, stdout};
use std::process;
use std::time::Duration;

fn main() -> io::Result<()> {
  // ── 0. Passthrough mode: if the user passes arguments, behave   ──
  //    like the real `mount` command.  This lets you `alias mount=fm`
  //    without breaking scripts that call `mount -t …`.
  let args: Vec<String> = std::env::args().collect();
  if args.len() > 1 {
    return passthrough_to_mount(&args[1..]);
  }

  // ── 1. Terminal setup ──────────────────────────────────────────
  //
  // `enable_raw_mode()` disables line buffering and echo so we get
  // each keypress immediately.  Returns `io::Result<()>`.
  enable_raw_mode()?;

  // `execute!` is a crossterm macro.  It takes a writer (stdout) and
  // one or more `Command` impls.  `EnterAlternateScreen` switches to
  // the alternate buffer — the app's drawing canvas.
  let mut stdout = stdout().lock();
  execute!(stdout, EnterAlternateScreen)?;

  // `CrosstermBackend` wraps stdout.  `Terminal::new` takes ownership
  // of the backend.  `?` propagates any I/O error up to `main`.
  let backend = CrosstermBackend::new(stdout);
  let mut terminal = ratatui::Terminal::new(backend)?;

  // ── 2. Application state ───────────────────────────────────────
  //
  // `App::new()` gathers mounts from the OS.  `mut` because we'll
  // mutate it on every event.
  let mut app = App::new();

  // ── 3. Event loop ──────────────────────────────────────────────
  //
  // Rust doesn't have a `while true` keyword, but `loop { … }` is
  // the infinite-loop construct.  `break` exits it.
  let tick_rate = Duration::from_millis(100);

  loop {
    // Draw the current frame.  `terminal.draw(|f| { … })` takes a
    // closure with `&mut Frame`.  The closure borrows `app` mutably —
    // the borrow checker guarantees we can't draw while someone else
    // is mutating `app` on another thread (not that we have threads).
    terminal.draw(|f| app.render(f))?;

    // Poll for an event with a timeout.  `event::poll(d)` returns
    // `Ok(true)` if an event is available, `Ok(false)` on timeout,
    // `Err(_)` on I/O error.
    if event::poll(tick_rate)? {
      // `event::read()` blocks until an event arrives.  Since we just
      // confirmed one is available with `poll`, this won't block.
      match event::read()? {
        Event::Key(key) => {
          // Ignore key-release and repeat events —
          // we only care about the initial press.
          if key.kind != KeyEventKind::Press {
            continue;
          }

          // Dispatch the key to the app.  `handle_key` may return
          // `Some(OpKind)` if a privileged operation is needed.
          if let Some(op) = app.handle_key(key) {
            // If we already have a cached password from a previous
            // successful operation, skip the prompt and go straight
            // to execution.  Otherwise show the password dialog.
            if let Some(ref pw) = app.cached_password {
              app.pending_privileged = Some((op, pw.clone()));
            } else {
              app.set_password_prompt(op);
            }
          }

          // Ctrl+L clears the cached sudo password (re-lock).
          if key.code == crossterm::event::KeyCode::Char('l')
            && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
          {
            app.cached_password = None;
            app.set_status("Sudo credentials cleared.");
          }
        }
        Event::Resize(_, _) => {
          // Terminal was resized — ratatui handles the layout
          // recalculation on the next `draw()`.
        }
        _ => {}  // ignore mouse events, focus events, etc.
      }
    }

    // ── Check for a pending privileged operation ─────────────────
    //
    // When the user submits the password prompt, `pending_privileged`
    // is populated with the (operation, password) pair.  We consume
    // it, run sudo with the password piped to stdin, and feed the
    // result back to the app — all without leaving the TUI.
    if let Some((op, password)) = app.pending_privileged.take() {
      let result = execute_privileged_op(op, &password);
      app.on_op_result(&result);
      // Cache the password on success so the user doesn't have to
      // re-enter it for every operation.
      if result.success {
        app.cached_password = Some(password);
      }
    }

    // Tick the status message countdown.
    app.tick();

    // Check quit flag (set by 'q', Esc, or Ctrl+C).
    if app.should_quit {
      break;
    }
  }

  // ── 4. Cleanup ─────────────────────────────────────────────────
  //
  // Restore the terminal to its original state.  `Drop` impls on
  // `Terminal` and `CrosstermBackend` would do this automatically
  // when they go out of scope, but explicit restoration is clearer.
  terminal.clear()?;
  disable_raw_mode()?;
  execute!(io::stdout(), LeaveAlternateScreen)?;

  Ok(())
}

// ═══════════════════════════════════════════════════════════════════════
//  PASSTHROUGH — behave like the real `mount` when given arguments
// ═══════════════════════════════════════════════════════════════════════

/// When the user runs `fm -t ext4 /dev/sda1 /mnt` (i.e. with arguments),
/// we act as a transparent wrapper around the system's real `mount`
/// binary.  This means you can `alias mount=fm` and scripts that call
/// `mount` with flags will still work correctly.
///
/// We use an absolute path to the real mount so we don't accidentally
/// re-invoke ourselves (which would be an infinite loop under an alias).
///
/// # Platform paths
///
/// | macOS           | `/sbin/mount`                     |
/// | Linux (common)  | `/bin/mount` → `/usr/bin/mount` |
///
/// The exit code is forwarded so callers can check success/failure.
fn passthrough_to_mount(args: &[String]) -> io::Result<()> {
  // Absolute paths — bypass PATH lookup so an alias won't cause recursion.
  let mount_path = if cfg!(target_os = "macos") {
    "/sbin/mount"
  } else {
    // Linux: try a few common locations.
    if std::path::Path::new("/bin/mount").exists() {
      "/bin/mount"
    } else {
      "/usr/bin/mount"
    }
  };

  // `Command::new` takes the program; `.args()` passes the rest.
  // `.status()` runs the child and waits for it to finish.
  let status = process::Command::new(mount_path)
    .args(args)
    .status()
    .map_err(|e| {
      io::Error::new(
        io::ErrorKind::NotFound,
        format!("could not run {}: {}", mount_path, e),
      )
    })?;

  // Forward the real mount's exit code.  `process::exit()` terminates
  // immediately — no destructors run, but that's fine for a CLI tool.
  if let Some(code) = status.code() {
    process::exit(code);
  }
  // If the process was killed by a signal, use 128 + signal number
  // (convention followed by bash, dash, etc.).
  #[cfg(unix)]
  {
    use std::os::unix::process::ExitStatusExt;
    if let Some(sig) = status.signal() {
      process::exit(128 + sig);
    }
  }
  process::exit(1);
}
