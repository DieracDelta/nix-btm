use std::{
    collections::{BTreeSet, HashMap},
    ops::Deref,
    sync::{Arc, atomic::AtomicBool},
};

use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind};
use futures::{
    StreamExt,
    stream::{BoxStream, SelectAll},
};
use ratatui::crossterm::{
    event::DisableMouseCapture, execute, terminal::disable_raw_mode,
};
use tokio::sync::watch;
use tokio_stream::wrappers::WatchStream;

use crate::{
    App, Pane, Terminal,
    get_stats::{NIX_USERS, ProcMetadata, SORTED_NIX_USERS},
    handle_internal_json::JobsStateInner,
    setup_terminal,
    ui::ui,
};

pub enum Events {
    TickBJ(JobsStateInner),
    TickProcMD(HashMap<String, BTreeSet<ProcMetadata>>),
    InputEvent(Event),
}

pub static TICK_PERIOD_SECS: u64 = 1;

pub async fn handle_keeb_event(event: Event, app: &mut App) -> bool {
    if let Event::Key(key) = event
        && key.kind == KeyEventKind::Press
    {
        match key.code {
            KeyCode::Char('g') => {
                if !SORTED_NIX_USERS.is_empty() {
                    app.builder_view
                        .state
                        .select(vec![SORTED_NIX_USERS[0].clone()]);
                }
            }
            KeyCode::Char('G') => {
                if !SORTED_NIX_USERS.is_empty() {
                    app.builder_view.state.select(vec![
                        SORTED_NIX_USERS[SORTED_NIX_USERS.len() - 1].clone(),
                    ]);
                }
            }
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Tab => match app.tab_selected {
                crate::SelectedTab::BuilderView => {
                    let num_open = app.builder_view.state.opened().len();
                    if num_open == NIX_USERS.len() {
                        app.builder_view.state.close_all();
                    } else {
                        for user in Deref::deref(&NIX_USERS) {
                            app.builder_view.state.open(vec![user.to_string()]);
                        }
                    }
                }
                crate::SelectedTab::EagleEyeView => {
                    app.eagle_eye_view.state.toggle_selected();
                }
                crate::SelectedTab::BuildJobView => {}
            },
            KeyCode::Char('j') | KeyCode::Down => match app.tab_selected {
                crate::SelectedTab::BuilderView => {
                    if !SORTED_NIX_USERS.is_empty() {
                        if let Some(selected) =
                            app.builder_view.state.selected().first()
                        {
                            let idx = SORTED_NIX_USERS
                                .iter()
                                .position(|x| x == selected)
                                .unwrap();
                            let new_idx = (idx + 1) % SORTED_NIX_USERS.len();
                            app.builder_view.state.select(vec![
                                SORTED_NIX_USERS[new_idx].clone(),
                            ]);
                        } else {
                            app.builder_view
                                .state
                                .select(vec![SORTED_NIX_USERS[0].clone()]);
                        }
                    }
                }
                crate::SelectedTab::EagleEyeView => {
                    app.eagle_eye_view.state.key_down();
                }
                crate::SelectedTab::BuildJobView => todo!(),
            },
            KeyCode::Char('k') | KeyCode::Up => match app.tab_selected {
                crate::SelectedTab::BuilderView => {
                    if !SORTED_NIX_USERS.is_empty() {
                        if let Some(selected) =
                            app.builder_view.state.selected().first()
                        {
                            let idx = SORTED_NIX_USERS
                                .iter()
                                .position(|x| x == selected)
                                .unwrap();
                            let new_idx = (idx - 1) % SORTED_NIX_USERS.len();
                            app.builder_view.state.select(vec![
                                SORTED_NIX_USERS[new_idx].clone(),
                            ]);
                        } else {
                            app.builder_view
                                .state
                                .select(vec![SORTED_NIX_USERS[0].clone()]);
                        }
                    }
                }
                crate::SelectedTab::EagleEyeView => {
                    app.eagle_eye_view.state.key_up();
                }
                crate::SelectedTab::BuildJobView => {}
            },
            KeyCode::Char('h') => {
                app.builder_view.go_left();
            }
            KeyCode::Char('l') => {
                app.builder_view.go_right();
            }
            KeyCode::Char('<') | KeyCode::Left => {
                if app.builder_view.selected_pane == Pane::Right {
                    app.builder_view.horizontal_scroll =
                        app.builder_view.horizontal_scroll.saturating_sub(1);
                }
            }
            KeyCode::Char('>') | KeyCode::Right => {
                if app.builder_view.selected_pane == Pane::Right {
                    app.builder_view.horizontal_scroll += 1;
                }
            }
            KeyCode::Enter => {
                // HACK the api has a cleaner way
                if !app.builder_view.state.key_right() {
                    app.builder_view.state.key_left();
                }
            }
            KeyCode::Char('M') => match app.tab_selected {
                crate::SelectedTab::BuilderView => {
                    app.builder_view.man_toggle = !app.builder_view.man_toggle;
                }
                crate::SelectedTab::EagleEyeView => {
                    app.eagle_eye_view.man_toggle =
                        !app.eagle_eye_view.man_toggle;
                }
                crate::SelectedTab::BuildJobView => {
                    app.build_job_view.man_toggle =
                        !app.build_job_view.man_toggle;
                }
            },
            KeyCode::Char('n') => {
                app.tab_selected = app.tab_selected.next();
            }
            KeyCode::Char('p') => {
                app.tab_selected = app.tab_selected.previous();
            }
            _ => {}
        }
    }
    false
}

pub async fn event_loop(
    mut app: Box<App>,
    is_shutdown: Arc<AtomicBool>,
    recv_proc_updates: watch::Receiver<HashMap<String, BTreeSet<ProcMetadata>>>,
    recv_job_updates: watch::Receiver<JobsStateInner>,
) {
    let mut terminal = Box::new(setup_terminal().unwrap());
    let event_stream: BoxStream<'static, Events> = EventStream::new()
        .filter_map(|res| async move { res.ok() })
        .map(Events::InputEvent)
        .boxed();

    let update_proc_stream: BoxStream<'static, Events> =
        WatchStream::new(recv_proc_updates)
            .map(Events::TickProcMD)
            .boxed();

    let update_job_stream: BoxStream<'static, Events> =
        WatchStream::new(recv_job_updates)
            .map(Events::TickBJ)
            .boxed();

    let mut merged: SelectAll<BoxStream<'static, Events>> =
        futures::stream::select_all(vec![
            update_proc_stream,
            event_stream,
            update_job_stream,
        ]);

    loop {
        Terminal::draw(&mut terminal, |f| ui(f, &mut app)).unwrap();

        match merged.next().await {
            Some(Events::TickBJ(new_info_builds)) => {
                app.cur_info_builds = new_info_builds;
            }
            Some(Events::TickProcMD(new_info)) => {
                app.cur_info = new_info;
            }
            Some(Events::InputEvent(event)) => {
                let should_quit = handle_keeb_event(event, &mut app).await;
                if should_quit {
                    is_shutdown
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    break;
                }
            }
            None => {
                is_shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
                break;
            }
        }
    }

    // restore terminal
    disable_raw_mode().unwrap();
    execute!(
        terminal.backend_mut(),
        ratatui::crossterm::terminal::LeaveAlternateScreen,
        DisableMouseCapture
    )
    .unwrap();
    terminal.show_cursor().unwrap();
}
