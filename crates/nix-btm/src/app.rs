use std::{
    collections::{BTreeSet, HashMap},
    error::Error,
    io::{self, Stdout},
    panic,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crossterm::{
    event::DisableMouseCapture,
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use ratatui::{
    backend::CrosstermBackend, style::Style, text::Line,
    widgets::ScrollbarState,
};
use strum::{Display, EnumCount, EnumIter, FromRepr};
use tui_tree_widget::TreeState;

use crate::{
    get_stats::ProcMetadata,
    handle_internal_json::JobsStateInner,
    tree_generation::PruneType,
    ui::{
        BORDER_STYLE_SELECTED, BORDER_STYLE_UNSELECTED, TITLE_STYLE_SELECTED,
        TITLE_STYLE_UNSELECTED,
    },
};

pub type Terminal = ratatui::Terminal<CrosstermBackend<Stdout>>;
pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

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
    #[strum(to_string = "Nix Builder View 👷")]
    BuilderView,
    #[strum(to_string = "Eagle Eye View 🦅")]
    EagleEyeView,
    #[strum(to_string = "Build Job View 💼")]
    BuildJobView,
}

impl SelectedTab {
    pub fn title(self) -> Line<'static> {
        format!("  {self}  ").into()
    }

    pub fn previous(self) -> Self {
        let current_index: usize = self as usize;
        let previous_index = (current_index + SelectedTab::COUNT)
            .saturating_sub(1)
            % SelectedTab::COUNT;
        Self::from_repr(previous_index).unwrap_or(self)
    }

    pub fn next(self) -> Self {
        let current_index = self as usize;
        let next_index = current_index.saturating_add(1) % SelectedTab::COUNT;
        Self::from_repr(next_index).unwrap_or(self)
    }
}

#[derive(Default, Debug)]
pub struct App {
    pub builder_view: BuilderViewState,
    pub eagle_eye_view: EagleEyeViewState,
    pub build_job_view: BuildJobViewState,
    pub tab_selected: SelectedTab,
    // I hate this. Stream updates instead. Better when we separate out to the
    // daemon
    pub cur_info_builds: JobsStateInner,
    pub cur_info: HashMap<String, BTreeSet<ProcMetadata>>,
}

#[derive(Default, Copy, Clone, Debug)]
pub enum TreeToggle {
    Open,
    Closed,
    #[default]
    Never,
}

#[derive(Default, Debug)]
pub struct EagleEyeViewState {
    pub man_toggle: bool,
    pub active_only: PruneType,
    pub state: TreeState<String>,
    pub perform_toggle: bool,
    pub last_toggle: TreeToggle,
}

#[derive(Default, Debug)]
pub struct BuildJobViewState {
    pub man_toggle: bool,
}

#[derive(Default, Debug)]
pub struct BuilderViewState {
    pub vertical_scroll_state: ScrollbarState,
    pub vertical_scroll: usize,
    pub horizontal_scroll: usize,
    pub state: TreeState<String>,
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
