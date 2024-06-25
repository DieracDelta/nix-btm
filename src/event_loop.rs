use std::{io, ops::Deref, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};

use crate::{
    get_stats::{NIX_USERS, SORTED_NIX_USERS},
    ui::ui,
    App, Terminal, WhichPane,
};

pub fn event_loop(terminal: &mut Terminal, mut app: App) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, &mut app))?;

        // TODO fix scrolling to only scroll by root node
        if event::poll(Duration::from_millis(32))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('g') => {
                            app.state.select(vec![SORTED_NIX_USERS[0].clone()]);
                        }
                        KeyCode::Char('G') => {
                            app.state
                                .select(vec![SORTED_NIX_USERS[SORTED_NIX_USERS.len() - 1].clone()]);
                        }
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Tab => {
                            let num_open = app.state.opened().len();
                            if num_open == NIX_USERS.len() {
                                app.state.close_all();
                            } else {
                                for user in Deref::deref(&NIX_USERS) {
                                    app.state.open(vec![user.to_string()]);
                                }
                            }
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            if let Some(selected) = app.state.selected().first() {
                                let idx =
                                    SORTED_NIX_USERS.iter().position(|x| x == selected).unwrap();
                                let new_idx = (idx + 1) % SORTED_NIX_USERS.len();
                                app.state.select(vec![SORTED_NIX_USERS[new_idx].clone()]);
                            } else {
                                app.state.select(vec![SORTED_NIX_USERS[0].clone()]);
                            }
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            if let Some(selected) = app.state.selected().first() {
                                let idx =
                                    SORTED_NIX_USERS.iter().position(|x| x == selected).unwrap();
                                let new_idx = (idx - 1) % SORTED_NIX_USERS.len();
                                app.state.select(vec![SORTED_NIX_USERS[new_idx].clone()]);
                            } else {
                                app.state.select(vec![SORTED_NIX_USERS[0].clone()]);
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
                        KeyCode::Char('<') | KeyCode::Left => {
                            if app.which_pane == WhichPane::Right {
                                app.horizontal_scroll = app.horizontal_scroll.saturating_sub(1);
                            }
                        }
                        KeyCode::Char('>') | KeyCode::Right => {
                            if app.which_pane == WhichPane::Right {
                                app.horizontal_scroll += 1;
                            }
                        }
                        KeyCode::Enter => {
                            // HACK the api has a cleaner way
                            if !app.state.key_right() {
                                app.state.key_left();
                            }
                        }
                        KeyCode::Char('M') => {
                            app.man_toggle = !app.man_toggle;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}
