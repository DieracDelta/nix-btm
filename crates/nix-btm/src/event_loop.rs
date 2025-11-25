use std::{
    collections::{BTreeSet, HashMap},
    ops::Deref,
    panic,
};

use crossterm::event::{
    Event, EventStream, KeyCode, KeyEventKind, KeyModifiers,
};
use futures::{
    StreamExt,
    stream::{BoxStream, SelectAll},
};
use ratatui::{
    crossterm::{
        event::DisableMouseCapture,
        execute,
        terminal::{
            EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
            enable_raw_mode,
        },
    },
    prelude::CrosstermBackend,
};
use tokio::sync::watch;
use tokio_stream::wrappers::WatchStream;

use crate::{
    app::{App, Pane, SelectedTab, Terminal, TreeToggle},
    get_stats::{NIX_USERS, ProcMetadata, SORTED_NIX_USERS},
    handle_internal_json::JobsStateInner,
    shutdown::Shutdown,
    ui::ui,
};

fn setup_terminal() -> crate::app::Result<crate::app::Terminal> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let panic_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic| {
        let _ = disable_raw_mode();
        let _ = execute!(
            std::io::stderr(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );

        panic_hook(panic);
    }));

    Ok(terminal)
}

pub enum Events {
    TickBJ(Box<JobsStateInner>),
    TickProcMD(HashMap<String, BTreeSet<ProcMetadata>>),
    InputEvent(Event),
}

pub static TICK_PERIOD_SECS: u64 = 1;

pub async fn handle_keeb_event(event: Event, app: &mut App) -> bool {
    if let Event::Key(key) = event
        && key.kind == KeyEventKind::Press
    {
        match key.code {
            KeyCode::Char('y') => match app.tab_selected {
                SelectedTab::BuilderView => {}
                SelectedTab::EagleEyeView => {
                    if let Some(drv_name) =
                        &app.eagle_eye_view.state.selected().last()
                    {
                        let _ = tui_clipboard::osc52_copy(drv_name).await;
                    }
                }
                SelectedTab::BuildJobView => {}
            },
            KeyCode::Char('g') => match app.tab_selected {
                SelectedTab::BuilderView => {
                    if !SORTED_NIX_USERS.is_empty() {
                        app.builder_view
                            .state
                            .select(vec![SORTED_NIX_USERS[0].clone()]);
                    }
                }
                SelectedTab::EagleEyeView => {
                    app.eagle_eye_view.state.select_first();
                }
                SelectedTab::BuildJobView => todo!(),
            },
            KeyCode::Char('G') => match app.tab_selected {
                SelectedTab::BuilderView => {
                    if !SORTED_NIX_USERS.is_empty() {
                        app.builder_view.state.select(vec![
                            SORTED_NIX_USERS[SORTED_NIX_USERS.len() - 1]
                                .clone(),
                        ]);
                    }
                }
                SelectedTab::EagleEyeView => {
                    app.eagle_eye_view.state.select_last();
                }
                SelectedTab::BuildJobView => {}
            },
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Tab => match app.tab_selected {
                SelectedTab::BuilderView => {
                    let num_open = app.builder_view.state.opened().len();
                    if num_open == NIX_USERS.len() {
                        app.builder_view.state.close_all();
                    } else {
                        for user in Deref::deref(&NIX_USERS) {
                            app.builder_view.state.open(vec![user.to_string()]);
                        }
                    }
                }
                SelectedTab::EagleEyeView => {
                    app.eagle_eye_view.state.toggle_selected();
                    app.eagle_eye_view.last_toggle = TreeToggle::Never;
                }
                SelectedTab::BuildJobView => {}
            },
            KeyCode::Char('u')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                match app.tab_selected {
                    SelectedTab::BuilderView => {
                        app.builder_view.state.scroll_down(10);
                    }
                    SelectedTab::EagleEyeView => {
                        app.eagle_eye_view.state.scroll_up(10);
                    }
                    SelectedTab::BuildJobView => (),
                }
            }
            KeyCode::Char('d')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                match app.tab_selected {
                    SelectedTab::BuilderView => {
                        app.builder_view.state.scroll_down(10);
                    }
                    SelectedTab::EagleEyeView => {
                        app.eagle_eye_view.state.scroll_down(10);
                    }
                    SelectedTab::BuildJobView => (),
                }
            }
            KeyCode::Char('j') | KeyCode::Down => match app.tab_selected {
                SelectedTab::BuilderView => {
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
                SelectedTab::EagleEyeView => {
                    app.eagle_eye_view.state.key_down();
                }
                SelectedTab::BuildJobView => todo!(),
            },
            KeyCode::Char('k') | KeyCode::Up => match app.tab_selected {
                SelectedTab::BuilderView => {
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
                SelectedTab::EagleEyeView => {
                    app.eagle_eye_view.state.key_up();
                }
                SelectedTab::BuildJobView => {}
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
            KeyCode::Char('O') => match app.tab_selected {
                SelectedTab::BuilderView => (),
                SelectedTab::EagleEyeView => {
                    app.eagle_eye_view.perform_toggle = true;
                }
                SelectedTab::BuildJobView => (),
            },
            KeyCode::Char('A') => match app.tab_selected {
                SelectedTab::BuilderView => (),
                SelectedTab::EagleEyeView => {
                    app.eagle_eye_view.active_only =
                        app.eagle_eye_view.active_only.increment();
                }
                SelectedTab::BuildJobView => (),
            },
            KeyCode::Enter => {
                // HACK the api has a cleaner way
                if !app.builder_view.state.key_right() {
                    app.builder_view.state.key_left();
                }
            }
            KeyCode::Char('M') => match app.tab_selected {
                SelectedTab::BuilderView => {
                    app.builder_view.man_toggle = !app.builder_view.man_toggle;
                }
                SelectedTab::EagleEyeView => {
                    app.eagle_eye_view.man_toggle =
                        !app.eagle_eye_view.man_toggle;
                }
                SelectedTab::BuildJobView => {
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
    shutdown: Shutdown,
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
            .map(|x| Events::TickBJ(Box::new(x)))
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
                app.cur_info_builds = *new_info_builds;
            }
            Some(Events::TickProcMD(new_info)) => {
                app.cur_info = new_info;
            }
            Some(Events::InputEvent(event)) => {
                let should_quit = handle_keeb_event(event, &mut app).await;
                if should_quit {
                    shutdown.trigger();
                    break;
                }
            }
            None => {
                shutdown.trigger();
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
