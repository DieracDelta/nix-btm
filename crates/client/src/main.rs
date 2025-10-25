use std::{
    collections::{BTreeSet, HashMap},
    error::Error,
    io::{self, Stdout},
    panic,
    sync::{Arc, atomic::AtomicBool},
    thread::{self, sleep},
    time::Duration,
};

use clap::Parser;
use futures::future::join_all;
use mimalloc::MiMalloc;
use ratatui::text::Line;
use strum::{Display, EnumCount, EnumIter, FromRepr};

pub mod event_loop;
pub mod get_stats;
pub mod gruvbox;
pub mod handle_internal_json;
pub mod listen_to_output;
pub mod ui;

use crossterm::{
    event::{DisableMouseCapture, EventStream},
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use event_loop::event_loop;
use ratatui::{
    backend::CrosstermBackend, style::Style, widgets::ScrollbarState,
};
use tokio::sync::{Mutex, watch};
//use tikv_jemallocator::Jemalloc;
use tui_tree_widget::TreeState;
use ui::{
    BORDER_STYLE_SELECTED, BORDER_STYLE_UNSELECTED, TITLE_STYLE_SELECTED,
    TITLE_STYLE_UNSELECTED,
};

use crate::{
    get_stats::{ProcMetadata, get_active_users_and_pids},
    handle_internal_json::{BuildJob, handle_daemon_info},
};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

//static GLOBAL: Jemalloc = Jemalloc;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
type Terminal = ratatui::Terminal<CrosstermBackend<Stdout>>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Pane {
    #[default]
    Left,
    Right,
}

#[derive(
    Default,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Display,
    FromRepr,
    EnumIter,
    EnumCount,
)]
pub enum SelectedTab {
    #[default]
    #[strum(to_string = "Nix Builder View")]
    BuilderView,
    #[strum(to_string = "Birds Eye View")]
    BirdsEyeView,
}

impl SelectedTab {
    fn title(self) -> Line<'static> {
        format!("  {self}  ").into()
    }

    fn previous(self) -> Self {
        let current_index: usize = self as usize;
        let previous_index = (current_index + SelectedTab::COUNT)
            .saturating_sub(1)
            % SelectedTab::COUNT;
        Self::from_repr(previous_index).unwrap_or(self)
    }

    fn next(self) -> Self {
        let current_index = self as usize;
        let next_index = current_index.saturating_add(1) % SelectedTab::COUNT;
        Self::from_repr(next_index).unwrap_or(self)
    }
}

#[derive(Default, Debug)]
pub struct App {
    builder_view: BuilderViewState,
    birds_eye_view: BirdsEyeViewState,
    tab_selected: SelectedTab,
    // I hate this. Stream updates instead. Better when we separate out to the
    // daemon
    cur_info_builds: HashMap<u64, BuildJob>,
    cur_info: HashMap<String, BTreeSet<ProcMetadata>>,
}

#[derive(Default, Debug)]
pub struct BirdsEyeViewState {
    man_toggle: bool,
}

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

#[tokio::main]
pub async fn main() {
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

    // construct_everything();

    run().await.unwrap();
}

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "The fully qualified path of the socket to read from. See the \
             README for more details. Without this flag, the eagle view will \
             not work."
)]
struct Args {
    /// Path to the Unix domain socket to connect to
    #[arg(short, long)]
    socket: Option<String>,
}

async fn run() -> Result<()> {
    let is_shutdown = Arc::new(AtomicBool::new(false));
    let local_is_shutdown = is_shutdown.clone();
    let local_is_shutdown2 = is_shutdown.clone();

    let (tx_jobs, recv_job_updates) = watch::channel(Default::default());
    let args = Args::parse();
    let maybe_jh = args.socket.map(|socket| {
        tokio::spawn(async move {
            handle_daemon_info(
                socket.into(),
                0o660,
                local_is_shutdown2,
                tx_jobs,
            )
            .await;
        })
    });

    let mut terminal = setup_terminal()?;

    // create app and run it
    let mut app = App::default();

    let (tx, recv_proc_updates) = watch::channel(Default::default());
    let t_handle = tokio::spawn(async move {
        while !local_is_shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            let user_map_new = get_active_users_and_pids();
            // TODO should do some sort of error checking
            let _ = tx.send(user_map_new);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    let main_app_handle = tokio::spawn(async move {
        let res = event_loop(
            &mut terminal,
            app,
            is_shutdown,
            recv_proc_updates,
            recv_job_updates,
        )
        .await;
        // restore terminal
        disable_raw_mode().unwrap();
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )
        .unwrap();
        terminal.show_cursor().unwrap();
        if let Err(err) = res {
            println!("{err:?}");
        }
    });

    let mut handles = vec![t_handle, main_app_handle];

    if let Some(jh) = maybe_jh {
        handles.push(jh);
    }

    join_all(handles).await;

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
        let _ =
            execute!(io::stderr(), LeaveAlternateScreen, DisableMouseCapture);

        panic_hook(panic);
    }));

    Ok(terminal)
}
