use std::{io, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use sysinfo::Users;

use crate::{get_stats::get_nix_users, ui::ui, App, Terminal};

pub fn event_loop(terminal: &mut Terminal, mut app: App) -> io::Result<()> {
    let mut last_frame_instant = std::time::Instant::now();
    loop {
        app.last_tick = last_frame_instant.elapsed();
        last_frame_instant = std::time::Instant::now();
        terminal.draw(|f| ui(f, &mut app))?;

        if event::poll(Duration::from_millis(32))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
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
                        },
                        KeyCode::Char('j') | KeyCode::Down => {
                            app.state.key_down();
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            app.state.key_up();
                        }
                        KeyCode::Char('h') | KeyCode::Left => {
                            app.state.key_left();
                        }
                        KeyCode::Char('l') | KeyCode::Right => {
                        }
                        KeyCode::Enter => {
                            // HACK the api has a cleaner way
                            if !app.state.key_right(){
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

