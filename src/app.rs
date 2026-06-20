//! ┌──────────────────────────────────────────────────────────────┐
//! │  app.rs — application state + UI rendering + input handling  │
//! └──────────────────────────────────────────────────────────────┘
//!
//! # Architecture
//!
//! The `App` struct owns all mutable state.  The main loop (in `main.rs`)
//! pumps events into `App::handle_input()`, then calls `App::render()`.
//! This is a classic Elm-like / Redux-like unidirectional data flow:
//!
//!   Event → update state → redraw
//!
//! No callbacks, no channels — just plain function calls.
//!
//! # Rust concepts on display
//!
//! * **Enums with data** — `Mode` is an enum where variants carry data
//!   (e.g. `Mode::NewMount { … }`).  This is Rust's "sum type" —
//!   like a tagged union, but memory-safe.
//!
//! * **`impl` methods with `&mut self`** — mutable methods borrow
//!   `&mut self`, which guarantees **exclusive** access at compile time.
//!   No data races, no accidental aliasing.
//!
//! * **Pattern matching** — `match self.mode { Mode::Normal => …, … }`
//!   is exhaustive: the compiler errors if you forget a variant.
//!
//! * **Bounded integers** — `usize` for indices.  `saturating_sub(1)`
//!   prevents underflow (clamps to 0 instead of wrapping to MAX).
//!
//! * **Lifetime elision** — in `fn render(&self, …)` the compiler
//!   infers that the returned `Rect` borrows from nothing and the
//!   references in parameters are independent.
//!
//! * **Terminal styling with `ratatui::style`** — Color, Modifier (bold,
//!   dim), and Style are builder-pattern types.  `Color::Rgb(r,g,b)`
//!   supports true color terminals.

use crate::mount_info::{gather_mounts, MountEntry};
use crate::mount_ops::{self, OpResult};

use ratatui::{
  layout::{Alignment, Constraint, Direction, Layout, Rect},
  style::{Color, Modifier, Style},
  text::{Line, Span},
  widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap,
  },
  Frame,
};

// ═══════════════════════════════════════════════════════════════════════
//  APPLICATION STATE
// ═══════════════════════════════════════════════════════════════════════

/// Which UI "screen" we're on.  `Normal` is the two-pane mount browser.
/// All dialog variants stack on top as modal overlays.
///
/// Each variant can carry its own state.  `NewMount` has six fields
/// for the form inputs.  `Message` has a title + body + optional
/// `on_close` action.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
  /// Main two-pane view: mount list + detail pane.
  Normal,
  /// "New Mount" dialog form.
  NewMount {
    device: String,
    mountpoint: String,
    fs_type_idx: usize,   // index into `mount_ops::FS_TYPES`
    options: String,
    active_field: u8,     // 0=device, 1=mountpoint, 2=fs_type, 3=options
  },
  /// "Mount from list" dialog — like NewMount but pre-filled from
  /// a suggestion (e.g. a umounted device).
  MountSelected {
    device: String,
    mountpoint: String,
    fs_type_idx: usize,
    options: String,
    active_field: u8,
  },
  /// Transient message overlay (errors, confirmations).
  Message {
    title: String,
    body: String,
    /// If `true`, hitting Enter/Esc refreshes the mount list.
    refresh_on_close: bool,
  },
  /// Small auto-dismissing toast for clipboard copies etc.
  /// Lives at the bottom of the screen for ~2 seconds.
  Toast {
    text: String,
    ticks: usize,
  },
  /// Help screen overlay.
  Help,
  /// Sudo password prompt — shown before privileged operations.
  /// The password field stores masked input (displayed as ••••).
  /// On Enter the operation + password are moved to
  /// `App::pending_privileged` for main.rs to execute.
  PasswordPrompt {
    operation: OpKind,
    password: String,
  },
}

/// The root application struct.  `'static` is not needed here because
/// we own all our data — no borrowed references escape.
pub struct App {
  /// All mount points, refreshed from the OS.
  mounts: Vec<MountEntry>,
  /// Which mount is highlighted in the list.
  selected: usize,
  /// First visible row in the list (for scrolling).
  scroll_offset: usize,
  /// Current UI screen.
  mode: Mode,
  /// `true` when the detail pane has focus; `false` when the list has focus.
  detail_focused: bool,
  /// Scroll position within the detail pane text.
  detail_scroll: usize,
  /// Status bar message (shown for a few seconds).
  status_msg: String,
  /// Countdown ticks until the status message is cleared.
  status_ticks: usize,
  /// Should the app exit? Set to true to break the main loop.
  pub should_quit: bool,
  /// When set, main.rs picks this up, runs the privileged operation
  /// (piping the password to sudo -S), then clears it.
  pub pending_privileged: Option<(OpKind, String)>,
  /// Cached sudo password — entered once, reused for subsequent ops.
  /// Set after the first successful privileged operation.  Press
  /// Ctrl+L to clear it (re-lock).
  pub cached_password: Option<String>,
}

impl App {
  /// Create a fresh `App` with mounts gathered from the OS.
  ///
  /// `new()` is a **constructor** — by convention, Rust types use `new()`
  /// for the default constructor (no arguments).  It returns `Self`,
  /// which is a type alias for the current `impl` type (`App`).
  pub fn new() -> Self {
    let mounts = gather_mounts();
    App {
      mounts,
      selected: 0,
      scroll_offset: 0,
      mode: Mode::Normal,
      detail_focused: false,
      detail_scroll: 0,
      status_msg: String::new(),
      status_ticks: 0,
      should_quit: false,
      pending_privileged: None,
      cached_password: None,
    }
  }

  // ── public helpers ──────────────────────────────────────────────

  /// Refresh the mount list from the OS.
  /// Preserves the selected index if possible (clamped to new length).
  pub fn refresh_mounts(&mut self) {
    self.mounts = gather_mounts();
    // Clamp selection to valid range.  `saturating_sub(1)` means
    // `len - 1` but returns 0 if len is already 0.
    if self.mounts.is_empty() {
      self.selected = 0;
    } else if self.selected >= self.mounts.len() {
      self.selected = self.mounts.len().saturating_sub(1);
    }
    self.detail_scroll = 0;
  }

  /// Show a transient status message for ~3 seconds (30 ticks at ~10 fps).
  pub fn set_status(&mut self, msg: &str) {
    self.status_msg = msg.to_string();
    self.status_ticks = 30;  // ~3 seconds at 100ms ticks
  }

  /// Transition to the in-TUI password prompt for a privileged operation.
  pub fn set_password_prompt(&mut self, op: OpKind) {
    self.mode = Mode::PasswordPrompt {
      operation: op,
      password: String::new(),
    };
  }

  /// Flash a toast notification near the bottom of the screen.
  /// Auto-dismisses after ~2 seconds (20 ticks).
  /// More prominent than the status bar for clipboard confirmations.
  pub fn toast(&mut self, msg: &str) {
    self.mode = Mode::Toast {
      text: msg.to_string(),
      ticks: 20,  // ~2 seconds
    };
  }

  // ── input handling ──────────────────────────────────────────────

  /// Dispatch a key event.  This is the main "reducer" — it pattern-
  /// matches on the current `Mode` and the key pressed, then mutates
  /// `self` accordingly.
  ///
  /// Returns `None` normally, or `Some(OpKind)` when the main loop
  /// needs to perform a privileged operation (like suspending the
  /// terminal for sudo).
  pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<OpKind> {
    use crossterm::event::KeyCode;

    // Ctrl+C always quits, regardless of mode.
    if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
      && key.code == KeyCode::Char('c')
    {
      self.should_quit = true;
      return None;
    }

    match &self.mode {
      Mode::Message { .. } => {
        // Any key dismisses the message overlay.
        let refresh = if let Mode::Message { refresh_on_close, .. } = &self.mode {
          *refresh_on_close
        } else {
          false
        };
        self.mode = Mode::Normal;
        if refresh {
          self.refresh_mounts();
        }
        None
      }

      Mode::Toast { .. } => {
        // Any key dismisses the toast early.
        self.mode = Mode::Normal;
        None
      }

      Mode::Help => {
        // Any key dismisses help.
        self.mode = Mode::Normal;
        None
      }

      Mode::PasswordPrompt { .. } => {
        self.handle_password_key(key);
        None
      }

      Mode::Normal => self.handle_normal_key(key),

      Mode::NewMount { .. } | Mode::MountSelected { .. } => {
        self.handle_dialog_key(key)
      }
    }
  }

  /// Handle keys in the main two-pane mode.
  ///
  /// We check `detail_focused` FIRST — when the detail pane has focus,
  /// arrow keys scroll its content.  When it doesn't, they navigate
  /// the mount list.  The detail-scroll check must come before the
  /// list-navigation arms in control flow; we do it with an early
  /// `if self.detail_focused` block that returns, then fall through
  /// to the list navigation.
  fn handle_normal_key(&mut self, key: crossterm::event::KeyEvent) -> Option<OpKind> {
    use crossterm::event::KeyCode;

    // ── detail-pane scrolling (priority when focused) ────────
    if self.detail_focused {
      match key.code {
        KeyCode::Up       => { self.detail_scroll = self.detail_scroll.saturating_sub(1); return None; }
        KeyCode::Down     => { self.detail_scroll = self.detail_scroll.saturating_add(1); return None; }
        KeyCode::PageUp   => { self.detail_scroll = self.detail_scroll.saturating_sub(10); return None; }
        KeyCode::PageDown => { self.detail_scroll = self.detail_scroll.saturating_add(10); return None; }
        KeyCode::Char('k') | KeyCode::Char('j') => { return None; } // vim keys ignore when detail has focus
        _ => {} // fall through to shared keybindings below
      }
    }

    match key.code {
      // ── navigation (list has focus) ──────────────────────────
      KeyCode::Up | KeyCode::Char('k') => {
        self.selected = self.selected.saturating_sub(1);
        self.clamp_scroll();
      }
      KeyCode::Down | KeyCode::Char('j') => {
        if !self.mounts.is_empty() {
          self.selected = (self.selected + 1).min(self.mounts.len() - 1);
        }
        self.clamp_scroll();
      }
      KeyCode::PageUp => {
        let page = 10usize;
        self.selected = self.selected.saturating_sub(page);
        self.clamp_scroll();
      }
      KeyCode::PageDown => {
        let page = 10usize;
        if !self.mounts.is_empty() {
          self.selected = (self.selected + page).min(self.mounts.len() - 1);
        }
        self.clamp_scroll();
      }
      KeyCode::Home => {
        self.selected = 0;
        self.scroll_offset = 0;
      }
      KeyCode::End => {
        if !self.mounts.is_empty() {
          self.selected = self.mounts.len() - 1;
        }
        self.clamp_scroll();
      }

      // ── pane focus toggle ────────────────────────────────────
      KeyCode::Tab => {
        self.detail_focused = !self.detail_focused;
        self.detail_scroll = 0;
      }

      // ── actions ───────────────────────────────────────────────
      KeyCode::Char('m') => {
        // "Mount" — open the mount dialog.
        // If we have a selected entry, pre-fill the device.
        let device = self
          .mounts
          .get(self.selected)
          .map(|m| m.device.clone())
          .unwrap_or_default();
        self.mode = Mode::MountSelected {
          device,
          mountpoint: String::new(),
          fs_type_idx: 0,
          options: String::new(),
          active_field: 1,  // start on mountpoint field
        };
      }

      KeyCode::Char('u') => {
        // Unmount the selected mount point.
        return self.do_unmount(false);
      }

      KeyCode::Char('f') => {
        // Force-unmount the selected mount point.
        return self.do_unmount(true);
      }

      KeyCode::Char('n') => {
        // "New" — open blank mount dialog.
        self.mode = Mode::NewMount {
          device: String::new(),
          mountpoint: String::new(),
          fs_type_idx: 0,
          options: String::new(),
          active_field: 0,
        };
      }

      KeyCode::Char('r') => {
        self.refresh_mounts();
        self.set_status("Mount list refreshed.");
      }

      KeyCode::Char('y') => {
        // "yank" (vim-speak) — copy the mount point path to clipboard.
        // (The path is the harder one to type; the device is usually short.)
        if let Some(m) = self.mounts.get(self.selected) {
          let text = &m.mount_point;
          if crate::clipboard::copy(text) {
            self.toast(&format!("📋 Copied: {}", text));
          } else {
            self.set_status("Clipboard: no tool found (install xclip / wl-clipboard on Linux)");
          }
        }
      }

      KeyCode::Char('Y') => {
        // Shift-Y — copy the device path.
        if let Some(m) = self.mounts.get(self.selected) {
          let text = &m.device;
          if crate::clipboard::copy(text) {
            self.toast(&format!("📋 Copied: {}", text));
          } else {
            self.set_status("Clipboard: no tool found (install xclip / wl-clipboard on Linux)");
          }
        }
      }

      KeyCode::Char('?') => {
        self.mode = Mode::Help;
      }

      KeyCode::Esc | KeyCode::Char('q') => {
        self.should_quit = true;
      }

      _ => {}
    }

    None   // no privileged operation needed
  }

  /// Handle keys while a dialog is open.
  fn handle_dialog_key(&mut self, key: crossterm::event::KeyEvent) -> Option<OpKind> {
    use crossterm::event::KeyCode;

    match key.code {
      KeyCode::Esc => {
        self.mode = Mode::Normal;
      }

      KeyCode::Tab => {
        // Rotate through fields: 0 → 1 → 2 → 3 → 0
        let active = match &mut self.mode {
          Mode::NewMount { active_field, .. }
          | Mode::MountSelected { active_field, .. } => active_field,
          _ => return None,
        };
        *active = (*active + 1) % 4;
      }

      // BackTab (Shift+Tab) — rotate backwards
      KeyCode::BackTab => {
        let active = match &mut self.mode {
          Mode::NewMount { active_field, .. }
          | Mode::MountSelected { active_field, .. } => active_field,
          _ => return None,
        };
        *active = (*active + 3) % 4;  // +3 ≡ -1 mod 4
      }

      KeyCode::Enter => {
        // Execute the mount operation.
        return self.do_mount_from_dialog();
      }

      // Text input for the active field
      KeyCode::Char(c) => {
        self.dialog_type_char(c);
      }

      KeyCode::Backspace => {
        self.dialog_backspace();
      }

      // Navigate fs_type dropdown
      KeyCode::Left | KeyCode::Right => {
        self.dialog_adjust_fs_type(key.code);
      }

      _ => {}
    }

    None
  }

  // ── dialog field manipulation ──────────────────────────────────

  /// Type a character into the active dialog field.
  fn dialog_type_char(&mut self, c: char) {
    match &mut self.mode {
      Mode::NewMount {
        device,
        mountpoint,
        options,
        active_field,
        ..
      }
      | Mode::MountSelected {
        device,
        mountpoint,
        options,
        active_field,
        ..
      } => match *active_field {
        0 => device.push(c),
        1 => mountpoint.push(c),
        3 => options.push(c),
        _ => {}  // fs_type is a dropdown, not text
      },
      _ => {}
    }
  }

  /// Backspace in the active dialog field.
  fn dialog_backspace(&mut self) {
    match &mut self.mode {
      Mode::NewMount {
        device,
        mountpoint,
        options,
        active_field,
        ..
      }
      | Mode::MountSelected {
        device,
        mountpoint,
        options,
        active_field,
        ..
      } => match *active_field {
        0 => { device.pop(); }
        1 => { mountpoint.pop(); }
        3 => { options.pop(); }
        _ => {}
      },
      _ => {}
    }
  }

  /// Adjust fs_type dropdown index with Left/Right arrows.
  fn dialog_adjust_fs_type(&mut self, key: crossterm::event::KeyCode) {
    let idx = match &mut self.mode {
      Mode::NewMount { fs_type_idx, .. }
      | Mode::MountSelected { fs_type_idx, .. } => fs_type_idx,
      _ => return,
    };
    let max = mount_ops::FS_TYPES.len().saturating_sub(1);
    match key {
      crossterm::event::KeyCode::Left  => *idx = idx.saturating_sub(1),
      crossterm::event::KeyCode::Right => *idx = (*idx + 1).min(max),
      _ => {}
    }
  }

  // ── mount / unmount actions ────────────────────────────────────

  /// Unmount the currently-selected entry.
  fn do_unmount(&mut self, force: bool) -> Option<OpKind> {
    if let Some(entry) = self.mounts.get(self.selected) {
      let mp = entry.mount_point.clone();
      // Return an OpKind so main.rs can suspend the terminal before sudo.
      Some(OpKind::Unmount {
        mountpoint: mp,
        force,
      })
    } else {
      self.mode = Mode::Message {
        title: "Error".into(),
        body: "No mount point selected.".into(),
        refresh_on_close: false,
      };
      None
    }
  }

  /// Execute the mount operation from the dialog form.
  fn do_mount_from_dialog(&mut self) -> Option<OpKind> {
    let (device, mountpoint, fs_type, options) = match &self.mode {
      Mode::NewMount {
        device,
        mountpoint,
        fs_type_idx,
        options,
        ..
      }
      | Mode::MountSelected {
        device,
        mountpoint,
        fs_type_idx,
        options,
        ..
      } => (
        device.clone(),
        mountpoint.clone(),
        mount_ops::FS_TYPES[*fs_type_idx].to_string(),
        options.clone(),
      ),
      _ => return None,
    };

    if device.is_empty() || mountpoint.is_empty() {
      self.mode = Mode::Message {
        title: "Error".into(),
        body: "Device and mountpoint are required.".into(),
        refresh_on_close: false,
      };
      return None;
    }

    Some(OpKind::Mount {
      device,
      mountpoint,
      fs_type,
      options,
    })
  }

  /// Called by main.rs after a privileged operation completes.
  /// Updates the UI with the result.
  pub fn on_op_result(&mut self, result: &OpResult) {
    self.mode = Mode::Message {
      title: if result.success { "Success".into() } else { "Error".into() },
      body: result.message.clone(),
      refresh_on_close: result.success,
    };
  }

  // ── password prompt handling ──────────────────────────────────

  /// Handle keystrokes while the sudo password prompt is visible.
  /// Printable chars are appended to the password buffer; backspace
  /// deletes the last char; Enter submits the operation.
  fn handle_password_key(&mut self, key: crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode;

    if let Mode::PasswordPrompt { ref mut password, ref operation } = self.mode {
      match key.code {
        KeyCode::Esc => {
          self.mode = Mode::Normal;
        }
        KeyCode::Enter => {
          // Move operation + password to the field main.rs watches.
          self.pending_privileged = Some((operation.clone(), password.clone()));
          self.mode = Mode::Normal;
        }
        KeyCode::Backspace => {
          password.pop();
        }
        KeyCode::Char(c) => {
          password.push(c);
        }
        _ => {}
      }
    }
  }

  // ── scroll helpers ─────────────────────────────────────────────

  /// Ensure the selected item is visible in the list viewport.
  fn clamp_scroll(&mut self) {
    // The list pane height will be determined at render time.
    // We store a "desired" scroll offset and clamp it during rendering.
    if self.selected < self.scroll_offset {
      self.scroll_offset = self.selected;
    }
  }

  // ═════════════════════════════════════════════════════════════════
  //  RENDERING
  // ═════════════════════════════════════════════════════════════════

  /// Draw the entire UI into the terminal frame.
  ///
  /// `Frame<'_>` is a type with a lifetime parameter — it borrows the
  /// terminal backend for the duration of the render call.  The `'_`
  /// means "elided lifetime" (the compiler infers it).
  pub fn render(&mut self, f: &mut Frame) {
    // Fill the entire screen with a dark background.
    let area = f.size();
    let bg = Block::new().style(Style::new().bg(Color::Rgb(15, 15, 22)));
    f.render_widget(bg, area);

    // Main vertical split: header + body + footer
    let main_layout = Layout::default()
      .direction(Direction::Vertical)
      .constraints([
        Constraint::Length(1),   // title bar
        Constraint::Min(1),      // body (fills remaining space)
        Constraint::Length(1),   // status/help bar
      ])
      .split(area);

    self.render_title_bar(f, main_layout[0]);
    self.render_body(f, main_layout[1]);
    self.render_status_bar(f, main_layout[2]);

    // Modal dialogs render ON TOP of the body.
    match &self.mode {
      Mode::NewMount { .. } | Mode::MountSelected { .. } => {
        self.render_mount_dialog(f, area);
      }
      Mode::Message { title, body, .. } => {
        self.render_message_dialog(f, area, title, body);
      }
      Mode::Toast { text, .. } => {
        self.render_toast(f, area, text);
      }
      Mode::Help => {
        self.render_help(f, area);
      }
      Mode::PasswordPrompt { password, .. } => {
        self.render_password_prompt(f, area, password);
      }
      Mode::Normal => {}  // no overlay
    }
  }

  // ── title bar ─────────────────────────────────────────────────

  fn render_title_bar(&self, f: &mut Frame, area: Rect) {
    // Gradient-style title bar: deep indigo background.
    let bar_bg = Color::Rgb(45, 40, 110);
    let mut title = Line::from(vec![
      Span::styled(" ⛰ FancyMount ", Style::default()
        .fg(Color::Rgb(255, 255, 255))
        .bg(bar_bg)
        .add_modifier(Modifier::BOLD)),
      Span::styled(
        format!(" v{}  ", env!("CARGO_PKG_VERSION")),
        Style::default().fg(Color::Rgb(160, 160, 200)).bg(bar_bg),
      ),
      Span::styled(
        format!(" {} mounts ", self.mounts.len()),
        Style::default()
          .fg(Color::Rgb(190, 190, 220))
          .bg(Color::Rgb(30, 28, 60)),
      ),
      Span::raw(" "),
      Span::styled(
        std::env::consts::OS,
        Style::default().fg(Color::Rgb(150, 150, 190)),
      ),
    ]);
    // Show unlock icon when sudo credentials are cached.
    if self.cached_password.is_some() {
      title.push_span(Span::raw(" "));
      title.push_span(Span::styled(
        "🔓",
        Style::default().fg(Color::Rgb(100, 220, 140)),
      ));
    }
    let block = Block::new()
      .style(Style::new().bg(bar_bg));
    f.render_widget(block, area);
    f.render_widget(title, area);
  }

  // ── body: two-pane layout ─────────────────────────────────────

  fn render_body(&mut self, f: &mut Frame, area: Rect) {
    let panes = Layout::default()
      .direction(Direction::Horizontal)
      .constraints([
        Constraint::Percentage(63),  // mount list (left) — more room for columnar layout
        Constraint::Percentage(37),  // detail pane (right)
      ])
      .split(area);

    self.render_mount_list(f, panes[0]);
    self.render_detail_pane(f, panes[1]);
  }

  // ── mount list (left pane) ────────────────────────────────────

  fn render_mount_list(&mut self, f: &mut Frame, area: Rect) {
    let list_height = area.height.saturating_sub(2) as usize; // minus borders

    // Clamp scroll: keep selected in view.
    if self.selected < self.scroll_offset {
      self.scroll_offset = self.selected;
    }
    if self.selected >= self.scroll_offset.saturating_add(list_height) {
      self.scroll_offset = self.selected.saturating_sub(list_height.saturating_sub(1));
    }
    if self.scroll_offset + list_height > self.mounts.len() && !self.mounts.is_empty() {
      self.scroll_offset = self.mounts.len().saturating_sub(list_height);
    }

    // ── Compute the widest device basename for column alignment ──
    // We scan ALL mounts (not just visible ones) so the device column
    // stays fixed-width and the mount-point column always starts at
    // the same horizontal position — no jitter when scrolling.
    let max_dev_w = self.mounts.iter()
      .map(|m| basename(&m.device).len())
      .max()
      .unwrap_or(6)
      .max(6);  // floor: at least wide enough for "device"

    // Build list items from the visible window.
    let visible_end = (self.scroll_offset + list_height).min(self.mounts.len());
    let items: Vec<ListItem> = self.mounts[self.scroll_offset..visible_end]
      .iter()
      .enumerate()
      .map(|(i, m)| {
        let global_idx = self.scroll_offset + i;
        let is_selected = global_idx == self.selected;

        // Build the row as styled spans, column by column.
        let mut spans: Vec<Span> = Vec::with_capacity(8);

        // No index column — the row number lives in the detail pane.

        // ── Column 1: filesystem type (7 chars wide) ──────────
        // Vibrant true-color palette — ratatui degrades gracefully
        // to 256-colour and 16-colour terminals via quantisation.
        let fs_color = match m.fs_type.as_str() {
          "apfs"   => Color::Rgb(100, 210, 255),
          "hfs"    => Color::Rgb(200, 170, 255),
          "ext4"   => Color::Rgb(80, 230, 140),
          "ext3"   => Color::Rgb(60, 210, 120),
          "ext2"   => Color::Rgb(40, 190, 100),
          "xfs"    => Color::Rgb(240, 190, 80),
          "btrfs"  => Color::Rgb(100, 230, 210),
          "zfs"    => Color::Rgb(210, 150, 240),
          "ntfs"   => Color::Rgb(80, 190, 235),
          "exfat"  => Color::Rgb(220, 210, 80),
          "fat32"  => Color::Rgb(210, 190, 90),
          "nfs" | "nfs4" => Color::Rgb(255, 150, 60),
          "smbfs" | "cifs" => Color::Rgb(255, 120, 80),
          "tmpfs"  => Color::Rgb(140, 210, 150),
          "iso9660"=> Color::Rgb(200, 180, 140),
          _        => Color::Rgb(190, 190, 210),
        };

        spans.push(Span::styled(
          format!("{:<7}", m.fs_type),
          Style::default().fg(fs_color),
        ));

        // ── Column 2: usage bar + percentage ──────────────────
        // Bar is exactly 10 chars: ▇ = used (lower 7/8 block),
        // ▒ = free (medium shade).  Same width for every entry
        // so you can scan down the column and compare instantly.
        if let Some(frac) = m.usage_fraction() {
          let (used_str, free_str) = make_mini_bar(frac, BAR_W);
          let pct_str = format!(" {:3.0}% ", frac * 100.0);
          spans.push(Span::styled(
            used_str,
            Style::default().fg(usage_color(frac)),
          ));
          spans.push(Span::styled(
            free_str,
            Style::default().fg(Color::Rgb(80, 80, 90)),
          ));
          spans.push(Span::styled(
            pct_str,
            Style::default().fg(Color::Rgb(175, 175, 195)),
          ));
        } else {
          // Pseudo-fs with no usage data: dim placeholder of uniform ▒ chars.
          spans.push(Span::styled(
            format!("{} {:>4}", "▒".repeat(BAR_W), "-"),
            Style::default().fg(Color::Rgb(65, 65, 75)),
          ));
        }

        // ── Column 3: device (padded)  mountpoint ─────────────
        // The device column is padded to the width of the longest
        // device basename so all mount points start at the same
        // horizontal position.  No arrow glyph needed — the space
        // separation is clear enough.
        let device_short = basename(&m.device);
        let mount_name  = basename(&m.mount_point);
        spans.push(Span::styled(
          format!("{:<max_dev_w$} ", device_short, max_dev_w = max_dev_w),
          Style::default().fg(Color::Rgb(180, 180, 200)),
        ));
        spans.push(Span::styled(
          mount_name,  // owned String → Cow::Owned, no borrow
          Style::default().fg(Color::Rgb(245, 245, 255)),
        ));

        let style = if is_selected && !self.detail_focused {
          Style::default()
            .bg(Color::Rgb(50, 45, 95))
            .add_modifier(Modifier::BOLD)
        } else if is_selected {
          Style::default().bg(Color::Rgb(32, 30, 55))
        } else {
          Style::default()
        };

        ListItem::new(Line::from(spans)).style(style)
      })
      .collect();

    let list = List::new(items)
      .block(
        Block::new()
          .borders(Borders::ALL)
          .border_type(BorderType::Rounded)
          .border_style(if !self.detail_focused {
        // Focused border: soft lavender-cyan glow.
        Style::default().fg(Color::Rgb(110, 150, 240))
      } else {
        // Unfocused border: muted but still visible.
        Style::default().fg(Color::Rgb(65, 65, 80))
      })
          .title(format!(
            " Mount Points ({}) ",
            self.mounts.len()
          ))
          .title_alignment(Alignment::Left),
      )
      .highlight_style(Style::default())
      .highlight_symbol(""); // we handle highlighting ourselves

    f.render_widget(list, area);
  }

  // ── detail pane (right) ───────────────────────────────────────

  fn render_detail_pane(&self, f: &mut Frame, area: Rect) {
    let entry = self.mounts.get(self.selected);

    let lines = if let Some(m) = entry {
      build_detail_lines(m, self.selected + 1, self.mounts.len())
    } else {
      vec![Line::from("No mount points found.")]
    };

    // Apply scroll offset when detail pane is focused.
    let visible_lines: Vec<Line> = if self.detail_focused {
      let skip = self.detail_scroll.min(lines.len().saturating_sub(1));
      lines.into_iter().skip(skip).collect()
    } else {
      lines
    };

    let para = Paragraph::new(visible_lines)
      .block(
        Block::new()
          .borders(Borders::ALL)
          .border_type(BorderType::Rounded)
          .border_style(if self.detail_focused {
        // Focused border:
        Style::default().fg(Color::Rgb(110, 150, 240))
      } else {
        // Unfocused:
        Style::default().fg(Color::Rgb(65, 65, 80))
      })
          .title(if let Some(m) = entry {
            format!(" {} ", m.mount_point)
          } else {
            " Details ".into()
          })
          .title_alignment(Alignment::Left),
      )
      .wrap(Wrap { trim: false });

    f.render_widget(para, area);
  }

  // ── status bar ────────────────────────────────────────────────

  fn render_status_bar(&self, f: &mut Frame, area: Rect) {
    // Reduce tick counter each render (~10/sec at 100ms frame rate).
    // This is a simplified approach — see `tick()` below.

    let help = if !self.status_msg.is_empty() {
      Span::styled(
        &self.status_msg,
        Style::default()
          .fg(Color::Yellow)
          .add_modifier(Modifier::BOLD),
      )
    } else {
      Span::styled(
        " q:Quit  ↑↓/j,k:Nav  Tab:Switch Pane  m:Mount  u:Unmount  f:Force  n:New  r:Refresh  y:Copy path  Y:Copy dev  ^L:Lock  ?:Help ",
        Style::default().fg(Color::Rgb(130, 130, 155)),
      )
    };

    let block = Block::new()
      .style(Style::new().bg(Color::Rgb(22, 22, 34)));
    f.render_widget(block, area);
    f.render_widget(help, area);
  }

  /// Called once per frame to decrement counters.
  pub fn tick(&mut self) {
    // Status bar message countdown.
    if self.status_ticks > 0 {
      self.status_ticks -= 1;
      if self.status_ticks == 0 {
        self.status_msg.clear();
      }
    }
    // Toast auto-dismiss.
    if let Mode::Toast { ticks, .. } = &mut self.mode {
      if *ticks > 0 {
        *ticks -= 1;
      }
      if *ticks == 0 {
        self.mode = Mode::Normal;
      }
    }
  }

  // ── dialogs ────────────────────────────────────────────────────

  /// Render the "New Mount" / "Mount Selected" dialog as a centered overlay.
  fn render_mount_dialog(&self, f: &mut Frame, parent_area: Rect) {
    let (device, mountpoint, fs_type_idx, options, active_field) = match &self.mode {
      Mode::NewMount {
        device,
        mountpoint,
        fs_type_idx,
        options,
        active_field,
      } => (device.clone(), mountpoint.clone(), *fs_type_idx, options.clone(), *active_field),
      Mode::MountSelected {
        device,
        mountpoint,
        fs_type_idx,
        options,
        active_field,
      } => (device.clone(), mountpoint.clone(), *fs_type_idx, options.clone(), *active_field),
      _ => return,
    };

    // Center the dialog at ~60% width.
    let dialog_area = centered_rect(60, 55, parent_area);

    // Clear behind the dialog.
    f.render_widget(Clear, dialog_area);

    let fs_type_name = mount_ops::FS_TYPES.get(fs_type_idx).unwrap_or(&"auto");

    let field_style = |idx: u8| -> Style {
      if active_field == idx {
        Style::default()
          .fg(Color::Yellow)
          .bg(Color::Rgb(50, 50, 60))
          .add_modifier(Modifier::BOLD)
      } else {
        Style::default().fg(Color::Rgb(200, 200, 220))
      }
    };

    let lines = vec![
      Line::from(Span::styled(" Mount Filesystem", Style::default()
        .fg(Color::White).add_modifier(Modifier::BOLD))),
      Line::from(""),
      Line::from(vec![
        Span::styled(" Device:      ", field_style(0)),
        Span::styled(&device, field_style(0)),
        Span::styled(if device.is_empty() && active_field == 0 {
          "█"  // cursor indicator
        } else {
          ""
        }, Style::default().fg(Color::Yellow)),
      ]),
      Line::from(vec![
        Span::styled(" Mount Point: ", field_style(1)),
        Span::styled(&mountpoint, field_style(1)),
        Span::styled(if mountpoint.is_empty() && active_field == 1 {
          "█"
        } else {
          ""
        }, Style::default().fg(Color::Yellow)),
      ]),
      Line::from(vec![
        Span::styled(" Filesystem:  ", field_style(2)),
        Span::styled(
          format!("◀ {} ▶", fs_type_name),
          field_style(2),
        ),
        Span::raw("  ← → to change"),
      ]),
      Line::from(vec![
        Span::styled(" Options:     ", field_style(3)),
        Span::styled(&options, field_style(3)),
        Span::styled(if options.is_empty() && active_field == 3 {
          "█"
        } else {
          ""
        }, Style::default().fg(Color::Yellow)),
      ]),
      Line::from(""),
      Line::from(Span::styled(
        " Enter:Mount  Tab:Next Field  Esc:Cancel",
        Style::default().fg(Color::Rgb(140, 140, 160)),
      )),
    ];

    let block = Block::new()
      .borders(Borders::ALL)
      .border_type(BorderType::Rounded)
      .border_style(Style::default().fg(Color::Rgb(100, 150, 240)))
      .style(Style::default().bg(Color::Rgb(20, 22, 38)));

    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, dialog_area);
  }

  /// Render a message/error overlay.
  fn render_message_dialog(&self, f: &mut Frame, parent_area: Rect, title: &str, body: &str) {
    // Wider dialog with wrapping for long error messages.
    let dialog_area = centered_rect(65, 40, parent_area);
    f.render_widget(Clear, dialog_area);

    let color = if title == "Success" {
      Color::Rgb(80, 220, 140)
    } else {
      Color::Rgb(255, 110, 110)
    };

    // Build a paragraph that wraps long lines and has internal
    // padding so text doesn't bump against the border.
    let text = vec![
      Line::from(Span::styled(
        format!(" {} ", title),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
      )),
      Line::from(""),
      Line::from(Span::styled(body, Style::default().fg(Color::Rgb(235, 235, 245)))),
      Line::from(""),
      Line::from(Span::styled(
        " Press any key to dismiss",
        Style::default().fg(Color::Rgb(140, 140, 165)),
      )),
    ];

    let block = Block::new()
      .borders(Borders::ALL)
      .border_type(BorderType::Rounded)
      .border_style(Style::default().fg(color))
      .style(Style::default().bg(Color::Rgb(20, 20, 35)))
      .padding(Padding::horizontal(2));

    let para = Paragraph::new(text)
      .block(block)
      .wrap(Wrap { trim: true })
      .alignment(Alignment::Center);
    f.render_widget(para, dialog_area);
  }

  /// Render a small toast notification anchored near the bottom of the
  /// screen.  Used for clipboard confirmations — more visible than the
  /// status bar but less intrusive than a modal.
  fn render_toast(&self, f: &mut Frame, parent_area: Rect, text: &str) {
    // Toast is 50% wide, 3 lines tall, anchored at the bottom.
    let w = (parent_area.width * 50 / 100).min(parent_area.width).max(20);
    let h = 3u16;
    let x = parent_area.x + (parent_area.width.saturating_sub(w)) / 2;
    let y = parent_area.y + parent_area.height.saturating_sub(h + 1);
    let area = Rect::new(x, y, w, h.min(parent_area.height.saturating_sub(y)));

    f.render_widget(Clear, area);

    let line = Line::from(Span::styled(
      text,
      Style::default()
        .fg(Color::Rgb(200, 240, 200))
        .add_modifier(Modifier::BOLD),
    ));

    let block = Block::new()
      .borders(Borders::ALL)
      .border_type(BorderType::Rounded)
      .border_style(Style::default().fg(Color::Rgb(80, 200, 120)))
      .style(Style::default().bg(Color::Rgb(20, 40, 25)));

    let para = Paragraph::new(line).block(block).alignment(Alignment::Center);
    f.render_widget(para, area);
  }

  /// Render the sudo password prompt as a centered modal.
  /// The password is displayed as `••••` for privacy.
  fn render_password_prompt(&self, f: &mut Frame, parent_area: Rect, password: &str) {
    let dialog_area = centered_rect(50, 30, parent_area);
    f.render_widget(Clear, dialog_area);

    let masked: String = password.chars().map(|_| '•').collect();
    let cursor = if masked.is_empty() { "█" } else { "" };

    let lines = vec![
      Line::from(Span::styled(
        " 🔐 Sudo Password",
        Style::default().fg(Color::Rgb(255, 200, 80)).add_modifier(Modifier::BOLD),
      )),
      Line::from(""),
      Line::from(Span::styled(
        " Enter your password to authorise:",
        Style::default().fg(Color::Rgb(200, 200, 220)),
      )),
      Line::from(""),
      Line::from(vec![
        Span::raw("  "),
        Span::styled(
          format!("{}{}", masked, cursor),
          Style::default()
            .fg(Color::Rgb(255, 220, 100))
            .bg(Color::Rgb(40, 38, 25))
            .add_modifier(Modifier::BOLD),
        ),
      ]),
      Line::from(""),
      Line::from(Span::styled(
        " Enter:Submit  Esc:Cancel",
        Style::default().fg(Color::Rgb(150, 150, 170)),
      )),
    ];

    let block = Block::new()
      .borders(Borders::ALL)
      .border_type(BorderType::Rounded)
      .border_style(Style::default().fg(Color::Rgb(220, 170, 40)))
      .style(Style::default().bg(Color::Rgb(25, 25, 38)));

    let para = Paragraph::new(lines).block(block).alignment(Alignment::Center);
    f.render_widget(para, dialog_area);
  }

  /// Render help screen overlay.
  fn render_help(&self, f: &mut Frame, parent_area: Rect) {
    let dialog_area = centered_rect(70, 70, parent_area);
    f.render_widget(Clear, dialog_area);

    let lines = vec![
      Line::from(Span::styled(" FancyMount Help", Style::default()
        .fg(Color::White).add_modifier(Modifier::BOLD))),
      Line::from(""),
      Line::from(Span::styled(" Navigation", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
      Line::from("  ↑/↓  or  j/k      Move selection up/down"),
      Line::from("  PgUp / PgDn        Jump by 10 entries"),
      Line::from("  Home / End         First / last entry"),
      Line::from("  Tab                Switch focus (list ↔ detail)"),
      Line::from(""),
      Line::from(Span::styled(" Actions", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
      Line::from("  m                  Mount — open mount dialog"),
      Line::from("  u                  Unmount selected mount point"),
      Line::from("  f                  Force-unmount (lazy on Linux)"),
      Line::from("  n                  New mount — blank dialog"),
      Line::from("  y                  Copy mount point path to clipboard"),
      Line::from("  Y                  Copy device path to clipboard"),
      Line::from("  r                  Refresh mount list"),
      Line::from(""),
      Line::from(Span::styled(" Dialogs", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
      Line::from("  Tab / Shift+Tab    Next/previous field"),
      Line::from("  ← →               Change filesystem type"),
      Line::from("  Enter              Execute action"),
      Line::from("  Esc                Cancel / close"),
      Line::from(""),
      Line::from(Span::styled(" General", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
      Line::from("  q  or  Esc         Quit"),
      Line::from("  Ctrl+C             Quit (force)"),
      Line::from("  Ctrl+L             Clear cached sudo password"),
      Line::from("  ?                  This help screen"),
      Line::from(""),
      Line::from(Span::styled(
        " Press any key to close help",
        Style::default().fg(Color::Rgb(140, 140, 160)),
      )),
    ];

    let block = Block::new()
      .borders(Borders::ALL)
      .border_type(BorderType::Rounded)
      .border_style(Style::default().fg(Color::Rgb(100, 150, 240)))
      .style(Style::default().bg(Color::Rgb(20, 22, 38)));

    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, dialog_area);
  }
}

// ═══════════════════════════════════════════════════════════════════════
//  OPERATION KINDS (for main.rs terminal suspend)
// ═══════════════════════════════════════════════════════════════════════

/// An operation that requires temporarily leaving raw terminal mode
/// so the user can interact with sudo's password prompt.
#[derive(Debug, Clone, PartialEq)]
pub enum OpKind {
  Mount {
    device: String,
    mountpoint: String,
    fs_type: String,
    options: String,
  },
  Unmount {
    mountpoint: String,
    force: bool,
  },
}

/// Execute a privileged operation.  Call this AFTER exiting raw mode
/// and BEFORE re-entering it.
pub fn execute_privileged_op(op: OpKind, password: &str) -> OpResult {
  match op {
    OpKind::Mount {
      device,
      mountpoint,
      fs_type,
      options,
    } => mount_ops::mount_device(&device, &mountpoint, &fs_type, &options, password),
    OpKind::Unmount { mountpoint, force } => {
      if force {
        mount_ops::force_unmount(&mountpoint, password)
      } else {
        mount_ops::unmount_device(&mountpoint, password)
      }
    }
  }
}

// ═══════════════════════════════════════════════════════════════════════
//  RENDERING HELPERS
// ═══════════════════════════════════════════════════════════════════════

/// Build the detail text lines for a single MountEntry.
/// Returns owned `Line`s so callers can hang onto them.
fn build_detail_lines(m: &MountEntry, index: usize, total: usize) -> Vec<Line<'static>> {
  let mut lines = Vec::new();  // type inferred as Vec<Line<'static>>

  // Helper: section header (owned String → 'static)
  let section = |s: &str| -> Line<'static> {
    let title = format!(" {} ", s);
    Line::from(Span::styled(
      title,
      Style::default()
        .fg(Color::Rgb(90, 180, 220))
        .add_modifier(Modifier::BOLD),
    ))
  };

  // Helper: key-value pair.
  let kv = |k: &str, v: &str| -> Line<'static> {
    let key = format!("  {:<14}", k);
    let val = v.to_string();
    Line::from(vec![
      Span::styled(key, Style::default().fg(Color::Rgb(130, 135, 170))),
      Span::styled(val, Style::default().fg(Color::Rgb(225, 225, 245))),
    ])
  };

  lines.push(section("Identity"));
  lines.push(kv("#", &format!("{} of {}", index, total)));
  lines.push(kv("Device:", m.device.as_str()));
  lines.push(kv("Mount Point:", m.mount_point.as_str()));
  lines.push(kv("Filesystem:", m.fs_type.as_str()));
  lines.push(Line::from(""));

  lines.push(section("Options"));
  for opt in m.options.split(',') {
    let opt = opt.trim();
    if opt.is_empty() {
      continue;
    }
    lines.push(Line::from(vec![
      Span::raw("  • "),
      Span::styled(opt.to_string(), Style::default().fg(Color::Rgb(195, 195, 220))),
    ]));
  }
  lines.push(Line::from(""));

  // Usage section
  if let (Some(total), Some(avail), Some(used)) = (
    m.total_bytes,
    m.avail_bytes,
    m.used_bytes,
  ) {
    lines.push(section("Usage"));
    let total_str = mount_ops::format_bytes(total);
    let used_str = mount_ops::format_bytes(used);
    let avail_str = mount_ops::format_bytes(avail);
    lines.push(kv("Total:", &total_str));
    lines.push(kv("Used:", &used_str));
    lines.push(kv("Available:", &avail_str));

    if let Some(frac) = m.usage_fraction() {
      let (used_str, free_str) = make_big_bar(frac, 30);
      let pct = format!("{:.1}%", frac * 100.0);
      lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(used_str, Style::default().fg(usage_color(frac))),
        Span::styled(free_str, Style::default().fg(Color::Rgb(60, 60, 75))),
        Span::raw(" "),
        Span::styled(pct, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
      ]));
    }
  }

  lines
}

/// How many chars wide the inline usage bar is in the mount list.
/// Every bar is exactly this width so used/free ratios are
/// comparable at a glance across all entries.
const BAR_W: usize = 10;

/// Build a mini bar for the mount list.
/// Returns `(used_str, free_str)` so the caller can style each
/// portion independently.
///
/// Used chars: `▇` (U+2587, lower 7/8 block).
/// Free chars: `▒` (U+2592, medium shade 50%) — the visible track.
fn make_mini_bar(fraction: f64, width: usize) -> (String, String) {
  make_dual_bar(fraction, width)
}

/// Build a wider bar for the detail pane.  Same semantics.
fn make_big_bar(fraction: f64, width: usize) -> (String, String) {
  make_dual_bar(fraction, width)
}

/// Core bar builder.  Every bar has exactly `width` characters.
///
/// The bar is a *track* of `▒` characters with a *fill* of `▇`
/// characters painted up to `fraction` of the width.  No partial
/// characters — we round to the nearest whole slot for a clean,
/// consistent look across all entries.
///
/// Returns two Strings whose total length equals `width`.
fn make_dual_bar(fraction: f64, width: usize) -> (String, String) {
  if width == 0 {
    return (String::new(), String::new());
  }

  let frac = fraction.clamp(0.0, 1.0);
  // Round to nearest whole character.  `as usize` truncates;
  // adding 0.5 before the cast gives us proper rounding.
  let used_slots = ((frac * width as f64) + 0.5) as usize;
  let used_slots = used_slots.min(width);
  let free_slots = width - used_slots;

  let used = "▇".repeat(used_slots);
  let free = "▒".repeat(free_slots);

  (used, free)
}

/// Colour gradient for the usage bar — smooth ramp through
/// perceptually-balanced stops.  Uses true-colour RGB; ratatui
/// quantises automatically to 256- or 16-colour when the terminal
/// doesn't support 24-bit.
fn usage_color(fraction: f64) -> Color {
  if fraction < 0.5 {
    // Emerald green: plenty of space.
    Color::Rgb(60, 210, 110)
  } else if fraction < 0.75 {
    // Amber: getting fuller.
    Color::Rgb(250, 200, 40)
  } else if fraction < 0.90 {
    // Deep orange: almost full.
    Color::Rgb(255, 130, 50)
  } else {
    // Crimson: critically full.
    Color::Rgb(255, 70, 70)
  }
}

/// Strip a path down to its final component (like `basename(1)`).
/// * `"/Volumes/Data"` → `"Data"`
/// * `"/"` → `"/"`
/// * `"//server/share"` → `"share"`
///
/// Uses `std::path::Path` so the separator is platform-correct.
fn basename(path: &str) -> String {
  // `Path::new` borrows the &str — no allocation.
  let p = std::path::Path::new(path);
  // `file_name()` returns the last component as an `Option<&OsStr>`.
  // `and_then` chains: if file_name is Some, convert to &str.
  // `unwrap_or` falls back to the original path.
  p.file_name()
    .and_then(|n| n.to_str())
    .unwrap_or(path)
    .to_string()
}

/// Compute a rectangle centered within `parent`.
/// `pct_x` and `pct_y` are percentages (0–100) of the parent size.
fn centered_rect(pct_x: u16, pct_y: u16, parent: Rect) -> Rect {
  let w = (parent.width * pct_x / 100).min(parent.width);
  let h = (parent.height * pct_y / 100).min(parent.height);
  let x = parent.x + (parent.width.saturating_sub(w)) / 2;
  let y = parent.y + (parent.height.saturating_sub(h)) / 2;
  Rect::new(x, y, w, h)
}
