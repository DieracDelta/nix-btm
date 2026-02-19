//! Application state for the TUI.

use std::collections::{HashMap, HashSet};
use std::ops::RangeInclusive;

use nix_btm_common::dep_graph::DepGraph;
use nix_btm_common::types::{BtmSnapshot, Build, CompletedBuild, ProcessInfo, Progress, RemoteMachine};
use ratatui::widgets::TableState;

use crate::client::BtmClient;
use crate::ui;

/// Which pane currently has keyboard focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusPane {
    Top,
    Bottom,
}

/// Identifies what a dep tree row represents (for fold/selection dispatch).
#[derive(Clone, Debug)]
pub enum DepTreeRowId {
    /// A command root row (keyed by command_line).
    CommandRoot(Option<String>),
    /// A derivation node row (keyed by drv_path).
    DrvNode(String),
}

/// A row in the flattened processes tree view.
pub enum ProcRow {
    /// Top-level nix command (level 0).
    NixCommand { pid: u32, cmdline: String },
    /// A derivation under a nix command (level 1).
    Derivation { activity_id: u64, drv: String, is_frozen: bool },
    /// An OS process inside a build's cgroup (level 2+).
    Process { info: ProcessInfo, activity_id: u64 },
}

pub struct App {
    pub snapshot: Option<BtmSnapshot>,
    /// Builds in depth-first tree order (with Realise nodes collapsed).
    pub builds: Vec<Build>,
    /// Activity IDs whose original parent was a collapsed Realise node.
    pub had_realise_parent: HashSet<u64>,
    /// Activity IDs whose FileTransfer child was hoisted (show as "download" step).
    pub hoisted_download: HashSet<u64>,
    /// Hoisted progress: activity_id → Progress from the hidden FileTransfer child.
    pub hoisted_progress: HashMap<u64, Progress>,
    /// Activity IDs that serve as command roots (rendered as bold label-only rows).
    pub command_root_ids: HashSet<u64>,
    /// Currently selected index in the builds list.
    pub selected: usize,
    /// Whether the log panel is visible.
    pub show_log: bool,
    /// Log lines for the selected build.
    pub log_lines: Vec<String>,
    /// Scroll offset for the log panel (0 = bottom/latest).
    pub log_scroll: usize,
    /// Whether to show history instead of active builds.
    pub show_history: bool,
    /// Whether to show the machines panel.
    pub show_machines: bool,
    /// Whether to show the dependency tree view instead of active builds.
    pub show_dep_tree: bool,
    /// Dependency graphs from the daemon snapshot.
    pub dep_graphs: Vec<DepGraph>,
    /// Total number of rows in the flattened dep tree (for scrolling).
    pub dep_tree_row_count: usize,
    /// Currently selected row index in the dep tree view.
    pub dep_tree_selected: usize,
    /// Transient status message shown at the bottom (with expiry time).
    pub status_message: Option<(String, std::time::Instant)>,
    /// Whether a `g` keypress is pending (for `gg` two-key chord).
    pub pending_g: bool,
    /// Whether a `z` keypress is pending (for `z-c`/`z-o` fold chords).
    pub pending_z: bool,
    /// Table area height (updated each render), used for half-page scrolling.
    pub visible_rows: usize,
    /// Whether to show all roots including finished ones (default false = hide finished).
    pub show_all_roots: bool,
    /// Whether to show finished dep tree graphs (independent of show_all_roots).
    pub show_dep_history: bool,
    /// Maps visible row index → index in `self.builds` (for active builds view filtering).
    pub visible_build_indices: Vec<usize>,
    /// Command root activity_ids that are collapsed in the builds view.
    pub folded_build_roots: HashSet<u64>,
    /// Drv paths that are collapsed in the dep tree view.
    pub folded_nodes: HashSet<String>,
    /// Command roots that are collapsed in the dep tree view.
    pub folded_dep_roots: HashSet<String>,
    /// Maps dep tree row index → row identity (command root or drv node).
    pub dep_tree_drv_at_row: Vec<DepTreeRowId>,
    /// Whether currently in text input mode (search or filter).
    pub input_mode: bool,
    /// The current search/filter query string.
    pub input_query: String,
    /// Whether the input is for search (dep tree) vs filter (builds).
    pub input_is_search: bool,
    /// Row indices in dep tree matching current search query.
    pub search_matches: Vec<usize>,
    /// Current position within search_matches (for n/N cycling).
    pub search_match_idx: usize,
    /// Active filter string for builds view (persists after exiting input mode).
    pub builds_filter: String,
    /// Whether visual (multi-select) mode is active.
    pub visual_mode: bool,
    /// Row index where visual mode was entered (anchor for selection range).
    pub visual_anchor: usize,
    /// Whether the signal prompt is showing (waiting for signal key after K).
    pub action_prompt: bool,
    /// Whether to show the processes view.
    pub show_processes: bool,
    /// Currently selected index in the processes view (into visible rows).
    pub proc_selected: usize,
    /// Flattened process tree rows for rendering.
    pub proc_rows: Vec<ProcRow>,
    /// Visible proc row indices (after folding).
    pub visible_proc_indices: Vec<usize>,
    /// Folded NixCommand PIDs in processes view.
    pub folded_proc_pids: HashSet<u32>,
    /// Folded derivation activity_ids in processes view.
    pub folded_proc_builds: HashSet<u64>,
    /// Whether the yank prompt is showing (waiting for field key after y).
    pub yank_prompt: bool,
    /// Whether the UI needs a redraw (set on data refresh or key press).
    pub dirty: bool,
    /// Persistent table state for builds view (retains scroll offset across renders).
    pub builds_state: TableState,
    /// Persistent table state for dep tree view.
    pub dep_tree_state: TableState,
    /// Persistent table state for processes view.
    pub proc_state: TableState,
    /// Persistent table state for history view.
    pub history_state: TableState,
    /// Currently selected index in the history panel.
    pub history_selected: usize,
    /// Whether the help overlay is showing.
    pub show_help: bool,
    /// Which pane (top or bottom) has keyboard focus.
    pub focused_pane: FocusPane,
    /// Activity IDs of builds that came from recent_history (for styling).
    pub history_build_ids: HashSet<u64>,
    /// Completion metadata for history builds: activity_id → (success, duration_us).
    pub completed_meta: HashMap<u64, (bool, u64)>,
}

impl App {
    pub fn new() -> Self {
        Self {
            snapshot: None,
            builds: Vec::new(),
            had_realise_parent: HashSet::new(),
            hoisted_download: HashSet::new(),
            hoisted_progress: HashMap::new(),
            command_root_ids: HashSet::new(),
            selected: 0,
            show_log: false,
            log_lines: Vec::new(),
            log_scroll: 0,
            show_history: false,
            show_machines: false,
            show_dep_tree: false,
            dep_graphs: Vec::new(),
            dep_tree_row_count: 0,
            dep_tree_selected: 0,
            status_message: None,
            pending_g: false,
            pending_z: false,
            visible_rows: 20,
            show_all_roots: false,
            show_dep_history: false,
            visible_build_indices: Vec::new(),
            folded_build_roots: HashSet::new(),
            folded_nodes: HashSet::new(),
            folded_dep_roots: HashSet::new(),
            dep_tree_drv_at_row: Vec::new(),
            input_mode: false,
            input_query: String::new(),
            input_is_search: true,
            search_matches: Vec::new(),
            search_match_idx: 0,
            builds_filter: String::new(),
            visual_mode: false,
            visual_anchor: 0,
            action_prompt: false,
            show_processes: false,
            proc_selected: 0,
            proc_rows: Vec::new(),
            visible_proc_indices: Vec::new(),
            folded_proc_pids: HashSet::new(),
            folded_proc_builds: HashSet::new(),
            yank_prompt: false,
            dirty: true,
            builds_state: TableState::default(),
            dep_tree_state: TableState::default(),
            proc_state: TableState::default(),
            history_state: TableState::default(),
            history_selected: 0,
            show_help: false,
            focused_pane: FocusPane::Top,
            history_build_ids: HashSet::new(),
            completed_meta: HashMap::new(),
        }
    }

    pub async fn refresh(&mut self, client: &mut BtmClient) -> anyhow::Result<()> {
        // Clear transient status message after it expires.
        if let Some((_, expires_at)) = &self.status_message {
            if std::time::Instant::now() >= *expires_at {
                self.status_message = None;
            }
        }

        let mut snapshot = client.get_snapshot().await?;

        let mut builds: Vec<Build> = snapshot.active_builds.values().cloned().collect();

        // Merge history builds into the list when show_all_roots is on.
        self.history_build_ids.clear();
        self.completed_meta.clear();
        if self.show_all_roots {
            for completed in &snapshot.recent_history {
                let aid = completed.build.activity_id;
                if !snapshot.active_builds.contains_key(&aid) {
                    builds.push(completed.build.clone());
                    self.history_build_ids.insert(aid);
                    self.completed_meta.insert(
                        aid,
                        (completed.success, completed.duration.as_micros() as u64),
                    );
                }
            }
        }

        let tree_result = ui::tree_order_builds(builds);

        self.builds = tree_result.builds;
        self.had_realise_parent = tree_result.had_realise_parent;
        self.hoisted_download = tree_result.hoisted_download;
        self.hoisted_progress = tree_result.hoisted_progress;
        self.command_root_ids = tree_result.command_root_ids;
        self.dep_graphs = std::mem::take(&mut snapshot.dep_graphs);

        // Build process tree rows from snapshot data.
        self.proc_rows = ui::build_process_tree(&snapshot, &self.builds, &self.command_root_ids);

        self.snapshot = Some(snapshot);

        // Keep selection in bounds.
        if !self.builds.is_empty() && self.selected >= self.builds.len() {
            self.selected = self.builds.len() - 1;
        }

        // Recompute visible proc indices (applying fold state).
        self.recompute_visible_procs();

        // Keep proc_selected in bounds.
        if !self.visible_proc_indices.is_empty() && self.proc_selected >= self.visible_proc_indices.len() {
            self.proc_selected = self.visible_proc_indices.len() - 1;
        }

        // Keep history_selected in bounds.
        let hist_len = self.history().len();
        if hist_len > 0 && self.history_selected >= hist_len {
            self.history_selected = hist_len - 1;
        }

        // Sync persistent table states after clamping.
        self.builds_state.select(Some(self.selected));
        self.proc_state.select(Some(self.proc_selected));
        self.dep_tree_state.select(Some(self.dep_tree_selected));
        self.history_state.select(Some(self.history_selected));

        // Refresh log if panel is open.
        if self.show_log {
            if let Some(id) = self.selected_build_id() {
                self.log_lines = client.get_build_log(id, 50).await.unwrap_or_default();
            }
        }

        self.dirty = true;
        Ok(())
    }

    pub fn select_prev(&mut self) {
        if self.show_processes {
            self.proc_selected = self.proc_selected.saturating_sub(1);
            self.proc_state.select(Some(self.proc_selected));
        } else if self.show_dep_tree {
            if self.dep_tree_selected > 0 {
                self.dep_tree_selected -= 1;
            }
            self.dep_tree_state.select(Some(self.dep_tree_selected));
        } else if self.selected > 0 {
            self.selected -= 1;
            self.builds_state.select(Some(self.selected));
        }
    }

    pub fn select_next(&mut self) {
        if self.show_processes {
            let len = self.visible_proc_indices.len();
            if len > 0 && self.proc_selected < len - 1 {
                self.proc_selected += 1;
            }
            self.proc_state.select(Some(self.proc_selected));
        } else if self.show_dep_tree {
            if self.dep_tree_row_count > 0 && self.dep_tree_selected < self.dep_tree_row_count - 1 {
                self.dep_tree_selected += 1;
            }
            self.dep_tree_state.select(Some(self.dep_tree_selected));
        } else {
            let len = self.visible_build_indices.len();
            if len > 0 && self.selected < len - 1 {
                self.selected += 1;
            }
            self.builds_state.select(Some(self.selected));
        }
    }

    pub fn select_top(&mut self) {
        if self.show_processes {
            self.proc_selected = 0;
            self.proc_state.select(Some(0));
        } else if self.show_dep_tree {
            self.dep_tree_selected = 0;
            self.dep_tree_state.select(Some(0));
        } else {
            self.selected = 0;
            self.builds_state.select(Some(0));
        }
    }

    pub fn select_bottom(&mut self) {
        if self.show_processes {
            let len = self.visible_proc_indices.len();
            if len > 0 {
                self.proc_selected = len - 1;
            }
            self.proc_state.select(Some(self.proc_selected));
        } else if self.show_dep_tree {
            if self.dep_tree_row_count > 0 {
                self.dep_tree_selected = self.dep_tree_row_count - 1;
            }
            self.dep_tree_state.select(Some(self.dep_tree_selected));
        } else {
            let len = self.visible_build_indices.len();
            if len > 0 {
                self.selected = len - 1;
            }
            self.builds_state.select(Some(self.selected));
        }
    }

    pub fn half_page_up(&mut self) {
        let delta = self.visible_rows / 2;
        if self.show_processes {
            self.proc_selected = self.proc_selected.saturating_sub(delta);
            self.proc_state.select(Some(self.proc_selected));
        } else if self.show_dep_tree {
            self.dep_tree_selected = self.dep_tree_selected.saturating_sub(delta);
            self.dep_tree_state.select(Some(self.dep_tree_selected));
        } else {
            self.selected = self.selected.saturating_sub(delta);
            self.builds_state.select(Some(self.selected));
        }
    }

    pub fn half_page_down(&mut self) {
        let delta = self.visible_rows / 2;
        if self.show_processes {
            let len = self.visible_proc_indices.len();
            if len > 0 {
                self.proc_selected = (self.proc_selected + delta).min(len - 1);
            }
            self.proc_state.select(Some(self.proc_selected));
        } else if self.show_dep_tree {
            let max = if self.dep_tree_row_count > 0 {
                self.dep_tree_row_count - 1
            } else {
                0
            };
            self.dep_tree_selected = (self.dep_tree_selected + delta).min(max);
            self.dep_tree_state.select(Some(self.dep_tree_selected));
        } else {
            let len = self.visible_build_indices.len();
            if len > 0 {
                self.selected = (self.selected + delta).min(len - 1);
            }
            self.builds_state.select(Some(self.selected));
        }
    }

    pub fn selected_build_id(&self) -> Option<u64> {
        self.visible_build_indices
            .get(self.selected)
            .and_then(|&i| self.builds.get(i))
            .map(|b| b.activity_id)
    }

    pub fn selected_build(&self) -> Option<&Build> {
        self.visible_build_indices
            .get(self.selected)
            .and_then(|&i| self.builds.get(i))
    }

    pub fn toggle_show_all(&mut self) {
        self.show_all_roots = !self.show_all_roots;
    }

    pub fn toggle_dep_history(&mut self) {
        self.show_dep_history = !self.show_dep_history;
    }

    pub fn toggle_fold(&mut self) {
        if let Some(row_id) = self.dep_tree_drv_at_row.get(self.dep_tree_selected) {
            match row_id {
                DepTreeRowId::DrvNode(drv_path) => {
                    let drv_path = drv_path.clone();
                    if !self.folded_nodes.remove(&drv_path) {
                        self.folded_nodes.insert(drv_path);
                    }
                }
                DepTreeRowId::CommandRoot(Some(cmd)) => {
                    let cmd = cmd.clone();
                    if !self.folded_dep_roots.remove(&cmd) {
                        self.folded_dep_roots.insert(cmd);
                    }
                }
                DepTreeRowId::CommandRoot(None) => {}
            }
        }
    }

    pub fn fold_close(&mut self) {
        if let Some(row_id) = self.dep_tree_drv_at_row.get(self.dep_tree_selected) {
            match row_id {
                DepTreeRowId::DrvNode(drv_path) => {
                    self.folded_nodes.insert(drv_path.clone());
                }
                DepTreeRowId::CommandRoot(Some(cmd)) => {
                    self.folded_dep_roots.insert(cmd.clone());
                }
                DepTreeRowId::CommandRoot(None) => {}
            }
        }
    }

    pub fn fold_open(&mut self) {
        if let Some(row_id) = self.dep_tree_drv_at_row.get(self.dep_tree_selected) {
            match row_id {
                DepTreeRowId::DrvNode(drv_path) => {
                    self.folded_nodes.remove(drv_path);
                }
                DepTreeRowId::CommandRoot(Some(cmd)) => {
                    self.folded_dep_roots.remove(cmd);
                }
                DepTreeRowId::CommandRoot(None) => {}
            }
        }
    }

    /// Get the command root activity_id for the currently selected build.
    /// Returns Some(id) if the selected build is a command root, or walks
    /// up the parent chain to find the nearest command root ancestor.
    fn selected_build_root_id(&self) -> Option<u64> {
        let &build_idx = self.visible_build_indices.get(self.selected)?;
        let build = self.builds.get(build_idx)?;
        if self.command_root_ids.contains(&build.activity_id) {
            return Some(build.activity_id);
        }
        // Walk parent chain to find root.
        let id_to_idx: std::collections::HashMap<u64, usize> = self.builds
            .iter()
            .enumerate()
            .map(|(i, b)| (b.activity_id, i))
            .collect();
        let mut cur_id = build.activity_id;
        loop {
            let cur = id_to_idx.get(&cur_id).and_then(|&i| self.builds.get(i))?;
            match cur.parent_id {
                Some(pid) if self.command_root_ids.contains(&pid) => return Some(pid),
                Some(pid) if id_to_idx.contains_key(&pid) => cur_id = pid,
                _ => return None,
            }
        }
    }

    /// Toggle fold on the selected build's command root.
    pub fn build_toggle_fold(&mut self) {
        if let Some(root_id) = self.selected_build_root_id() {
            if !self.folded_build_roots.remove(&root_id) {
                self.folded_build_roots.insert(root_id);
            }
        }
    }

    /// Fold close the selected build's command root.
    pub fn build_fold_close(&mut self) {
        if let Some(root_id) = self.selected_build_root_id() {
            self.folded_build_roots.insert(root_id);
        }
    }

    /// Fold open the selected build's command root.
    pub fn build_fold_open(&mut self) {
        if let Some(root_id) = self.selected_build_root_id() {
            self.folded_build_roots.remove(&root_id);
        }
    }

    /// Fold all command roots in the builds view.
    pub fn build_fold_all(&mut self) {
        for &id in &self.command_root_ids {
            self.folded_build_roots.insert(id);
        }
    }

    /// Unfold all command roots in the builds view.
    pub fn build_unfold_all(&mut self) {
        self.folded_build_roots.clear();
    }

    /// Fold all nodes in the dep tree view.
    pub fn dep_tree_fold_all(&mut self) {
        for row_id in &self.dep_tree_drv_at_row {
            match row_id {
                DepTreeRowId::DrvNode(drv_path) => {
                    self.folded_nodes.insert(drv_path.clone());
                }
                DepTreeRowId::CommandRoot(Some(cmd)) => {
                    self.folded_dep_roots.insert(cmd.clone());
                }
                DepTreeRowId::CommandRoot(None) => {}
            }
        }
    }

    /// Unfold all nodes in the dep tree view.
    pub fn dep_tree_unfold_all(&mut self) {
        self.folded_nodes.clear();
        self.folded_dep_roots.clear();
    }

    /// Fold all nodes in the processes view.
    pub fn proc_fold_all(&mut self) {
        for row in &self.proc_rows {
            match row {
                ProcRow::NixCommand { pid, .. } => {
                    self.folded_proc_pids.insert(*pid);
                }
                ProcRow::Derivation { activity_id, .. } => {
                    self.folded_proc_builds.insert(*activity_id);
                }
                ProcRow::Process { .. } => {}
            }
        }
        self.recompute_visible_procs();
        if !self.visible_proc_indices.is_empty() && self.proc_selected >= self.visible_proc_indices.len() {
            self.proc_selected = self.visible_proc_indices.len() - 1;
        }
        self.proc_state.select(Some(self.proc_selected));
    }

    /// Unfold all nodes in the processes view.
    pub fn proc_unfold_all(&mut self) {
        self.folded_proc_pids.clear();
        self.folded_proc_builds.clear();
        self.recompute_visible_procs();
    }

    pub fn toggle_log_panel(&mut self) {
        self.show_log = !self.show_log;
        self.log_scroll = 0;
        if !self.has_bottom_pane() {
            self.focused_pane = FocusPane::Top;
        }
    }

    pub fn log_scroll_up(&mut self) {
        if self.show_log && !self.log_lines.is_empty() {
            self.log_scroll = self.log_scroll.saturating_add(1)
                .min(self.log_lines.len().saturating_sub(1));
        }
    }

    pub fn log_scroll_down(&mut self) {
        if self.show_log {
            self.log_scroll = self.log_scroll.saturating_sub(1);
        }
    }

    pub fn toggle_history(&mut self) {
        self.show_history = !self.show_history;
        if !self.has_bottom_pane() {
            self.focused_pane = FocusPane::Top;
        }
    }

    pub fn history_scroll_up(&mut self) {
        self.history_selected = self.history_selected.saturating_sub(1);
        self.history_state.select(Some(self.history_selected));
    }

    pub fn history_scroll_down(&mut self) {
        let len = self.history().len();
        if len > 0 && self.history_selected < len - 1 {
            self.history_selected += 1;
        }
        self.history_state.select(Some(self.history_selected));
    }

    pub fn toggle_machines(&mut self) {
        self.show_machines = !self.show_machines;
        if !self.has_bottom_pane() {
            self.focused_pane = FocusPane::Top;
        }
    }

    pub fn toggle_dep_tree(&mut self) {
        self.show_dep_tree = !self.show_dep_tree;
    }

    pub fn toggle_processes(&mut self) {
        self.show_processes = !self.show_processes;
        if self.show_processes {
            self.show_dep_tree = false;
            self.proc_selected = 0;
            self.proc_state.select(Some(0));
        }
    }

    /// Recompute which proc rows are visible after applying fold state.
    pub fn recompute_visible_procs(&mut self) {
        let mut visible = Vec::new();
        let mut skip_under_pid: Option<u32> = None;
        let mut skip_under_build: Option<u64> = None;

        for (i, row) in self.proc_rows.iter().enumerate() {
            match row {
                ProcRow::NixCommand { pid, .. } => {
                    skip_under_pid = None;
                    skip_under_build = None;
                    visible.push(i);
                    if self.folded_proc_pids.contains(pid) {
                        skip_under_pid = Some(*pid);
                    }
                }
                ProcRow::Derivation { activity_id, .. } => {
                    skip_under_build = None;
                    if skip_under_pid.is_some() {
                        continue;
                    }
                    visible.push(i);
                    if self.folded_proc_builds.contains(activity_id) {
                        skip_under_build = Some(*activity_id);
                    }
                }
                ProcRow::Process { .. } => {
                    if skip_under_pid.is_some() || skip_under_build.is_some() {
                        continue;
                    }
                    visible.push(i);
                }
            }
        }

        self.visible_proc_indices = visible;
    }

    /// Toggle fold on the currently selected proc row.
    pub fn proc_toggle_fold(&mut self) {
        if let Some(&row_idx) = self.visible_proc_indices.get(self.proc_selected) {
            match &self.proc_rows[row_idx] {
                ProcRow::NixCommand { pid, .. } => {
                    if !self.folded_proc_pids.remove(pid) {
                        self.folded_proc_pids.insert(*pid);
                    }
                }
                ProcRow::Derivation { activity_id, .. } => {
                    if !self.folded_proc_builds.remove(activity_id) {
                        self.folded_proc_builds.insert(*activity_id);
                    }
                }
                ProcRow::Process { .. } => {}
            }
            self.recompute_visible_procs();
            // Clamp selection.
            if !self.visible_proc_indices.is_empty() && self.proc_selected >= self.visible_proc_indices.len() {
                self.proc_selected = self.visible_proc_indices.len() - 1;
            }
            self.proc_state.select(Some(self.proc_selected));
        }
    }

    /// Fold close the currently selected proc row.
    pub fn proc_fold_close(&mut self) {
        if let Some(&row_idx) = self.visible_proc_indices.get(self.proc_selected) {
            match &self.proc_rows[row_idx] {
                ProcRow::NixCommand { pid, .. } => {
                    self.folded_proc_pids.insert(*pid);
                }
                ProcRow::Derivation { activity_id, .. } => {
                    self.folded_proc_builds.insert(*activity_id);
                }
                ProcRow::Process { .. } => {}
            }
            self.recompute_visible_procs();
            if !self.visible_proc_indices.is_empty() && self.proc_selected >= self.visible_proc_indices.len() {
                self.proc_selected = self.visible_proc_indices.len() - 1;
            }
            self.proc_state.select(Some(self.proc_selected));
        }
    }

    /// Fold open the currently selected proc row.
    pub fn proc_fold_open(&mut self) {
        if let Some(&row_idx) = self.visible_proc_indices.get(self.proc_selected) {
            match &self.proc_rows[row_idx] {
                ProcRow::NixCommand { pid, .. } => {
                    self.folded_proc_pids.remove(pid);
                }
                ProcRow::Derivation { activity_id, .. } => {
                    self.folded_proc_builds.remove(activity_id);
                }
                ProcRow::Process { .. } => {}
            }
            self.recompute_visible_procs();
        }
    }

    /// Get the activity_id for the currently selected process row (for K actions).
    /// NixCommand rows return all child build activity_ids.
    /// Derivation rows return their own activity_id.
    /// Process rows return their parent build's activity_id.
    pub fn selected_proc_activity_ids(&self) -> Vec<u64> {
        let row_idx = match self.visible_proc_indices.get(self.proc_selected) {
            Some(&i) => i,
            None => return Vec::new(),
        };
        match self.proc_rows.get(row_idx) {
            Some(ProcRow::NixCommand { pid, .. }) => {
                // Return all derivation activity_ids under this nix command.
                let mut ids = Vec::new();
                for row in &self.proc_rows {
                    if let ProcRow::Derivation { activity_id, .. } = row {
                        // Check if this derivation belongs to the same nix command.
                        if let Some(snapshot) = &self.snapshot {
                            if let Some(build) = snapshot.active_builds.get(activity_id) {
                                if build.user_pid == Some(*pid) {
                                    ids.push(*activity_id);
                                }
                            }
                        }
                    }
                }
                ids
            }
            Some(ProcRow::Derivation { activity_id, .. }) => vec![*activity_id],
            Some(ProcRow::Process { activity_id, .. }) => vec![*activity_id],
            None => Vec::new(),
        }
    }

    pub fn machines(&self) -> &[RemoteMachine] {
        self.snapshot
            .as_ref()
            .map(|s| s.machines.as_slice())
            .unwrap_or(&[])
    }

    #[allow(dead_code)]
    pub fn history(&self) -> &[CompletedBuild] {
        self.snapshot
            .as_ref()
            .map(|s| s.recent_history.as_slice())
            .unwrap_or(&[])
    }

    pub fn enter_search(&mut self) {
        self.input_mode = true;
        self.input_is_search = true;
        self.input_query.clear();
        self.search_matches.clear();
        self.search_match_idx = 0;
    }

    pub fn enter_filter(&mut self) {
        self.input_mode = true;
        self.input_is_search = false;
        self.input_query = self.builds_filter.clone();
    }

    pub fn exit_input(&mut self) {
        self.input_mode = false;
        if !self.input_is_search {
            self.builds_filter = self.input_query.clone();
        }
    }

    pub fn cancel_input(&mut self) {
        self.input_mode = false;
        if !self.input_is_search {
            self.builds_filter.clear();
        }
        self.input_query.clear();
        self.search_matches.clear();
        self.search_match_idx = 0;
    }

    pub fn input_push(&mut self, c: char) {
        self.input_query.push(c);
    }

    pub fn input_pop(&mut self) {
        self.input_query.pop();
    }

    pub fn search_next(&mut self) {
        if !self.search_matches.is_empty() {
            self.search_match_idx = (self.search_match_idx + 1) % self.search_matches.len();
            self.jump_to_search_match();
        }
    }

    pub fn search_prev(&mut self) {
        if !self.search_matches.is_empty() {
            if self.search_match_idx == 0 {
                self.search_match_idx = self.search_matches.len() - 1;
            } else {
                self.search_match_idx -= 1;
            }
            self.jump_to_search_match();
        }
    }

    /// Jump to the current search match in the appropriate view.
    fn jump_to_search_match(&mut self) {
        let idx = self.search_matches[self.search_match_idx];
        if self.focused_pane == FocusPane::Bottom {
            if self.show_history {
                self.history_selected = idx;
                self.history_state.select(Some(idx));
            } else if self.show_log {
                self.log_scroll = idx;
            }
        } else if self.show_processes {
            self.proc_selected = idx;
            self.proc_state.select(Some(idx));
        } else if self.show_dep_tree {
            self.dep_tree_selected = idx;
            self.dep_tree_state.select(Some(idx));
        } else {
            self.selected = idx;
            self.builds_state.select(Some(idx));
        }
    }

    pub fn enter_visual(&mut self) {
        if !self.show_dep_tree {
            self.visual_mode = true;
            self.visual_anchor = self.selected;
            self.focused_pane = FocusPane::Top;
        }
    }

    pub fn exit_visual(&mut self) {
        self.visual_mode = false;
        self.action_prompt = false;
        self.yank_prompt = false;
    }

    /// Range lo..=hi between anchor and cursor.
    pub fn visual_selection_range(&self) -> RangeInclusive<usize> {
        let lo = self.visual_anchor.min(self.selected);
        let hi = self.visual_anchor.max(self.selected);
        lo..=hi
    }

    /// Collect activity IDs of selected builds (skip command roots).
    pub fn selected_activity_ids(&self) -> Vec<u64> {
        if self.show_processes {
            return self.selected_proc_activity_ids();
        }
        if self.visual_mode {
            let range = self.visual_selection_range();
            self.visible_build_indices
                .iter()
                .enumerate()
                .filter(|(vis_idx, _)| range.contains(vis_idx))
                .filter_map(|(_, &build_idx)| {
                    let build = &self.builds[build_idx];
                    if self.command_root_ids.contains(&build.activity_id) {
                        None
                    } else {
                        Some(build.activity_id)
                    }
                })
                .collect()
        } else {
            self.selected_build_id().into_iter().collect()
        }
    }

    /// Set a status message that persists for the given duration.
    pub fn set_status(&mut self, msg: String, duration: std::time::Duration) {
        self.status_message = Some((msg, std::time::Instant::now() + duration));
    }

    /// Whether any of the selected builds have a cgroup path assigned.
    pub fn selected_have_cgroups(&self) -> bool {
        let ids = self.selected_activity_ids();
        if let Some(snapshot) = &self.snapshot {
            ids.iter().any(|id| {
                snapshot.active_builds.get(id).is_some_and(|b| b.cgroup_path.is_some())
            })
        } else {
            ids.iter().any(|id| {
                self.builds.iter().any(|b| b.activity_id == *id && b.cgroup_path.is_some())
            })
        }
    }

    pub fn show_action_prompt(&mut self) {
        self.action_prompt = true;
    }

    pub fn cancel_action_prompt(&mut self) {
        self.action_prompt = false;
    }

    pub fn show_yank_prompt(&mut self) {
        self.yank_prompt = true;
    }

    pub fn cancel_yank_prompt(&mut self) {
        self.yank_prompt = false;
    }

    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    /// Whether any bottom pane is currently visible.
    pub fn has_bottom_pane(&self) -> bool {
        self.show_log || self.show_history || self.show_machines
    }

    /// Cycle focus between Top and Bottom panes.
    pub fn cycle_focus(&mut self) {
        if !self.has_bottom_pane() {
            return;
        }
        self.focused_pane = match self.focused_pane {
            FocusPane::Top => FocusPane::Bottom,
            FocusPane::Bottom => FocusPane::Top,
        };
    }

    /// Navigate the bottom pane up (toward older entries).
    pub fn bottom_select_prev(&mut self) {
        if self.show_log {
            self.log_scroll_up();
        } else if self.show_history {
            self.history_scroll_up();
        }
        // machines: no-op (short list)
    }

    /// Navigate the bottom pane down (toward newer entries).
    pub fn bottom_select_next(&mut self) {
        if self.show_log {
            self.log_scroll_down();
        } else if self.show_history {
            self.history_scroll_down();
        }
    }

    /// Jump to the top of the bottom pane.
    pub fn bottom_select_top(&mut self) {
        if self.show_log {
            // gg in log = oldest = max scroll
            self.log_scroll = self.log_lines.len().saturating_sub(1);
        } else if self.show_history {
            self.history_selected = 0;
            self.history_state.select(Some(0));
        }
    }

    /// Jump to the bottom of the bottom pane.
    pub fn bottom_select_bottom(&mut self) {
        if self.show_log {
            // G in log = latest = scroll 0
            self.log_scroll = 0;
        } else if self.show_history {
            let len = self.history().len();
            if len > 0 {
                self.history_selected = len - 1;
            }
            self.history_state.select(Some(self.history_selected));
        }
    }

    /// Half-page up in the bottom pane.
    pub fn bottom_half_page_up(&mut self) {
        let delta = self.visible_rows / 2;
        if self.show_log {
            for _ in 0..delta {
                self.log_scroll_up();
            }
        } else if self.show_history {
            self.history_selected = self.history_selected.saturating_sub(delta);
            self.history_state.select(Some(self.history_selected));
        }
    }

    /// Half-page down in the bottom pane.
    pub fn bottom_half_page_down(&mut self) {
        let delta = self.visible_rows / 2;
        if self.show_log {
            for _ in 0..delta {
                self.log_scroll_down();
            }
        } else if self.show_history {
            let len = self.history().len();
            if len > 0 {
                self.history_selected = (self.history_selected + delta).min(len - 1);
            }
            self.history_state.select(Some(self.history_selected));
        }
    }

    /// Extract a field value from the builds view for yanking.
    /// In visual mode, collects from all selected rows, newline-separated.
    pub fn yank_build_field(&self, field: char) -> Option<String> {
        let indices: Vec<usize> = if self.visual_mode {
            let range = self.visual_selection_range();
            self.visible_build_indices
                .iter()
                .enumerate()
                .filter(|(vis_idx, _)| range.contains(vis_idx))
                .filter_map(|(_, &build_idx)| {
                    let build = &self.builds[build_idx];
                    if self.command_root_ids.contains(&build.activity_id) {
                        None
                    } else {
                        Some(build_idx)
                    }
                })
                .collect()
        } else {
            self.visible_build_indices
                .get(self.selected)
                .copied()
                .into_iter()
                .collect()
        };

        if indices.is_empty() {
            return None;
        }

        let values: Vec<String> = indices
            .iter()
            .filter_map(|&idx| {
                let build = &self.builds[idx];
                match field {
                    'd' => {
                        // Short derivation name (basename, trimmed of hash)
                        build.drv_path.as_ref().map(|drv| {
                            drv.rsplit('/')
                                .next()
                                .and_then(|name| name.split_once('-').map(|(_, rest)| rest))
                                .map(|s| s.trim_end_matches(".drv").to_string())
                                .unwrap_or_else(|| drv.clone())
                        })
                    }
                    'D' => {
                        // Full derivation path
                        build.drv_path.clone()
                    }
                    'c' => {
                        // Command line
                        build.command_line.clone()
                    }
                    'p' => {
                        // PID
                        build.user_pid.map(|p| p.to_string())
                    }
                    'u' => {
                        // User
                        build.user.clone()
                    }
                    'm' => {
                        // Memory: prefer cgroup memory_current, fall back to sum of process RSS
                        if let Some(mem) = build.memory_current {
                            Some(ui::format_bytes(mem))
                        } else if let Some(snapshot) = &self.snapshot {
                            snapshot.build_processes.get(&build.activity_id).map(|procs| {
                                let total: u64 = procs.iter().map(|p| p.rss_bytes).sum();
                                ui::format_bytes(total)
                            })
                        } else {
                            None
                        }
                    }
                    'l' => {
                        // Log lines (from fetched log if available, else recent_log)
                        if !self.log_lines.is_empty() {
                            Some(self.log_lines.join("\n"))
                        } else if !build.recent_log.is_empty() {
                            let lines: Vec<&str> = build.recent_log.iter().map(|s| s.as_str()).collect();
                            Some(lines.join("\n"))
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            })
            .collect();

        if values.is_empty() {
            None
        } else {
            Some(values.join("\n"))
        }
    }

    /// Extract a field value from the processes view for yanking.
    /// In visual mode, collects from all selected rows, newline-separated.
    pub fn yank_proc_field(&self, field: char) -> Option<String> {
        let vis_indices: Vec<usize> = if self.visual_mode {
            let range = self.visual_selection_range();
            range.filter(|&i| i < self.visible_proc_indices.len()).collect()
        } else {
            if self.proc_selected < self.visible_proc_indices.len() {
                vec![self.proc_selected]
            } else {
                return None;
            }
        };

        if vis_indices.is_empty() {
            return None;
        }

        let values: Vec<String> = vis_indices
            .iter()
            .filter_map(|&vi| {
                let row_idx = self.visible_proc_indices[vi];
                let row = &self.proc_rows[row_idx];
                match field {
                    'd' => {
                        // Derivation / name
                        match row {
                            ProcRow::NixCommand { cmdline, .. } => Some(cmdline.clone()),
                            ProcRow::Derivation { drv, .. } => Some(drv.clone()),
                            ProcRow::Process { info, .. } => {
                                if info.cmdline.is_empty() {
                                    Some(info.name.clone())
                                } else {
                                    Some(info.name.clone())
                                }
                            }
                        }
                    }
                    'p' => {
                        // PID
                        match row {
                            ProcRow::NixCommand { pid, .. } => Some(pid.to_string()),
                            ProcRow::Derivation { .. } => None,
                            ProcRow::Process { info, .. } => Some(info.pid.to_string()),
                        }
                    }
                    'c' => {
                        // Command
                        match row {
                            ProcRow::NixCommand { cmdline, .. } => Some(cmdline.clone()),
                            ProcRow::Derivation { drv, .. } => Some(drv.clone()),
                            ProcRow::Process { info, .. } => {
                                if info.cmdline.is_empty() {
                                    Some(format!("[{}]", info.name))
                                } else {
                                    Some(info.cmdline.clone())
                                }
                            }
                        }
                    }
                    'm' => {
                        // Memory (RSS)
                        match row {
                            ProcRow::Process { info, .. } => {
                                Some(ui::format_bytes(info.rss_bytes))
                            }
                            ProcRow::Derivation { activity_id, .. } => {
                                // Sum RSS of all processes under this build
                                self.snapshot.as_ref().and_then(|s| {
                                    s.build_processes.get(activity_id).map(|procs| {
                                        let total: u64 = procs.iter().map(|p| p.rss_bytes).sum();
                                        ui::format_bytes(total)
                                    })
                                })
                            }
                            ProcRow::NixCommand { .. } => None,
                        }
                    }
                    _ => None,
                }
            })
            .collect();

        if values.is_empty() {
            None
        } else {
            Some(values.join("\n"))
        }
    }
}
