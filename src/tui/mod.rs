//! TUI entry point: terminal lifecycle, event loop, tick.
//!
//! Enters alternate screen + raw mode in a RAII guard so panics also restore
//! the terminal. A background thread polls input; the main thread ticks at
//! ~16 fps, fetching sessions from the daemon each tick.

pub mod anim;
pub mod client;
pub mod layout;
pub mod peek;
pub mod render;
pub mod stats;
pub mod theme;
pub mod viewmodel;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::history;
use crate::jump::platform::RealExecutor;
use crate::jump::{JumpOutcome, jump};
use crate::protocol::SessionDetail;
use crate::sock;
use anim::Anim;
use layout::{PeekPlacement, peek_placement, split_for_inline_peek, visible_window};
use render::{RenderState, render_frame};
use theme::Theme;
use viewmodel::{Selection, group_and_sort, total_rows};

// ── RAII terminal guard ───────────────────────────────────────────────────────

struct TermGuard;

impl TermGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(std::io::stdout(), EnterAlternateScreen)?;
        Ok(TermGuard)
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        // Best-effort: ignore errors during cleanup (may already be panicking)
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
    }
}

// ── Input events ──────────────────────────────────────────────────────────────

#[derive(Debug)]
enum InputEvent {
    Key(KeyEvent),
    Tick,
    Resize,
    /// Jump completed in background thread.
    JumpDone(JumpOutcome),
}

// ── run_tui ───────────────────────────────────────────────────────────────────

const TICK_RATE: Duration = Duration::from_millis(41); // ~24 fps
const FETCH_INTERVAL: Duration = Duration::from_millis(500); // fetch every 0.5 s

/// Entry point for `roost` (no-arg invocation).
///
/// 1. Ensures the daemon is running (start it if not).
/// 2. Enters alternate screen + raw mode.
/// 3. Runs the render loop at ~24 fps.
/// 4. Exits cleanly on `q` / `Ctrl-C`.
pub fn run_tui() -> Result<()> {
    // Try to ensure daemon, but don't hard-fail if it takes a moment.
    let _ = sock::ensure_daemon();

    let _guard = TermGuard::enter()?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // ── State ─────────────────────────────────────────────────────────────
    let anim = Anim::new();
    let theme = Theme::classic();
    let mut selection = Selection::new();
    let mut sessions = vec![];
    let mut daemon_ready = false;

    // Session IDs to maintain stable selection across refreshes
    let mut selected_id: Option<String> = None;

    // Peek panel state
    let mut peek_open = false;
    let mut peek_detail: Option<SessionDetail> = None;
    // Number of recent-event rows scrolled past in the peek panel (0 = newest).
    let mut peek_scroll: usize = 0;

    // Stats page state. A persistent read-only connection (opened lazily when
    // the page is first shown) is reused across refreshes to avoid reopening
    // the DB on every tick.
    let mut stats_open = false;
    let mut stats_conn: Option<rusqlite::Connection> = None;
    let mut stats_data: Option<stats::StatsData> = None;
    // Number of logical lines scrolled past at the top of the stats page.
    let mut stats_scroll: usize = 0;

    // Jump status: message shown in footer + timestamp for auto-clear
    let mut jump_status: Option<String> = None;
    let mut jump_status_at: Option<Instant> = None;
    const JUMP_STATUS_DURATION: Duration = Duration::from_secs(4);

    let mut last_fetch = Instant::now() - FETCH_INTERVAL; // trigger fetch on first tick

    // Input thread sends events to main thread.
    let (tx, rx) = mpsc::channel::<InputEvent>();

    let tx_input = tx.clone();
    std::thread::spawn(move || {
        loop {
            if event::poll(TICK_RATE).unwrap_or(false) {
                match event::read() {
                    Ok(Event::Key(k)) => {
                        let _ = tx_input.send(InputEvent::Key(k));
                    }
                    Ok(Event::Resize(_, _)) => {
                        let _ = tx_input.send(InputEvent::Resize);
                    }
                    _ => {}
                }
            } else {
                let _ = tx_input.send(InputEvent::Tick);
            }
        }
    });

    // ── Main render loop ──────────────────────────────────────────────────
    loop {
        // Periodically fetch fresh sessions from daemon.
        if last_fetch.elapsed() >= FETCH_INTERVAL {
            let (new_sessions, ready) = client::fetch_sessions();
            daemon_ready = ready;
            sessions = new_sessions;
            last_fetch = Instant::now();

            // If peek is open, refresh detail for the current selection at the
            // same cadence as the session list so the panel stays live.
            if peek_open && let Some(ref id) = selected_id {
                match client::fetch_detail(id) {
                    Some(detail) => peek_detail = Some(detail),
                    None => {
                        // Session disappeared; close peek gracefully.
                        peek_open = false;
                        peek_detail = None;
                    }
                }
            }

            // Keep the stats page live on the same cadence (reuse the read-only
            // connection; recomputing the aggregates is cheap).
            if stats_open && let Some(conn) = &stats_conn {
                stats_data = Some(stats::compute(conn));
            }
        }

        let groups = group_and_sort(&sessions);
        let n_rows = total_rows(&groups);

        // Restore/update selection: try to track the same session id
        if let Some(ref id) = selected_id {
            // Find the flat index of this id
            let mut found = false;
            let mut flat = 0usize;
            'outer: for g in &groups {
                for row in &g.rows {
                    if &row.id == id {
                        selection.index = Some(flat);
                        found = true;
                        break 'outer;
                    }
                    flat += 1;
                }
            }
            if !found {
                selection.clamp(n_rows);
            }
        } else {
            selection.clamp(n_rows);
        }

        // Compute scroll window
        let sel_idx = selection.index.unwrap_or(0);
        let terminal_size = terminal.size()?;
        let terminal_height = terminal_size.height;
        let terminal_width = terminal_size.width;

        // Determine peek placement mode
        let placement = peek_placement(terminal_height);

        // The list area height depends on whether peek is open and its placement.
        let list_area_height = if peek_open {
            match placement {
                PeekPlacement::Inline => terminal_height / 2,
                PeekPlacement::FullScreen => 0, // list hidden
            }
        } else {
            terminal_height
        };

        // Dynamic viewport height: subtract header (1) + footer (1) + actual group-header rows.
        // In Critical mode, non-NEEDS-INPUT groups don't get a header row.
        let height_prof = layout::height_profile(list_area_height);
        let is_critical = matches!(height_prof, layout::HeightProfile::Critical);
        let group_header_rows = if is_critical {
            // Only NEEDS INPUT group renders a header
            groups.iter().filter(|g| g.title == "NEEDS INPUT").count()
        } else {
            groups.len()
        };
        let body_h = list_area_height
            .saturating_sub(2) // header + footer
            .saturating_sub(group_header_rows as u16) as usize;
        let (scroll_offset, _) = visible_window(n_rows, sel_idx, body_h.max(1));

        // Draw
        // Auto-clear jump status after JUMP_STATUS_DURATION
        if jump_status_at.is_some_and(|at| at.elapsed() >= JUMP_STATUS_DURATION) {
            jump_status = None;
            jump_status_at = None;
        }

        // Clamp peek scroll to the part of the recent timeline that overflows
        // the panel, so ↑/↓ can't scroll into empty space.
        if peek_open {
            if let Some(ref detail) = peek_detail {
                let peek_h = match placement {
                    PeekPlacement::Inline => terminal_height - terminal_height / 2,
                    PeekPlacement::FullScreen => terminal_height,
                };
                let cap = peek::recent_rows_capacity(peek_h, detail.action.is_some());
                let max_scroll = detail.recent_events.len().saturating_sub(cap);
                peek_scroll = peek_scroll.min(max_scroll);
            }
        } else {
            peek_scroll = 0;
        }

        // Clamp the stats scroll against its content height. The page body is
        // `height - 2` rows (top + bottom borders), mirroring the peek clamp so
        // ↑/↓ never get stuck past the end.
        if stats_open {
            let body_h = terminal_height.saturating_sub(2) as usize;
            let total = stats_data.as_ref().map(stats::line_count).unwrap_or(0);
            stats_scroll = stats_scroll.min(total.saturating_sub(body_h));
        } else {
            stats_scroll = 0;
        }

        let peek_open_snap = peek_open;
        let peek_detail_snap = peek_detail.clone();
        let placement_snap = placement.clone();
        let jump_status_snap = jump_status.as_deref();
        let peek_scroll_snap = peek_scroll;
        let stats_open_snap = stats_open;
        let stats_data_snap = stats_data.clone();
        let stats_scroll_snap = stats_scroll;
        terminal.draw(|frame| {
            let full_area = frame.area();

            if stats_open_snap {
                // The stats page takes over the whole screen.
                let data = stats_data_snap.clone().unwrap_or_default();
                stats::render_stats(frame.buffer_mut(), full_area, &data, &theme, stats_scroll_snap);
            } else if peek_open_snap {
                match placement_snap {
                    PeekPlacement::Inline => {
                        let (list_area, peek_area) = split_for_inline_peek(full_area);
                        // Render list in upper half
                        let list_state = RenderState {
                            groups: &groups,
                            selection: &selection,
                            daemon_ready,
                            anim: &anim,
                            theme: &theme,
                            scroll_offset,
                            peek_open: true,
                            jump_status: jump_status_snap,
                        };
                        render_frame(frame.buffer_mut(), list_area, &list_state);
                        // Render peek in lower half
                        if let Some(ref detail) = peek_detail_snap {
                            peek::render_peek(
                                frame.buffer_mut(),
                                peek_area,
                                detail,
                                &theme,
                                peek_scroll_snap,
                            );
                        }
                    }
                    PeekPlacement::FullScreen => {
                        // Render peek full-screen
                        if let Some(ref detail) = peek_detail_snap {
                            peek::render_peek(
                                frame.buffer_mut(),
                                full_area,
                                detail,
                                &theme,
                                peek_scroll_snap,
                            );
                        }
                    }
                }
            } else {
                // Normal list render
                let state = RenderState {
                    groups: &groups,
                    selection: &selection,
                    daemon_ready,
                    anim: &anim,
                    theme: &theme,
                    scroll_offset,
                    peek_open: false,
                    jump_status: jump_status_snap,
                };
                render_frame(frame.buffer_mut(), full_area, &state);
            }
        })?;

        let _ = (terminal_width, terminal_height); // suppress potential unused warnings

        // Process one event (non-blocking: already polled via thread)
        match rx.recv_timeout(TICK_RATE) {
            Ok(InputEvent::Key(key)) => {
                let action = handle_key(
                    key,
                    &mut selection,
                    n_rows,
                    &groups,
                    &mut selected_id,
                    peek_open,
                    stats_open,
                );
                match action {
                    KeyAction::Quit => break,
                    KeyAction::ToggleStats => {
                        if stats_open {
                            stats_open = false;
                            stats_data = None;
                            stats_conn = None;
                            stats_scroll = 0;
                        } else {
                            // Opening stats supersedes the peek panel.
                            peek_open = false;
                            peek_detail = None;
                            stats_conn = history::open_readonly().ok();
                            stats_data = stats_conn.as_ref().map(stats::compute);
                            stats_open = true;
                            stats_scroll = 0;
                        }
                    }
                    KeyAction::CloseStats => {
                        stats_open = false;
                        stats_data = None;
                        stats_conn = None;
                        stats_scroll = 0;
                    }
                    KeyAction::TogglePeek => {
                        if peek_open {
                            peek_open = false;
                            peek_detail = None;
                        } else {
                            // Open peek for current selection
                            if let Some(ref id) = selected_id {
                                peek_detail = client::fetch_detail(id);
                            }
                            peek_open = peek_detail.is_some();
                            peek_scroll = 0;
                        }
                    }
                    KeyAction::ClosePeek => {
                        peek_open = false;
                        peek_detail = None;
                    }
                    KeyAction::SelectionMoved => {
                        // If peek is open, refresh detail for new selection
                        if peek_open && let Some(ref id) = selected_id {
                            if let Some(detail) = client::fetch_detail(id) {
                                peek_detail = Some(detail);
                            } else {
                                peek_open = false;
                                peek_detail = None;
                            }
                        }
                    }
                    KeyAction::NextSession | KeyAction::PrevSession => {
                        // Selection already moved in handle_key. When peek is
                        // open, switch its detail to the new session and reset
                        // the scroll position to the newest event.
                        peek_scroll = 0;
                        if peek_open && let Some(ref id) = selected_id {
                            if let Some(detail) = client::fetch_detail(id) {
                                peek_detail = Some(detail);
                            } else {
                                peek_open = false;
                                peek_detail = None;
                            }
                        }
                    }
                    KeyAction::ScrollUp => {
                        // Clamped against content height before rendering.
                        if stats_open {
                            stats_scroll = stats_scroll.saturating_sub(1);
                        } else {
                            peek_scroll = peek_scroll.saturating_sub(1);
                        }
                    }
                    KeyAction::ScrollDown => {
                        if stats_open {
                            stats_scroll = stats_scroll.saturating_add(1);
                        } else {
                            peek_scroll = peek_scroll.saturating_add(1);
                        }
                    }
                    KeyAction::Jump => {
                        // Build JumpTarget from current selection and launch in background
                        if let Some(sv) = selected_session_view(&selection, &groups) {
                            let mut target = crate::jump::JumpTarget::from_session_view(sv);
                            // Fill in cwd from daemon detail (best-effort)
                            if let Some(ref id) = selected_id
                                && let Some(detail) = client::fetch_detail(id)
                            {
                                target.cwd = detail.cwd;
                            }
                            let tx_jump = tx.clone();
                            std::thread::spawn(move || {
                                let executor = RealExecutor;
                                let outcome = jump(&target, &executor);
                                let _ = tx_jump.send(InputEvent::JumpDone(outcome));
                            });
                            jump_status = Some("Jumping…".to_string());
                            jump_status_at = Some(Instant::now());
                        } else {
                            jump_status = Some("No session selected".to_string());
                            jump_status_at = Some(Instant::now());
                        }
                    }
                    KeyAction::None => {}
                }
            }
            Ok(InputEvent::JumpDone(outcome)) => {
                jump_status = Some(outcome.message().to_string());
                jump_status_at = Some(Instant::now());
            }
            Ok(InputEvent::Resize)
            | Ok(InputEvent::Tick)
            | Err(mpsc::RecvTimeoutError::Timeout) => {
                // Just redraw
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }

    Ok(())
}

// ── Key handling ──────────────────────────────────────────────────────────────

/// The result of handling a key event.
#[derive(Debug, Clone, PartialEq, Eq)]
enum KeyAction {
    /// TUI should exit.
    Quit,
    /// Toggle peek open/closed for current selection.
    TogglePeek,
    /// Close peek (Esc when peek is open).
    ClosePeek,
    /// Selection moved (↑/↓/j/k while peek closed); caller refreshes peek if open.
    SelectionMoved,
    /// Switch to the next/previous session (`Tab` / `Shift-Tab`); when peek is
    /// open the caller refreshes the detail and resets the peek scroll.
    NextSession,
    PrevSession,
    /// Scroll the open peek detail's recent timeline (↑/↓ while peek open).
    ScrollUp,
    ScrollDown,
    /// Trigger terminal jump for current selection (`o` key).
    Jump,
    /// Toggle the stats page (`s` key).
    ToggleStats,
    /// Close the stats page (Esc while it is open).
    CloseStats,
    /// No special action needed.
    None,
}

/// Handle a key event. Returns the action to take.
fn handle_key(
    key: KeyEvent,
    selection: &mut Selection,
    n_rows: usize,
    groups: &[viewmodel::Group],
    selected_id: &mut Option<String>,
    peek_open: bool,
    stats_open: bool,
) -> KeyAction {
    // When the stats page is open it owns the screen: scroll / quit / close.
    if stats_open {
        return match key.code {
            KeyCode::Char('q') => KeyAction::Quit,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => KeyAction::Quit,
            KeyCode::Esc | KeyCode::Char('s') => KeyAction::CloseStats,
            KeyCode::Up | KeyCode::Char('k') => KeyAction::ScrollUp,
            KeyCode::Down | KeyCode::Char('j') => KeyAction::ScrollDown,
            _ => KeyAction::None,
        };
    }
    match key.code {
        KeyCode::Char('q') => return KeyAction::Quit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return KeyAction::Quit;
        }
        KeyCode::Esc => {
            if peek_open {
                return KeyAction::ClosePeek;
            } else {
                return KeyAction::Quit;
            }
        }
        KeyCode::Enter => return KeyAction::TogglePeek,
        // `o` = open / jump to agent's terminal window
        KeyCode::Char('o') => return KeyAction::Jump,
        // `s` = toggle the stats page (only from the list view).
        KeyCode::Char('s') if !peek_open => return KeyAction::ToggleStats,
        // Tab / Shift-Tab always switch the selected session, wrapping around.
        KeyCode::Tab => {
            selection.cycle_next(n_rows);
            *selected_id = current_session_id(selection, groups);
            return KeyAction::NextSession;
        }
        KeyCode::BackTab => {
            selection.cycle_prev(n_rows);
            *selected_id = current_session_id(selection, groups);
            return KeyAction::PrevSession;
        }
        // ↑/↓ (and j/k) scroll the detail when peek is open, otherwise move
        // the selection in the list.
        KeyCode::Down | KeyCode::Char('j') => {
            if peek_open {
                return KeyAction::ScrollDown;
            }
            selection.move_down(n_rows);
            *selected_id = current_session_id(selection, groups);
            return KeyAction::SelectionMoved;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if peek_open {
                return KeyAction::ScrollUp;
            }
            selection.move_up(n_rows);
            *selected_id = current_session_id(selection, groups);
            return KeyAction::SelectionMoved;
        }
        _ => {}
    }
    KeyAction::None
}

/// Get the session id at the current selection position.
fn current_session_id(selection: &Selection, groups: &[viewmodel::Group]) -> Option<String> {
    let target = selection.index?;
    let mut flat = 0usize;
    for g in groups {
        for row in &g.rows {
            if flat == target {
                return Some(row.id.clone());
            }
            flat += 1;
        }
    }
    None
}

/// Get the full `SessionView` at the current selection position.
fn selected_session_view<'a>(
    selection: &Selection,
    groups: &'a [viewmodel::Group],
) -> Option<&'a crate::protocol::SessionView> {
    let target = selection.index?;
    let mut flat = 0usize;
    for g in groups {
        for row in &g.rows {
            if flat == target {
                return Some(row);
            }
            flat += 1;
        }
    }
    None
}
