use std::{io, ops::Deref, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};

use crate::{
    App, Pane, Terminal,
    get_stats::{NIX_USERS, SORTED_NIX_USERS},
    ui::ui,
};

pub fn event_loop(terminal: &mut Terminal, mut app: App) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, &mut app))?;

        // TODO fix scrolling to only scroll by root node
        if event::poll(Duration::from_millis(32))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('g') => {
                    app.builder_view
                        .state
                        .select(vec![SORTED_NIX_USERS[0].clone()]);
                }
                KeyCode::Char('G') => {
                    app.builder_view.state.select(vec![
                        SORTED_NIX_USERS[SORTED_NIX_USERS.len() - 1].clone(),
                    ]);
                }
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Tab => {
                    let num_open = app.builder_view.state.opened().len();
                    if num_open == NIX_USERS.len() {
                        app.builder_view.state.close_all();
                    } else {
                        for user in Deref::deref(&NIX_USERS) {
                            app.builder_view.state.open(vec![user.to_string()]);
                        }
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    if let Some(selected) =
                        app.builder_view.state.selected().first()
                    {
                        let idx = SORTED_NIX_USERS
                            .iter()
                            .position(|x| x == selected)
                            .unwrap();
                        let new_idx = (idx + 1) % SORTED_NIX_USERS.len();
                        app.builder_view
                            .state
                            .select(vec![SORTED_NIX_USERS[new_idx].clone()]);
                    } else {
                        app.builder_view
                            .state
                            .select(vec![SORTED_NIX_USERS[0].clone()]);
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if let Some(selected) =
                        app.builder_view.state.selected().first()
                    {
                        let idx = SORTED_NIX_USERS
                            .iter()
                            .position(|x| x == selected)
                            .unwrap();
                        let new_idx = (idx - 1) % SORTED_NIX_USERS.len();
                        app.builder_view
                            .state
                            .select(vec![SORTED_NIX_USERS[new_idx].clone()]);
                    } else {
                        app.builder_view
                            .state
                            .select(vec![SORTED_NIX_USERS[0].clone()]);
                    }
                }
                KeyCode::Char('h') => {
                    app.builder_view.go_left();
                }
                KeyCode::Char('l') => {
                    app.builder_view.go_right();
                }
                KeyCode::Char('<') | KeyCode::Left => {
                    if app.builder_view.selected_pane == Pane::Right {
                        app.builder_view.horizontal_scroll = app
                            .builder_view
                            .horizontal_scroll
                            .saturating_sub(1);
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
                        app.builder_view.man_toggle =
                            !app.builder_view.man_toggle;
                    }
                    crate::SelectedTab::BirdsEyeView => {
                        app.birds_eye_view.man_toggle =
                            !app.birds_eye_view.man_toggle;
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
    }
}
