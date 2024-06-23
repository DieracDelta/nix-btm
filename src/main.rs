use std::error::Error;
use std::io::Stdout;
use std::time::Duration;
use std::{io, panic};

pub mod get_stats;
pub mod event_loop;
pub mod ui;
pub mod gruvbox;
pub mod tui_tree_items;

use crossterm::event::{DisableMouseCapture};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute};
use event_loop::event_loop;
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::ScrollbarState;
use tui_tree_widget::{TreeItem, TreeState};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
type Terminal = ratatui::Terminal<CrosstermBackend<Stdout>>;

#[derive(Default)]
pub struct App {
    // TODO delete lol this is leftovers
    last_tick: Duration,
    pub vertical_scroll_state: ScrollbarState,
    pub horizontal_scroll_state: ScrollbarState,
    pub vertical_scroll: usize,
    pub horizontal_scroll: usize,
    state: TreeState<String>,
    // items: Vec<TreeItem<'static, &'static str>>,
}

pub fn main() {
    if !sysinfo::IS_SUPPORTED_SYSTEM {
        panic!("This OS is supported!");
    }

    run().unwrap();
}

fn run() -> Result<()> {
    let mut terminal = setup_terminal()?;

    // create app and run it
    let app = App::default();
    let res = event_loop(&mut terminal, app);

    // restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{err:?}");
    }

    Ok(())
}

fn setup_terminal() -> Result<Terminal> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let panic_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stderr(), LeaveAlternateScreen, DisableMouseCapture);

        panic_hook(panic);
    }));

    Ok(terminal)
}
