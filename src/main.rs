use std::collections::HashSet;
use std::error::Error;
use std::io::Stdout;
use std::{io, panic};

pub mod event_loop;
pub mod get_stats;
pub mod gruvbox;
pub mod ui;

use crossterm::event::DisableMouseCapture;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use event_loop::event_loop;
use get_stats::{
    construct_pid_map, construct_tree, dump_pids, get_active_users_and_pids, get_drvs,
    strip_tf_outta_tree,
};
use ratatui::backend::CrosstermBackend;
use ratatui::style::Style;
use ratatui::widgets::ScrollbarState;
use tui_tree_widget::TreeState;
use ui::{
    BORDER_STYLE_SELECTED, BORDER_STYLE_UNSELECTED, TITLE_STYLE_SELECTED, TITLE_STYLE_UNSELECTED,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
type Terminal = ratatui::Terminal<CrosstermBackend<Stdout>>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Pane {
    #[default]
    Left,
    Right,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SelectedTab {
    #[default]
    BuilderView,
    BirdsEyeView,
}

#[derive(Default, Debug)]
pub struct App {
    builder_view: BuilderViewState,
    birds_eye_view: BirdsEyeViewState,
    tab_selected: SelectedTab,
}

#[derive(Default, Debug)]
pub struct BirdsEyeViewState {}

#[derive(Default, Debug)]
pub struct BuilderViewState {
    pub vertical_scroll_state: ScrollbarState,
    pub vertical_scroll: usize,
    pub horizontal_scroll: usize,
    state: TreeState<String>,
    pub selected_pane: Pane,
    pub man_toggle: bool,
}

impl BuilderViewState {
    pub fn gen_title_style(&self, this_pane: Pane) -> Style {
        if self.selected_pane == this_pane {
            *TITLE_STYLE_SELECTED
        } else {
            *TITLE_STYLE_UNSELECTED
        }
    }

    pub fn gen_border_style(&self, this_pane: Pane) -> Style {
        if self.selected_pane == this_pane {
            *BORDER_STYLE_SELECTED
        } else {
            *BORDER_STYLE_UNSELECTED
        }
    }

    pub fn go_right(&mut self) {
        if self.selected_pane == Pane::Left {
            self.selected_pane = Pane::Right;
        }
    }

    pub fn go_left(&mut self) {
        if self.selected_pane == Pane::Right {
            self.selected_pane = Pane::Left;
        }
    }
}

pub fn main() {
    if !sysinfo::IS_SUPPORTED_SYSTEM {
        panic!("This OS is supported!");
    }

    //let sets = get_active_users_and_pids();
    //let mut total_set = HashSet::new();
    //for (_, set) in sets {
    //    let sett: HashSet<_> = set.into_iter().collect();
    //    let unioned = total_set.union(&sett).cloned();
    //    total_set = unioned.collect::<HashSet<_>>();
    //}
    //let mut map = construct_pid_map(total_set.clone());
    //let total_tree = construct_tree(map.keys().cloned().collect(), &mut map)
    //    .into_iter()
    //    .next()
    //    .unwrap()
    //    .1;
    //let real_roots = strip_tf_outta_tree(total_tree, &map);
    //let drvs_roots = get_drvs(real_roots);
    //println!("{:#?}", drvs_roots);
    // dump_pids(&real_roots, &map);
    // println!("{t:#?}");

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
