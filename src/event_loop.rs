use std::{io, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use sysinfo::Users;

use crate::{get_stats::get_nix_users, ui::ui, App, Terminal, WhichPane};

pub fn event_loop(terminal: &mut Terminal, mut app: App) -> io::Result<()> {
    let mut last_frame_instant = std::time::Instant::now();
    loop {
        app.last_tick = last_frame_instant.elapsed();
        last_frame_instant = std::time::Instant::now();
        terminal.draw(|f| ui(f, &mut app))?;

        // TODO fix scrolling to only scroll by root node
        if event::poll(Duration::from_millis(32))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('g') => {
                            let users = Users::new_with_refreshed_list();
                            let mut sorted_users: Vec<String> =
                                get_nix_users(&users).into_iter().collect();
                            sorted_users.sort_by(|x, y| {
                                let offset = if x.starts_with('_') { 7 } else { 6 };
                                let x_num: usize = x[offset..].parse().unwrap();
                                let y_num: usize = y[offset..].parse().unwrap();
                                x_num.partial_cmp(&y_num).unwrap()
                            });
                            app.state.select(vec![sorted_users[0].clone()]);
                        }
                        KeyCode::Char('G') => {
                            let users = Users::new_with_refreshed_list();
                            let mut sorted_users: Vec<String> =
                                get_nix_users(&users).into_iter().collect();
                            sorted_users.sort_by(|x, y| {
                                let offset = if x.starts_with('_') { 7 } else { 6 };
                                let x_num: usize = x[offset..].parse().unwrap();
                                let y_num: usize = y[offset..].parse().unwrap();
                                x_num.partial_cmp(&y_num).unwrap()
                            });
                            app.state
                                .select(vec![sorted_users[sorted_users.len() - 1].clone()]);
                        }
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Tab => {
                            let num_open = app.state.opened().len();
                            let users = Users::new_with_refreshed_list();
                            let users = get_nix_users(&users);
                            if num_open == users.len() {
                                app.state.close_all();
                            } else {
                                for user in users {
                                    app.state.open(vec![user]);
                                }
                            }
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            if let Some(selected) = app.state.selected().first() {
                                let users = Users::new_with_refreshed_list();
                                let mut sorted_users: Vec<String> =
                                    get_nix_users(&users).into_iter().collect();
                                sorted_users.sort_by(|x, y| {
                                    let offset = if x.starts_with('_') { 7 } else { 6 };
                                    let x_num: usize = x[offset..].parse().unwrap();
                                    let y_num: usize = y[offset..].parse().unwrap();
                                    x_num.partial_cmp(&y_num).unwrap()
                                });
                                let idx = sorted_users.iter().position(|x| x == selected).unwrap();
                                let new_idx = (idx + 1) % sorted_users.len();
                                app.state.select(vec![sorted_users[new_idx].clone()]);
                            } else {
                                let users = Users::new_with_refreshed_list();
                                let mut sorted_users: Vec<String> =
                                    get_nix_users(&users).into_iter().collect();
                                sorted_users.sort_by(|x, y| {
                                    let offset = if x.starts_with('_') { 7 } else { 6 };
                                    let x_num: usize = x[offset..].parse().unwrap();
                                    let y_num: usize = y[offset..].parse().unwrap();
                                    x_num.partial_cmp(&y_num).unwrap()
                                });
                                app.state.select(vec![sorted_users[0].clone()]);
                            }
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            if let Some(selected) = app.state.selected().first() {
                                let users = Users::new_with_refreshed_list();
                                let mut sorted_users: Vec<String> =
                                    get_nix_users(&users).into_iter().collect();
                                sorted_users.sort_by(|x, y| {
                                    // TODO don't hardcode offsets
                                    let offset = if x.starts_with('_') { 7 } else { 6 };
                                    let x_num: usize = x[offset..].parse().unwrap();
                                    let y_num: usize = y[offset..].parse().unwrap();
                                    x_num.partial_cmp(&y_num).unwrap()
                                });
                                let idx = sorted_users.iter().position(|x| x == selected).unwrap();
                                let new_idx = (idx - 1) % sorted_users.len();
                                app.state.select(vec![sorted_users[new_idx].clone()]);
                            } else {
                                let users = Users::new_with_refreshed_list();
                                let mut sorted_users: Vec<String> =
                                    get_nix_users(&users).into_iter().collect();
                                sorted_users.sort_by(|x, y| {
                                    let offset = if x.starts_with('_') { 7 } else { 6 };
                                    let x_num: usize = x[offset..].parse().unwrap();
                                    let y_num: usize = y[offset..].parse().unwrap();
                                    x_num.partial_cmp(&y_num).unwrap()
                                });
                                app.state.select(vec![sorted_users[0].clone()]);
                            }
                        }
                        KeyCode::Char('h') => {
                            if app.which_pane == WhichPane::Right {
                                app.which_pane = WhichPane::Left;
                            }
                        }
                        KeyCode::Char('l') => {
                            if app.which_pane == WhichPane::Left {
                                app.which_pane = WhichPane::Right;
                            }
                        }
                        KeyCode::Left => {
                            if app.which_pane == WhichPane::Right {
                                app.horizontal_scroll = app.horizontal_scroll.saturating_sub(1);
                                app.horizontal_scroll_state =
                                    app.horizontal_scroll_state.position(app.horizontal_scroll);
                            }
                        }
                        KeyCode::Right => {
                            if app.which_pane == WhichPane::Right {
                                app.horizontal_scroll = app.horizontal_scroll.saturating_add(1);
                                app.horizontal_scroll_state =
                                    app.horizontal_scroll_state.position(app.horizontal_scroll);
                            }
                        }
                        KeyCode::Enter => {
                            // HACK the api has a cleaner way
                            if !app.state.key_right() {
                                app.state.key_left();
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}
