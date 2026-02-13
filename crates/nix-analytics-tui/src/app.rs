//! Application state for the TUI.

use std::collections::{HashMap, HashSet};
use std::ops::RangeInclusive;

use nix_analytics_common::dep_graph::DepGraph;
use nix_analytics_common::types::{AnalyticsSnapshot, Build, CompletedBuild, Progress, RemoteMachine};

use crate::client::AnalyticsClient;
use crate::ui;

pub struct App {
    pub snapshot: Option<AnalyticsSnapshot>,
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
    /// Maps visible row index → index in `self.builds` (for active builds view filtering).
    pub visible_build_indices: Vec<usize>,
    /// Drv paths that are collapsed in the dep tree view.
    pub folded_nodes: HashSet<String>,
    /// Maps dep tree row index → drv_path (None for command root rows).
    pub dep_tree_drv_at_row: Vec<Option<String>>,
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
            visible_build_indices: Vec::new(),
            folded_nodes: HashSet::new(),
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
        }
    }

    pub async fn refresh(&mut self, client: &mut AnalyticsClient) -> anyhow::Result<()> {
        // Clear transient status message after it expires.
        if let Some((_, expires_at)) = &self.status_message {
            if std::time::Instant::now() >= *expires_at {
                self.status_message = None;
            }
        }

        let snapshot = client.get_snapshot().await?;

        let builds: Vec<Build> = snapshot.active_builds.values().cloned().collect();
        let tree_result = ui::tree_order_builds(builds);

        self.builds = tree_result.builds;
        self.had_realise_parent = tree_result.had_realise_parent;
        self.hoisted_download = tree_result.hoisted_download;
        self.hoisted_progress = tree_result.hoisted_progress;
        self.command_root_ids = tree_result.command_root_ids;
        self.dep_graphs = snapshot.dep_graphs.clone();
        self.snapshot = Some(snapshot);

        // Keep selection in bounds.
        if !self.builds.is_empty() && self.selected >= self.builds.len() {
            self.selected = self.builds.len() - 1;
        }

        // Refresh log if panel is open.
        if self.show_log {
            if let Some(id) = self.selected_build_id() {
                self.log_lines = client.get_build_log(id, 50).await.unwrap_or_default();
            }
        }

        Ok(())
    }

    pub fn select_prev(&mut self) {
        if self.show_dep_tree {
            if self.dep_tree_selected > 0 {
                self.dep_tree_selected -= 1;
            }
        } else if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn select_next(&mut self) {
        if self.show_dep_tree {
            if self.dep_tree_row_count > 0 && self.dep_tree_selected < self.dep_tree_row_count - 1 {
                self.dep_tree_selected += 1;
            }
        } else {
            let len = self.visible_build_indices.len();
            if len > 0 && self.selected < len - 1 {
                self.selected += 1;
            }
        }
    }

    pub fn select_top(&mut self) {
        if self.show_dep_tree {
            self.dep_tree_selected = 0;
        } else {
            self.selected = 0;
        }
    }

    pub fn select_bottom(&mut self) {
        if self.show_dep_tree {
            if self.dep_tree_row_count > 0 {
                self.dep_tree_selected = self.dep_tree_row_count - 1;
            }
        } else {
            let len = self.visible_build_indices.len();
            if len > 0 {
                self.selected = len - 1;
            }
        }
    }

    pub fn half_page_up(&mut self) {
        let delta = self.visible_rows / 2;
        if self.show_dep_tree {
            self.dep_tree_selected = self.dep_tree_selected.saturating_sub(delta);
        } else {
            self.selected = self.selected.saturating_sub(delta);
        }
    }

    pub fn half_page_down(&mut self) {
        let delta = self.visible_rows / 2;
        if self.show_dep_tree {
            let max = if self.dep_tree_row_count > 0 {
                self.dep_tree_row_count - 1
            } else {
                0
            };
            self.dep_tree_selected = (self.dep_tree_selected + delta).min(max);
        } else {
            let len = self.visible_build_indices.len();
            if len > 0 {
                self.selected = (self.selected + delta).min(len - 1);
            }
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

    pub fn toggle_fold(&mut self) {
        if let Some(Some(drv_path)) = self.dep_tree_drv_at_row.get(self.dep_tree_selected) {
            if !self.folded_nodes.remove(drv_path) {
                self.folded_nodes.insert(drv_path.clone());
            }
        }
    }

    pub fn fold_close(&mut self) {
        if let Some(Some(drv_path)) = self.dep_tree_drv_at_row.get(self.dep_tree_selected) {
            self.folded_nodes.insert(drv_path.clone());
        }
    }

    pub fn fold_open(&mut self) {
        if let Some(Some(drv_path)) = self.dep_tree_drv_at_row.get(self.dep_tree_selected) {
            self.folded_nodes.remove(drv_path);
        }
    }

    pub fn toggle_log_panel(&mut self) {
        self.show_log = !self.show_log;
    }

    pub fn toggle_history(&mut self) {
        self.show_history = !self.show_history;
    }

    pub fn toggle_machines(&mut self) {
        self.show_machines = !self.show_machines;
    }

    pub fn toggle_dep_tree(&mut self) {
        self.show_dep_tree = !self.show_dep_tree;
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
            self.dep_tree_selected = self.search_matches[self.search_match_idx];
        }
    }

    pub fn search_prev(&mut self) {
        if !self.search_matches.is_empty() {
            if self.search_match_idx == 0 {
                self.search_match_idx = self.search_matches.len() - 1;
            } else {
                self.search_match_idx -= 1;
            }
            self.dep_tree_selected = self.search_matches[self.search_match_idx];
        }
    }

    pub fn enter_visual(&mut self) {
        if !self.show_dep_tree {
            self.visual_mode = true;
            self.visual_anchor = self.selected;
        }
    }

    pub fn exit_visual(&mut self) {
        self.visual_mode = false;
        self.action_prompt = false;
    }

    /// Range lo..=hi between anchor and cursor.
    pub fn visual_selection_range(&self) -> RangeInclusive<usize> {
        let lo = self.visual_anchor.min(self.selected);
        let hi = self.visual_anchor.max(self.selected);
        lo..=hi
    }

    /// Collect activity IDs of selected builds (skip command roots).
    pub fn selected_activity_ids(&self) -> Vec<u64> {
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
        ids.iter().any(|id| {
            self.builds.iter().any(|b| b.activity_id == *id && b.cgroup_path.is_some())
        })
    }

    pub fn show_action_prompt(&mut self) {
        self.action_prompt = true;
    }

    pub fn cancel_action_prompt(&mut self) {
        self.action_prompt = false;
    }
}
