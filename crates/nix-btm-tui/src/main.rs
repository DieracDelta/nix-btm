mod app;
mod client;
mod clipboard;
mod ui;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::prelude::*;
use std::io::stdout;

use nix_btm_common::protocol::{self, BuildAction};

const STATUS_DURATION: std::time::Duration = std::time::Duration::from_secs(3);
const STATUS_DURATION_ERR: std::time::Duration = std::time::Duration::from_secs(8);

#[tokio::main]
async fn main() -> Result<()> {
    let mut client = client::BtmClient::connect(protocol::control_socket_path()).await?;
    let mut app = app::App::new();

    // Initial data fetch.
    app.refresh(&mut client).await?;

    // Setup terminal.
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    // Main loop.
    let result = run_loop(&mut terminal, &mut app, &mut client).await;

    // Restore terminal.
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    result
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut app::App,
    client: &mut client::BtmClient,
) -> Result<()> {
    let refresh_interval = std::time::Duration::from_secs(1);
    let mut last_refresh = std::time::Instant::now();

    loop {
        // Only redraw when something changed.
        if app.dirty {
            terminal.draw(|frame| ui::render(frame, app))?;
            app.dirty = false;
        }

        // Drain all pending key events, then wait up to 50ms for the next one.
        let mut had_event = false;
        loop {
            let timeout = if had_event {
                std::time::Duration::ZERO
            } else {
                std::time::Duration::from_millis(50)
            };

            if !event::poll(timeout)? {
                break;
            }

            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                had_event = true;
                app.dirty = true;

                // Handle text input mode (search / filter).
                if app.input_mode {
                    match key.code {
                        KeyCode::Esc => { app.cancel_input(); }
                        KeyCode::Enter => { app.exit_input(); }
                        KeyCode::Backspace => { app.input_pop(); }
                        KeyCode::Char(c) => { app.input_push(c); }
                        _ => {}
                    }
                    continue;
                }

                // Handle action prompt (waiting for action key after K).
                if app.action_prompt {
                    let has_cgroups = app.selected_have_cgroups();
                    let action = match key.code {
                        KeyCode::Char('k') => Some(BuildAction::Kill),
                        KeyCode::Char('f') if has_cgroups => Some(BuildAction::Freeze),
                        KeyCode::Char('u') if has_cgroups => Some(BuildAction::Unfreeze),
                        _ => {
                            app.cancel_action_prompt();
                            continue;
                        }
                    };
                    if let Some(act) = action {
                        let ids = app.selected_activity_ids();
                        let (ok, errs) = client.control_builds(&ids, act).await;
                        let (msg, dur) = if errs.is_empty() {
                            (format!("Sent {act:?} to {ok} build(s)"), STATUS_DURATION)
                        } else {
                            (format!("Sent {act:?}: {ok} ok, {} failed: {}", errs.len(), errs.join("; ")), STATUS_DURATION_ERR)
                        };
                        app.set_status(msg, dur);
                        app.cancel_action_prompt();
                        if app.visual_mode {
                            app.exit_visual();
                        }
                    }
                    continue;
                }

                // Handle yank prompt (waiting for field key after y).
                if app.yank_prompt {
                    if key.code == KeyCode::Esc {
                        app.cancel_yank_prompt();
                        continue;
                    }
                    let key_char = match key.code {
                        KeyCode::Char(c) => c,
                        _ => { app.cancel_yank_prompt(); continue; }
                    };
                    let text = if app.show_processes {
                        app.yank_proc_field(key_char)
                    } else {
                        app.yank_build_field(key_char)
                    };
                    if let Some(text) = text {
                        match clipboard::osc52_copy(&text) {
                            Ok(()) => app.set_status("Yanked to clipboard".to_string(), STATUS_DURATION),
                            Err(e) => app.set_status(format!("Yank failed: {e}"), STATUS_DURATION_ERR),
                        }
                    } else {
                        app.set_status("Field not available".to_string(), STATUS_DURATION);
                    }
                    app.cancel_yank_prompt();
                    if app.visual_mode { app.exit_visual(); }
                    continue;
                }

                // Handle pending `g` for `gg` chord.
                if app.pending_g {
                    app.pending_g = false;
                    if key.code == KeyCode::Char('g') {
                        app.select_top();
                        continue;
                    }
                    // Not `g` — fall through to normal handling.
                }

                // Handle pending `z` for `z-c`/`z-o` fold chords.
                if app.pending_z {
                    app.pending_z = false;
                    if app.show_processes {
                        match key.code {
                            KeyCode::Char('c') => { app.proc_fold_close(); continue; }
                            KeyCode::Char('o') => { app.proc_fold_open(); continue; }
                            KeyCode::Char('a') => { app.proc_toggle_fold(); continue; }
                            _ => {}
                        }
                    } else if app.show_dep_tree {
                        match key.code {
                            KeyCode::Char('c') => { app.fold_close(); continue; }
                            KeyCode::Char('o') => { app.fold_open(); continue; }
                            KeyCode::Char('a') => { app.toggle_fold(); continue; }
                            _ => {}
                        }
                    }
                    // Not a fold key — fall through to normal handling.
                }

                match (key.code, key.modifiers) {
                    (KeyCode::Char('q'), _) => return Ok(()),
                    (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                        app.select_prev()
                    }
                    (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                        app.select_next()
                    }
                    (KeyCode::Char('u'), KeyModifiers::CONTROL) => app.half_page_up(),
                    (KeyCode::Char('d'), KeyModifiers::CONTROL) => app.half_page_down(),
                    (KeyCode::Char('g'), KeyModifiers::NONE) => {
                        app.pending_g = true;
                    }
                    (KeyCode::Char('z'), KeyModifiers::NONE) => {
                        app.pending_z = true;
                    }
                    (KeyCode::Char('G'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
                        app.select_bottom()
                    }
                    (KeyCode::Char('K'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
                        if !app.selected_activity_ids().is_empty() {
                            app.show_action_prompt();
                        }
                    }
                    (KeyCode::Char('V'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
                        if app.visual_mode {
                            app.exit_visual();
                        } else {
                            app.enter_visual();
                        }
                    }
                    (KeyCode::Char('y'), KeyModifiers::NONE) => {
                        app.show_yank_prompt();
                    }
                    (KeyCode::Char('a'), KeyModifiers::NONE) => app.toggle_show_all(),
                    (KeyCode::Char(' '), _) | (KeyCode::Tab, _) => {
                        if app.show_processes {
                            app.proc_toggle_fold();
                        } else if app.show_dep_tree {
                            app.toggle_fold();
                        }
                    }
                    (KeyCode::Char('p'), KeyModifiers::NONE) => {
                        if app.visual_mode {
                            app.exit_visual();
                        }
                        app.toggle_processes();
                    }
                    (KeyCode::Char('d'), KeyModifiers::NONE) => {
                        if app.visual_mode {
                            app.exit_visual();
                        }
                        app.toggle_dep_tree();
                    }
                    (KeyCode::Char('l'), KeyModifiers::NONE) => app.toggle_log_panel(),
                    (KeyCode::Char('['), KeyModifiers::NONE) => app.log_scroll_up(),
                    (KeyCode::Char(']'), KeyModifiers::NONE) => app.log_scroll_down(),
                    (KeyCode::Char('h'), KeyModifiers::NONE) => app.toggle_history(),
                    (KeyCode::Char('r'), KeyModifiers::NONE) => app.toggle_machines(),
                    (KeyCode::Char('/'), KeyModifiers::NONE) => {
                        if app.show_dep_tree { app.enter_search(); }
                    }
                    (KeyCode::Char('f'), KeyModifiers::NONE) => {
                        if !app.show_dep_tree { app.enter_filter(); }
                    }
                    (KeyCode::Char('n'), KeyModifiers::NONE) => {
                        if app.show_dep_tree {
                            app.search_next();
                        } else if let Some(id) = app.selected_build_id() {
                            match client.set_nice(id, 10).await {
                                Ok(()) => app.set_status(format!("Set nice (weight=10) on build {id}"), STATUS_DURATION),
                                Err(e) => app.set_status(format!("nice failed: {e}"), STATUS_DURATION_ERR),
                            }
                        }
                    }
                    (KeyCode::Char('N'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
                        if app.show_dep_tree {
                            app.search_prev();
                        }
                    }
                    (KeyCode::Char('c'), KeyModifiers::NONE) => {
                        if let Some(id) = app.selected_build_id() {
                            match client.set_cpu_limit(id, 100).await {
                                Ok(()) => app.set_status(format!("Set CPU limit (100%) on build {id}"), STATUS_DURATION),
                                Err(e) => app.set_status(format!("cpu limit failed: {e}"), STATUS_DURATION_ERR),
                            }
                        }
                    }
                    (KeyCode::Char('m'), KeyModifiers::NONE) => {
                        if let Some(id) = app.selected_build_id() {
                            match client.set_memory_limit(id, 4 * 1024 * 1024 * 1024).await {
                                Ok(()) => app.set_status(format!("Set memory limit (4G) on build {id}"), STATUS_DURATION),
                                Err(e) => app.set_status(format!("mem limit failed: {e}"), STATUS_DURATION_ERR),
                            }
                        }
                    }
                    (KeyCode::Esc, _) => {
                        if app.visual_mode {
                            app.exit_visual();
                        } else if !app.builds_filter.is_empty() {
                            app.builds_filter.clear();
                        }
                    }
                    _ => {}
                }
            }
        }

        // Refresh data on a 1-second timer.
        if last_refresh.elapsed() >= refresh_interval {
            app.refresh(client).await?;
            last_refresh = std::time::Instant::now();
        }
    }
}
