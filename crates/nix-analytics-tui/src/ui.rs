//! TUI rendering with ratatui.

use std::collections::{HashMap, HashSet};

use ratatui::prelude::*;
use ratatui::widgets::*;

use nix_analytics_common::dep_graph::DrvStatus;
use nix_analytics_common::event::ActivityType;
use nix_analytics_common::types::{AnalyticsSnapshot, Build, BuildMachine, Progress};

use crate::app::{App, ProcRow};

// Gruvbox dark palette.
#[allow(dead_code)]
const GRV_BG: Color = Color::Rgb(0x28, 0x28, 0x28);
const GRV_BG1: Color = Color::Rgb(0x3c, 0x38, 0x36);
const GRV_FG: Color = Color::Rgb(0xeb, 0xdb, 0xb2);
const GRV_FG4: Color = Color::Rgb(0xa8, 0x99, 0x84);
const GRV_GRAY: Color = Color::Rgb(0x92, 0x83, 0x74);
const GRV_RED: Color = Color::Rgb(0xfb, 0x49, 0x34);
const GRV_GREEN: Color = Color::Rgb(0xb8, 0xbb, 0x26);
const GRV_YELLOW: Color = Color::Rgb(0xfa, 0xbd, 0x2f);
const GRV_BLUE: Color = Color::Rgb(0x83, 0xa5, 0x98);
#[allow(dead_code)]
const GRV_AQUA: Color = Color::Rgb(0x8e, 0xc0, 0x7c);
#[allow(dead_code)]
const GRV_ORANGE: Color = Color::Rgb(0xfe, 0x80, 0x19);

pub fn render(frame: &mut Frame, app: &mut App) {
    let chunks = if app.show_log {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // header
                Constraint::Min(8),     // builds table
                Constraint::Length(8),  // machines (if shown)
                Constraint::Min(10),    // log panel
                Constraint::Length(1),  // status/input bar (always)
            ])
            .split(frame.area())
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // header
                Constraint::Min(15),    // builds table
                Constraint::Length(8),  // machines (if shown)
                Constraint::Length(1),  // status/input bar (always)
            ])
            .split(frame.area())
    };

    // Header.
    let active_count = app.builds.len();
    let view_indicator = if app.show_processes {
        " [PROC]"
    } else if app.show_dep_tree {
        " [DEPS]"
    } else {
        ""
    };
    let visual_indicator = if app.visual_mode { " [VISUAL]" } else { "" };
    let show_all_indicator = if app.show_all_roots { " [ALL]" } else { "" };
    let filter_indicator = if !app.builds_filter.is_empty() { " [FILTER]" } else { "" };
    let history_indicator = if app.show_history { " [HIST]" } else { "" };
    let header = Paragraph::new(format!(
        " nix-analytics | {active_count} active{view_indicator}{visual_indicator}{show_all_indicator}{filter_indicator}{history_indicator} | \
         [q]uit [d]eps [p]roc [K]ill/sig [V]isual [y]ank [l]og [h]ist [a]ll [/]search [f]ilter | j/k ^u/^d gg/G zc/zo n/N"
    ))
    .style(Style::default().fg(GRV_FG4))
    .block(Block::default().borders(Borders::ALL).title("nix-analytics")
        .border_style(Style::default().fg(GRV_GRAY)));
    frame.render_widget(header, chunks[0]);

    // Update visible_rows for half-page scrolling (area height minus borders and header).
    app.visible_rows = chunks[1].height.saturating_sub(3) as usize;

    // Builds table, dependency tree, or processes view.
    if app.show_processes {
        render_processes_table(frame, app, chunks[1]);
    } else if app.show_dep_tree {
        render_dep_tree_table(frame, app, chunks[1]);
    } else {
        render_builds_table(frame, app, chunks[1]);
    }

    // Machines or history panel (share the same slot).
    if app.show_history && chunks.len() > 2 {
        render_history(frame, app, chunks[2]);
    } else if app.show_machines && chunks.len() > 2 {
        render_machines(frame, app, chunks[2]);
    }

    // Log panel.
    if app.show_log && chunks.len() > 3 {
        render_log(frame, app, chunks[3]);
    }

    // Status / input bar (always the last chunk).
    let status_idx = chunks.len() - 1;
    if app.action_prompt {
        let count = app.selected_activity_ids().len();
        let prompt = if app.selected_have_cgroups() {
            format!(" Control {count} build(s): [k]ill [f]reeze [u]nfreeze  (Esc cancel)")
        } else {
            format!(" Control {count} build(s): [k]ill  (Esc cancel)")
        };
        let bar = Paragraph::new(prompt)
            .style(Style::default().fg(GRV_RED).bold());
        frame.render_widget(bar, chunks[status_idx]);
    } else if app.yank_prompt {
        let prompt = if app.show_processes {
            " Yank: [d]rv [p]id [c]md [m]em  (Esc cancel)"
        } else {
            " Yank: [d]rv [D]rv+hash [c]md [p]id [u]ser [m]em [l]og  (Esc cancel)"
        };
        let bar = Paragraph::new(prompt)
            .style(Style::default().fg(GRV_GREEN).bold());
        frame.render_widget(bar, chunks[status_idx]);
    } else if app.input_mode {
        let label = if app.input_is_search { "/" } else { "filter: " };
        let match_info = if app.input_is_search && !app.input_query.is_empty() {
            format!(" [{}/{}]", app.search_match_idx.saturating_add(1), app.search_matches.len())
        } else {
            String::new()
        };
        let bar = Paragraph::new(format!("{label}{}{match_info}", app.input_query))
            .style(Style::default().fg(GRV_FG).bg(GRV_BG1));
        frame.render_widget(bar, chunks[status_idx]);
    } else if !app.builds_filter.is_empty() {
        let bar = Paragraph::new(format!(" filter: \"{}\" (Esc to clear)", app.builds_filter))
            .style(Style::default().fg(GRV_YELLOW));
        frame.render_widget(bar, chunks[status_idx]);
    } else if let Some((ref msg, _)) = app.status_message {
        let status = Paragraph::new(format!(" {msg}"))
            .style(Style::default().fg(GRV_YELLOW));
        frame.render_widget(status, chunks[status_idx]);
    }
}

/// Result of tree-ordering builds.
pub struct TreeOrderResult {
    /// Builds in depth-first tree order, with Realise nodes collapsed.
    pub builds: Vec<Build>,
    /// Activity IDs whose original parent was a collapsed Realise node.
    /// Used to show "realise" in the Phase column for these builds.
    pub had_realise_parent: HashSet<u64>,
    /// Activity IDs whose FileTransfer child was hoisted (show as "download" step).
    pub hoisted_download: HashSet<u64>,
    /// Hoisted progress: activity_id → Progress from the hidden FileTransfer child.
    pub hoisted_progress: HashMap<u64, Progress>,
    /// Activity IDs that serve as command roots (rendered as bold label-only rows).
    pub command_root_ids: HashSet<u64>,
}

/// Reorder a flat list of builds into depth-first tree order.
///
/// 1. Realise-type activities with children are collapsed (children re-parented).
/// 2. Orphaned work items are adopted under matching container roots.
/// 3. Container activities sharing a `command_line` are merged into one command root.
/// 4. FileTransfer children are hoisted into their parent (parent shows as "download").
pub fn tree_order_builds(mut builds: Vec<Build>) -> TreeOrderResult {
    if builds.is_empty() {
        return TreeOrderResult {
            builds,
            had_realise_parent: HashSet::new(),
            hoisted_download: HashSet::new(),
            hoisted_progress: HashMap::new(),
            command_root_ids: HashSet::new(),
        };
    }

    // --- Step 1: Collapse Realise nodes with children ---

    let realise_ids: HashSet<u64> = builds
        .iter()
        .filter(|b| b.activity_type == ActivityType::Realise)
        .map(|b| b.activity_id)
        .collect();

    let realise_with_children: HashSet<u64> = builds
        .iter()
        .filter_map(|b| {
            b.parent_id
                .filter(|pid| realise_ids.contains(pid))
        })
        .collect();

    let had_realise_parent: HashSet<u64> = builds
        .iter()
        .filter(|b| {
            b.parent_id
                .is_some_and(|pid| realise_with_children.contains(&pid))
        })
        .map(|b| b.activity_id)
        .collect();

    let realise_parent: HashMap<u64, Option<u64>> = builds
        .iter()
        .filter(|b| realise_with_children.contains(&b.activity_id))
        .map(|b| (b.activity_id, b.parent_id))
        .collect();

    for build in &mut builds {
        if let Some(pid) = build.parent_id {
            if let Some(grandparent) = realise_parent.get(&pid) {
                build.parent_id = *grandparent;
            }
        }
    }

    builds.retain(|b| !realise_with_children.contains(&b.activity_id));

    // --- Step 2: Adopt orphans ---

    let remaining_ids: HashSet<u64> = builds.iter().map(|b| b.activity_id).collect();

    let builds_root = builds
        .iter()
        .find(|b| b.activity_type == ActivityType::Builds)
        .map(|b| b.activity_id);
    let copy_paths_root = builds
        .iter()
        .find(|b| b.activity_type == ActivityType::CopyPaths)
        .map(|b| b.activity_id);

    for build in &mut builds {
        let parent_missing = match build.parent_id {
            Some(pid) => !remaining_ids.contains(&pid),
            None => true,
        };
        if parent_missing {
            let adoptive_parent = match build.activity_type {
                ActivityType::Build
                | ActivityType::Substitute
                | ActivityType::PostBuildHook
                | ActivityType::Realise => builds_root,
                ActivityType::CopyPath => copy_paths_root,
                _ => None,
            };
            if let Some(root_id) = adoptive_parent {
                if root_id != build.activity_id {
                    build.parent_id = Some(root_id);
                }
            }
        }
    }

    // --- Step 3: Group all command_line-bearing activities under command roots ---

    // Group ALL top-level activities that have command_line set.
    // "Top-level" here means parent_id is None or points to a missing activity.
    let remaining_ids_snap: HashSet<u64> = builds.iter().map(|b| b.activity_id).collect();
    let mut cmd_groups: HashMap<String, Vec<u64>> = HashMap::new();
    for b in &builds {
        if let Some(ref cmd) = b.command_line {
            let is_top_level = match b.parent_id {
                None => true,
                Some(pid) => !remaining_ids_snap.contains(&pid),
            };
            if is_top_level {
                cmd_groups.entry(cmd.clone()).or_default().push(b.activity_id);
            }
        }
    }

    let mut command_root_ids: HashSet<u64> = HashSet::new();
    let mut ids_to_remove: HashSet<u64> = HashSet::new();
    let mut reparent_map: HashMap<u64, u64> = HashMap::new(); // old_parent → new_parent

    // Use a counter for synthetic root IDs, starting from u64::MAX and going down.
    let mut synthetic_counter: u64 = 0;

    for (cmd, group) in &cmd_groups {
        // Find the best representative: prefer Builds > CopyPaths > none.
        let builds_type_id = group.iter().find(|&&id| {
            builds
                .iter()
                .any(|b| b.activity_id == id && b.activity_type == ActivityType::Builds)
        });
        let copy_paths_type_id = group.iter().find(|&&id| {
            builds
                .iter()
                .any(|b| b.activity_id == id && b.activity_type == ActivityType::CopyPaths)
        });

        let representative = if let Some(&id) = builds_type_id {
            // Existing Builds container becomes the command root.
            command_root_ids.insert(id);
            id
        } else if let Some(&id) = copy_paths_type_id {
            // Existing CopyPaths container becomes the command root.
            command_root_ids.insert(id);
            id
        } else if group.len() > 1 {
            // No container exists — synthesize a virtual command root.
            let synthetic_id = u64::MAX - synthetic_counter;
            synthetic_counter += 1;
            let earliest_ts = group
                .iter()
                .filter_map(|&id| builds.iter().find(|b| b.activity_id == id))
                .map(|b| b.started_at_us)
                .min()
                .unwrap_or(0);
            builds.push(Build {
                activity_id: synthetic_id,
                activity_type: ActivityType::Builds, // treated as container for rendering
                drv_path: None,
                description: String::new(),
                parent_id: None,
                command_line: Some(cmd.clone()),
                started_at_us: earliest_ts,
                phase: None,
                recent_log: std::collections::VecDeque::new(),
                user: None,
                user_pid: None,
                user_uid: None,
                cgroup_path: None,
                cpu_user_us: 0,
                cpu_system_us: 0,
                memory_current: None,
                progress: None,
                machine: BuildMachine::Local,
                is_frozen: false,
            });
            command_root_ids.insert(synthetic_id);
            synthetic_id
        } else {
            // Single non-container activity with command_line — just mark it as
            // a command root so it renders as a label row, and make it a Builds type.
            let id = group[0];
            // Synthesize a virtual root for it too, so the original stays as a child.
            let synthetic_id = u64::MAX - synthetic_counter;
            synthetic_counter += 1;
            let ts = builds
                .iter()
                .find(|b| b.activity_id == id)
                .map(|b| b.started_at_us)
                .unwrap_or(0);
            builds.push(Build {
                activity_id: synthetic_id,
                activity_type: ActivityType::Builds,
                drv_path: None,
                description: String::new(),
                parent_id: None,
                command_line: Some(cmd.clone()),
                started_at_us: ts,
                phase: None,
                recent_log: std::collections::VecDeque::new(),
                user: None,
                user_pid: None,
                user_uid: None,
                cgroup_path: None,
                cpu_user_us: 0,
                cpu_system_us: 0,
                memory_current: None,
                progress: None,
                machine: BuildMachine::Local,
                is_frozen: false,
            });
            command_root_ids.insert(synthetic_id);
            synthetic_id
        };

        // Reparent all non-representative members under the representative.
        for &id in group {
            if id != representative {
                // This activity becomes a child of the representative.
                // If it was itself a container with children, those children
                // need to be reparented too.
                if builds
                    .iter()
                    .any(|b| b.activity_id == id && matches!(b.activity_type, ActivityType::Builds | ActivityType::CopyPaths))
                {
                    // Container being merged: reparent its children and remove it.
                    ids_to_remove.insert(id);
                    reparent_map.insert(id, representative);
                } else {
                    // Non-container: just reparent it under the command root.
                    if let Some(b) = builds.iter_mut().find(|b| b.activity_id == id) {
                        b.parent_id = Some(representative);
                    }
                }
            }
        }
    }

    // Also mark any existing Builds/CopyPaths containers with command_line that
    // weren't in a multi-member group as command roots.
    for b in &builds {
        if matches!(b.activity_type, ActivityType::Builds | ActivityType::CopyPaths)
            && b.command_line.is_some()
        {
            command_root_ids.insert(b.activity_id);
        }
    }

    // Reparent children of removed containers.
    for build in &mut builds {
        if let Some(pid) = build.parent_id {
            if let Some(&new_parent) = reparent_map.get(&pid) {
                build.parent_id = Some(new_parent);
            }
        }
    }

    // Remove merged-away containers.
    builds.retain(|b| !ids_to_remove.contains(&b.activity_id));

    // --- Step 4: Hoist FileTransfer into parent ---

    // Find work items (Build/FetchTree) that have exactly one child and it's a FileTransfer.
    let remaining_ids: HashSet<u64> = builds.iter().map(|b| b.activity_id).collect();
    let mut parent_child_count: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, b) in builds.iter().enumerate() {
        if let Some(pid) = b.parent_id {
            if remaining_ids.contains(&pid) {
                parent_child_count.entry(pid).or_default().push(i);
            }
        }
    }

    let mut hoisted_download: HashSet<u64> = HashSet::new();
    let mut hoisted_progress: HashMap<u64, Progress> = HashMap::new();
    let mut ft_ids_to_remove: HashSet<u64> = HashSet::new();

    for (parent_id, children_indices) in &parent_child_count {
        if children_indices.len() != 1 {
            continue;
        }
        let child_idx = children_indices[0];
        let child = &builds[child_idx];
        if child.activity_type != ActivityType::FileTransfer {
            continue;
        }
        // Check that parent is a work item type (not a container).
        let parent = builds.iter().find(|b| b.activity_id == *parent_id);
        let is_work_item = parent.is_some_and(|p| {
            matches!(
                p.activity_type,
                ActivityType::Build
                    | ActivityType::Substitute
                    | ActivityType::FetchTree
                    | ActivityType::CopyPath
            )
        });
        if !is_work_item {
            continue;
        }
        hoisted_download.insert(*parent_id);
        if let Some(ref prog) = child.progress {
            hoisted_progress.insert(*parent_id, prog.clone());
        }
        ft_ids_to_remove.insert(child.activity_id);
    }

    builds.retain(|b| !ft_ids_to_remove.contains(&b.activity_id));

    // --- Step 5: DFS tree ordering ---

    let remaining_ids: HashSet<u64> = builds.iter().map(|b| b.activity_id).collect();

    let mut children_of: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut roots: Vec<usize> = Vec::new();

    for (i, build) in builds.iter().enumerate() {
        match build.parent_id {
            Some(pid) if remaining_ids.contains(&pid) => {
                children_of.entry(pid).or_default().push(i);
            }
            _ => {
                roots.push(i);
            }
        }
    }

    roots.sort_by_key(|&i| builds[i].started_at_us);
    for children in children_of.values_mut() {
        children.sort_by_key(|&i| builds[i].started_at_us);
    }

    let mut result = Vec::with_capacity(builds.len());
    let mut stack: Vec<usize> = Vec::new();

    for &i in roots.iter().rev() {
        stack.push(i);
    }

    while let Some(idx) = stack.pop() {
        result.push(idx);
        if let Some(children) = children_of.get(&builds[idx].activity_id) {
            for &child_idx in children.iter().rev() {
                stack.push(child_idx);
            }
        }
    }

    let mut slots: Vec<Option<Build>> = builds.into_iter().map(Some).collect();
    let ordered: Vec<Build> = result
        .into_iter()
        .filter_map(|i| slots[i].take())
        .collect();

    TreeOrderResult {
        builds: ordered,
        had_realise_parent,
        hoisted_download,
        hoisted_progress,
        command_root_ids,
    }
}

/// Compute box-drawing tree prefixes for builds already in tree order.
///
/// Returns a prefix string per build: roots get `""`, children get
/// `"├─ "` or `"└─ "` with proper continuation lines (`"│  "` / `"   "`).
pub fn compute_tree_prefixes(builds: &[Build]) -> Vec<String> {
    if builds.is_empty() {
        return Vec::new();
    }

    let known_ids: HashSet<u64> = builds.iter().map(|b| b.activity_id).collect();

    // Build parent→children mapping.
    let mut children_of: HashMap<u64, Vec<u64>> = HashMap::new();

    for build in builds {
        if let Some(pid) = build.parent_id {
            if known_ids.contains(&pid) {
                children_of.entry(pid).or_default().push(build.activity_id);
            }
        }
    }

    // Index builds by activity_id for ancestor walks.
    let build_by_id: HashMap<u64, &Build> = builds.iter().map(|b| (b.activity_id, b)).collect();

    let mut prefixes = Vec::with_capacity(builds.len());
    for build in builds {
        match build.parent_id {
            Some(pid) if known_ids.contains(&pid) => {
                let siblings = &children_of[&pid];
                let is_last = siblings.last() == Some(&build.activity_id);

                // Build prefix segments from this node up to the root.
                let mut segments: Vec<&str> = Vec::new();
                segments.push(if is_last { "└─ " } else { "├─ " });

                // Walk ancestors to add continuation lines.
                let mut current_id = pid;
                loop {
                    let current_build = build_by_id.get(&current_id);
                    match current_build.and_then(|b| b.parent_id) {
                        Some(grandparent) if known_ids.contains(&grandparent) => {
                            let parent_siblings = &children_of[&grandparent];
                            if parent_siblings.last() == Some(&current_id) {
                                segments.push("   ");
                            } else {
                                segments.push("│  ");
                            }
                            current_id = grandparent;
                        }
                        _ => break,
                    }
                }

                segments.reverse();
                prefixes.push(segments.concat());
            }
            _ => {
                prefixes.push(String::new());
            }
        }
    }

    prefixes
}

/// Derive the Step column value for a build.
fn step_label(build: &Build, hoisted_download: &HashSet<u64>) -> &'static str {
    if hoisted_download.contains(&build.activity_id) {
        return "download";
    }
    match build.activity_type {
        ActivityType::Build => "build",
        ActivityType::Substitute => "substitute",
        ActivityType::FileTransfer => "download",
        ActivityType::CopyPath => "copy",
        ActivityType::FetchTree => "fetch",
        ActivityType::Realise => "realise",
        ActivityType::BuildWaiting => "waiting",
        ActivityType::PostBuildHook => "post-hook",
        ActivityType::Builds | ActivityType::CopyPaths => "",
        _ => "",
    }
}

/// Format a progress value for display.
fn format_progress(progress: &Progress) -> String {
    if progress.expected == 0 {
        return "-".to_string();
    }
    // Use human-readable byte units for large values (likely byte counts).
    if progress.expected >= 1024 {
        format!("{}/{}", format_bytes(progress.done), format_bytes(progress.expected))
    } else {
        format!("{}/{}", progress.done, progress.expected)
    }
}

fn render_builds_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let header = Row::new(vec![
        Cell::from(" Derivation"),
        Cell::from("Step"),
        Cell::from("Phase"),
        Cell::from("St"),
        Cell::from("Progress"),
        Cell::from("User"),
        Cell::from("Machine"),
        Cell::from("Time"),
        Cell::from("Mem"),
    ])
    .style(Style::default().bold());

    let now_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);

    // Determine which command roots are finished.
    let finished_root_ids: HashSet<u64> = if !app.show_all_roots {
        app.builds
            .iter()
            .filter(|b| app.command_root_ids.contains(&b.activity_id))
            .filter(|b| {
                b.progress.as_ref().is_some_and(|p| {
                    p.expected > 0 && p.done >= p.expected && p.running == 0
                })
            })
            .map(|b| b.activity_id)
            .collect()
    } else {
        HashSet::new()
    };

    // Map each build to its command root by walking parent_id chains.
    let build_id_to_idx: HashMap<u64, usize> = app
        .builds
        .iter()
        .enumerate()
        .map(|(i, b)| (b.activity_id, i))
        .collect();

    let mut visible_indices: Vec<usize> = Vec::new();
    for (i, build) in app.builds.iter().enumerate() {
        if !finished_root_ids.is_empty() {
            // Check if this build belongs to a finished root.
            let mut cur_id = build.activity_id;
            let mut is_under_finished = false;
            loop {
                if finished_root_ids.contains(&cur_id) {
                    is_under_finished = true;
                    break;
                }
                let cur_build = build_id_to_idx
                    .get(&cur_id)
                    .and_then(|&idx| app.builds.get(idx));
                match cur_build.and_then(|b| b.parent_id) {
                    Some(pid) if build_id_to_idx.contains_key(&pid) => cur_id = pid,
                    _ => break,
                }
            }
            if is_under_finished {
                continue;
            }
        }
        visible_indices.push(i);
    }

    // Apply builds filter (active filter or live input).
    let filter_str = if app.input_mode && !app.input_is_search {
        &app.input_query
    } else {
        &app.builds_filter
    };
    if !filter_str.is_empty() {
        let filter_lower = filter_str.to_lowercase();
        visible_indices.retain(|&i| {
            let build = &app.builds[i];
            // Always keep command roots visible if any child matches.
            app.command_root_ids.contains(&build.activity_id)
                || drv_display_name(build).to_lowercase().contains(&filter_lower)
        });
    }

    app.visible_build_indices = visible_indices;

    // Clamp selection.
    if !app.visible_build_indices.is_empty() && app.selected >= app.visible_build_indices.len() {
        app.selected = app.visible_build_indices.len() - 1;
    }

    let prefixes = compute_tree_prefixes(&app.builds);

    let rows: Vec<Row> = app
        .visible_build_indices
        .iter()
        .enumerate()
        .map(|(vis_idx, &build_idx)| {
            let build = &app.builds[build_idx];
            let prefix = &prefixes[build_idx];
            let is_command_root = app.command_root_ids.contains(&build.activity_id);

            let in_visual =
                app.visual_mode && app.visual_selection_range().contains(&vis_idx);
            let style = if in_visual {
                Style::default().bg(GRV_BLUE).fg(GRV_FG)
            } else if is_command_root {
                Style::default().bold()
            } else {
                Style::default()
            };

            let mut drv_name = format!("{}{}", prefix, drv_display_name(build));

            if is_command_root {
                // Append progress summary to command root.
                if let Some(ref prog) = build.progress {
                    let label = format_root_progress(prog);
                    if !label.is_empty() {
                        drv_name = format!("{drv_name} ({label})");
                    }
                }

                // When show_all_roots, append [done] / [FAILED] for finished roots.
                if app.show_all_roots {
                    if let Some(ref prog) = build.progress {
                        if prog.expected > 0 && prog.done >= prog.expected && prog.running == 0 {
                            if prog.failed > 0 {
                                drv_name = format!("{drv_name} [FAILED]");
                                return Row::new(vec![
                                    Cell::from(drv_name).style(Style::default().fg(GRV_RED)),
                                    Cell::from(""),
                                    Cell::from(""),
                                    Cell::from(""),
                                    Cell::from(""),
                                    Cell::from(""),
                                    Cell::from(""),
                                    Cell::from(""),
                                    Cell::from(""),
                                ])
                                .style(style);
                            } else {
                                drv_name = format!("{drv_name} [done]");
                            }
                        }
                    }
                }

                // Command root: only show the derivation name, rest is empty.
                return Row::new(vec![
                    Cell::from(drv_name),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(""),
                ])
                .style(style);
            }

            let step = step_label(build, &app.hoisted_download);
            let phase = build.phase.as_deref().unwrap_or("");

            // Progress: hoisted > own > "-"
            let progress_str = if let Some(prog) = app.hoisted_progress.get(&build.activity_id) {
                format_progress(prog)
            } else if let Some(ref prog) = build.progress {
                if prog.expected > 0 {
                    format_progress(prog)
                } else {
                    "-".to_string()
                }
            } else {
                "-".to_string()
            };

            let user = build.user.as_deref().unwrap_or("-");
            let machine = match &build.machine {
                BuildMachine::Local => "local".to_string(),
                BuildMachine::Remote { uri } => short_uri(uri),
            };
            let elapsed = format_duration_us(now_us.saturating_sub(build.started_at_us));
            let mem = build
                .memory_current
                .map(format_bytes)
                .unwrap_or_else(|| "-".to_string());

            let status_cell = if build.is_frozen {
                Cell::from("FROZEN").style(Style::default().fg(GRV_YELLOW))
            } else {
                Cell::from("")
            };

            Row::new(vec![
                Cell::from(drv_name),
                Cell::from(step),
                Cell::from(phase.to_string()),
                status_cell,
                Cell::from(progress_str),
                Cell::from(user.to_string()),
                Cell::from(machine),
                Cell::from(elapsed),
                Cell::from(mem),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(25),     // Derivation (tree-prefixed, needs space)
            Constraint::Length(12),   // Step
            Constraint::Length(16),   // Phase
            Constraint::Length(6),    // St (frozen status)
            Constraint::Length(12),   // Progress
            Constraint::Length(8),    // User
            Constraint::Length(12),   // Machine
            Constraint::Length(7),    // Time
            Constraint::Length(6),    // Mem
        ],
    )
    .header(header)
    .row_highlight_style(Style::default().bg(GRV_BG1).fg(GRV_FG))
    .block(Block::default().borders(Borders::ALL).title("Builds")
        .border_style(Style::default().fg(GRV_GRAY)));

    let mut table_state = TableState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(table, area, &mut table_state);
}

/// Format progress counters for display on command root rows.
/// Returns something like "5/23 done, 3 active".
fn format_root_progress(progress: &Progress) -> String {
    let mut parts = Vec::new();
    if progress.expected > 0 {
        parts.push(format!("{}/{} done", progress.done, progress.expected));
    }
    if progress.running > 0 {
        parts.push(format!("{} active", progress.running));
    }
    if progress.failed > 0 {
        parts.push(format!("{} failed", progress.failed));
    }
    parts.join(", ")
}

/// Render the dependency tree view.
fn render_dep_tree_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let header = Row::new(vec![
        Cell::from(" Derivation"),
        Cell::from("Status"),
        Cell::from("Time"),
    ])
    .style(Style::default().bold());

    let now_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);

    let mut rows: Vec<Row> = Vec::new();
    let mut drv_at_row: Vec<Option<String>> = Vec::new();
    let mut search_match_rows: Vec<usize> = Vec::new();
    let search_active = app.input_is_search && !app.input_query.is_empty();
    let query_lower = app.input_query.to_lowercase();

    for graph in &app.dep_graphs {
        // Try to find the matching command root in active builds for accurate progress.
        let build_progress: Option<&Progress> = graph.command_line.as_ref().and_then(|cmd| {
            app.builds.iter().find(|b| {
                app.command_root_ids.contains(&b.activity_id)
                    && b.command_line.as_ref() == Some(cmd)
            })
        }).and_then(|b| b.progress.as_ref());

        // Compute graph-level node counts (used as fallback and for "all finished" check).
        let active_count = graph
            .nodes
            .values()
            .filter(|n| {
                matches!(n.status, DrvStatus::Building { .. } | DrvStatus::Substituting { .. })
            })
            .count();
        let failed_count = graph
            .nodes
            .values()
            .filter(|n| matches!(n.status, DrvStatus::Failed))
            .count();

        // Skip finished graphs when not showing all.
        // Use build progress if available, otherwise fall back to graph node statuses.
        let all_finished = if let Some(prog) = build_progress {
            prog.expected > 0 && prog.done >= prog.expected && prog.running == 0
        } else {
            !graph.nodes.is_empty()
                && graph
                    .nodes
                    .values()
                    .all(|n| matches!(n.status, DrvStatus::Done | DrvStatus::Failed))
        };
        if !app.show_all_roots && all_finished {
            continue;
        }

        // Add command root row.
        let cmd_label = graph
            .command_line
            .as_deref()
            .unwrap_or("(unknown command)");

        // Build progress label: prefer active build's Progress, fall back to graph node counts.
        let mut progress_parts = Vec::new();
        if let Some(prog) = build_progress {
            if prog.expected > 0 {
                progress_parts.push(format!("{}/{} done", prog.done, prog.expected));
            }
            if prog.running > 0 {
                progress_parts.push(format!("{} active", prog.running));
            }
            if prog.failed > 0 {
                progress_parts.push(format!("{} failed", prog.failed));
            }
        } else {
            let total = graph.nodes.len();
            let done_count = graph
                .nodes
                .values()
                .filter(|n| matches!(n.status, DrvStatus::Done))
                .count();
            progress_parts.push(format!("{done_count}/{total} done"));
            if active_count > 0 {
                progress_parts.push(format!("{active_count} active"));
            }
            if failed_count > 0 {
                progress_parts.push(format!("{failed_count} failed"));
            }
        }
        let mut root_label = format!("{cmd_label} ({})", progress_parts.join(", "));

        // Append status label for finished graphs that are shown.
        if all_finished {
            if failed_count > 0 {
                root_label = format!("{root_label} [FAILED]");
                rows.push(
                    Row::new(vec![
                        Cell::from(format!(" {root_label}")).style(Style::default().fg(GRV_RED)),
                        Cell::from(""),
                        Cell::from(""),
                    ])
                    .style(Style::default().bold()),
                );
                drv_at_row.push(None);
            } else {
                root_label = format!("{root_label} [done]");
                rows.push(
                    Row::new(vec![
                        Cell::from(format!(" {root_label}")),
                        Cell::from(""),
                        Cell::from(""),
                    ])
                    .style(Style::default().bold()),
                );
                drv_at_row.push(None);
            }
        } else {
            rows.push(
                Row::new(vec![
                    Cell::from(format!(" {root_label}")),
                    Cell::from(""),
                    Cell::from(""),
                ])
                .style(Style::default().bold()),
            );
            drv_at_row.push(None);
        }

        // DFS traversal of each root in the graph.
        // Share `seen` across all roots so duplicates are properly detected.
        let mut seen: HashSet<&str> = HashSet::new();
        for root_drv in &graph.roots {
            let mut stack: Vec<(&str, Vec<bool>)> = vec![(root_drv.as_str(), Vec::new())];

            while let Some((drv_path, ancestor_is_last)) = stack.pop() {
                let Some(node) = graph.nodes.get(drv_path) else {
                    continue;
                };

                // Compute children before dup check (needed for fold indicator).
                let children: Vec<&str> = node
                    .input_drvs
                    .iter()
                    .filter(|d| graph.nodes.contains_key(d.as_str()))
                    .map(|d| d.as_str())
                    .collect();

                let is_dup = !seen.insert(drv_path);

                // Compute tree prefix.
                let prefix = if ancestor_is_last.is_empty() {
                    " └─ ".to_string()
                } else {
                    let mut prefix = String::from(" ");
                    for (i, &is_last) in ancestor_is_last.iter().enumerate() {
                        if i == ancestor_is_last.len() - 1 {
                            prefix.push_str(if is_last { "└─ " } else { "├─ " });
                        } else {
                            prefix.push_str(if is_last { "   " } else { "│  " });
                        }
                    }
                    prefix
                };

                if is_dup {
                    let drv_name = format!("{prefix}{}", node.name);
                    rows.push(
                        Row::new(vec![
                            Cell::from(drv_name),
                            Cell::from("(dup)"),
                            Cell::from(""),
                        ])
                        .style(Style::default().fg(GRV_GRAY).italic()),
                    );
                    drv_at_row.push(Some(drv_path.to_string()));
                    continue; // Don't recurse into duplicates.
                }

                let is_folded = app.folded_nodes.contains(drv_path);
                let has_children = !children.is_empty();

                let fold_indicator = if has_children {
                    if is_folded { "▶ " } else { "▼ " }
                } else {
                    ""
                };
                let drv_name = format!("{prefix}{fold_indicator}{}", node.name);

                let (status_text, status_style) = format_dep_status(&node.status);
                let time_str = match &node.status {
                    DrvStatus::Building { .. } | DrvStatus::Substituting { .. } => {
                        node.started_at_us
                            .map(|ts| format_duration_us(now_us.saturating_sub(ts)))
                            .unwrap_or_else(|| "-".to_string())
                    }
                    _ => "-".to_string(),
                };

                let is_search_match = search_active
                    && node.name.to_lowercase().contains(&query_lower);

                let row_idx = rows.len();
                let row_style = if is_search_match {
                    Style::default().fg(GRV_ORANGE)
                } else {
                    Style::default()
                };

                rows.push(
                    Row::new(vec![
                        Cell::from(drv_name),
                        Cell::from(status_text).style(status_style),
                        Cell::from(time_str),
                    ])
                    .style(row_style),
                );
                drv_at_row.push(Some(drv_path.to_string()));

                if is_search_match {
                    search_match_rows.push(row_idx);
                }

                // Only push children onto stack if not folded.
                if !is_folded {
                    for (ci, child) in children.iter().enumerate().rev() {
                        let is_last_child = ci == children.len() - 1;
                        let mut child_ancestors = ancestor_is_last.clone();
                        child_ancestors.push(is_last_child);
                        stack.push((child, child_ancestors));
                    }
                }
            }
        }
    }

    if rows.is_empty() {
        rows.push(Row::new(vec![
            Cell::from(" No dependency data yet (waiting for builds to start)"),
            Cell::from(""),
            Cell::from(""),
        ])
        .style(Style::default().fg(GRV_GRAY)));
    }

    // Update row count and drv_at_row for scroll bounds, selection clamping, and folding.
    app.dep_tree_row_count = rows.len();
    app.dep_tree_drv_at_row = drv_at_row;
    if app.dep_tree_selected >= rows.len() && !rows.is_empty() {
        app.dep_tree_selected = rows.len() - 1;
    }

    // Update search matches and auto-jump during live search input.
    app.search_matches = search_match_rows;
    if search_active && !app.search_matches.is_empty() {
        // Clamp match index.
        if app.search_match_idx >= app.search_matches.len() {
            app.search_match_idx = 0;
        }
        // During input mode, auto-jump to the first match.
        if app.input_mode {
            app.search_match_idx = 0;
            app.dep_tree_selected = app.search_matches[0];
        }
    } else if !search_active {
        app.search_matches.clear();
        app.search_match_idx = 0;
    }

    let table = Table::new(
        rows,
        [
            Constraint::Min(30),    // Derivation
            Constraint::Min(25),    // Status
            Constraint::Length(7),  // Time
        ],
    )
    .header(header)
    .row_highlight_style(Style::default().bg(GRV_BG1).fg(GRV_FG))
    .block(Block::default().borders(Borders::ALL).title("Dependency Tree")
        .border_style(Style::default().fg(GRV_GRAY)));

    let mut table_state = TableState::default().with_selected(Some(app.dep_tree_selected));
    frame.render_stateful_widget(table, area, &mut table_state);
}

/// Format a DrvStatus for display, returning (text, style).
fn format_dep_status(status: &DrvStatus) -> (String, Style) {
    match status {
        DrvStatus::Pending => ("pending".to_string(), Style::default().fg(GRV_GRAY)),
        DrvStatus::Building { phase } => {
            let text = match phase {
                Some(p) => format!("building: {p}"),
                None => "building".to_string(),
            };
            (text, Style::default().fg(GRV_YELLOW))
        }
        DrvStatus::Substituting { done, expected } => {
            let text = match (done, expected) {
                (Some(d), Some(e)) if *e > 0 => {
                    format!("substituting: {}/{}", format_bytes(*d), format_bytes(*e))
                }
                _ => "substituting".to_string(),
            };
            (text, Style::default().fg(GRV_BLUE))
        }
        DrvStatus::Done => ("done".to_string(), Style::default().fg(GRV_GREEN)),
        DrvStatus::Failed => ("FAILED".to_string(), Style::default().fg(GRV_RED).bold()),
    }
}

/// Build a flattened process tree from snapshot data.
///
/// Groups active builds by `user_pid` (nix command roots), then for each build
/// lists the processes from `build_processes`.
pub fn build_process_tree(
    snapshot: &AnalyticsSnapshot,
    builds: &[Build],
    command_root_ids: &HashSet<u64>,
) -> Vec<ProcRow> {
    let mut rows = Vec::new();

    // Group non-command-root builds by user_pid.
    let mut pid_groups: HashMap<u32, Vec<&Build>> = HashMap::new();
    let mut pid_cmdline: HashMap<u32, String> = HashMap::new();

    for build in builds {
        if command_root_ids.contains(&build.activity_id) {
            // Extract PID and command_line from command roots for the NixCommand row.
            if let (Some(pid), Some(ref cmd)) = (build.user_pid, &build.command_line) {
                pid_cmdline.entry(pid).or_insert_with(|| cmd.clone());
            }
            continue;
        }
        if let Some(pid) = build.user_pid {
            pid_groups.entry(pid).or_default().push(build);
        }
    }

    // Also discover PIDs from builds that have no command root.
    for build in builds {
        if command_root_ids.contains(&build.activity_id) {
            continue;
        }
        if let Some(pid) = build.user_pid {
            if !pid_cmdline.contains_key(&pid) {
                pid_cmdline.insert(pid, format!("nix (pid {})", pid));
            }
        }
    }

    // Sort PIDs for stable ordering.
    let mut pids: Vec<u32> = pid_cmdline.keys().copied().collect();
    pids.sort_unstable();

    for pid in pids {
        let cmdline = pid_cmdline.get(&pid).cloned().unwrap_or_default();
        rows.push(ProcRow::NixCommand { pid, cmdline });

        if let Some(group) = pid_groups.get(&pid) {
            let mut group_sorted: Vec<&&Build> = group.iter().collect();
            group_sorted.sort_by_key(|b| b.started_at_us);

            for build in group_sorted {
                let drv = drv_display_name(build);
                rows.push(ProcRow::Derivation {
                    activity_id: build.activity_id,
                    drv,
                    is_frozen: build.is_frozen,
                });

                // Add processes for this build.
                if let Some(procs) = snapshot.build_processes.get(&build.activity_id) {
                    let mut sorted_procs: Vec<_> = procs.iter().collect();
                    sorted_procs.sort_by_key(|p| p.pid);
                    for info in sorted_procs {
                        rows.push(ProcRow::Process {
                            info: info.clone(),
                            activity_id: build.activity_id,
                        });
                    }
                }
            }
        }
    }

    rows
}

/// Map a process state char to a full word.
fn process_state_label(state: char) -> &'static str {
    match state {
        'R' => "running",
        'S' => "sleeping",
        'D' => "disk",
        'Z' => "zombie",
        'T' => "stopped",
        'I' => "idle",
        'X' => "dead",
        _ => "?",
    }
}

/// Render the processes table view.
fn render_processes_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let header = Row::new(vec![
        Cell::from(" PID"),
        Cell::from("Status"),
        Cell::from("Command"),
    ])
    .style(Style::default().bold());

    // Precompute tree structure on the FULL proc_rows (before folding), since
    // visible_proc_indices references into the full list.
    let proc_rows = &app.proc_rows;

    let mut is_last_deriv_vis = HashMap::new(); // row_idx → bool
    let mut has_children_vis = HashMap::new();  // row_idx → bool (has visible children)
    let mut parent_deriv_last_vis = HashMap::new();
    let mut is_last_proc_vis = HashMap::new();
    let mut is_folded_map = HashMap::new();     // row_idx → bool

    // Work from visible_proc_indices to compute tree structure among visible rows.
    let vis = &app.visible_proc_indices;
    for (vi, &ri) in vis.iter().enumerate() {
        match &proc_rows[ri] {
            ProcRow::NixCommand { pid, .. } => {
                is_folded_map.insert(ri, app.folded_proc_pids.contains(pid));
            }
            ProcRow::Derivation { activity_id, .. } => {
                let folded = app.folded_proc_builds.contains(activity_id);
                is_folded_map.insert(ri, folded);

                // Is this the last Derivation before the next NixCommand (in visible rows)?
                let mut found_next_deriv = false;
                let mut found_child = false;
                for &rj in vis[(vi + 1)..].iter() {
                    match &proc_rows[rj] {
                        ProcRow::NixCommand { .. } => break,
                        ProcRow::Derivation { .. } => { found_next_deriv = true; break; }
                        ProcRow::Process { .. } => { found_child = true; }
                    }
                }
                is_last_deriv_vis.insert(ri, !found_next_deriv);
                // has_children: true if folded (children exist but hidden) or visible children found
                has_children_vis.insert(ri, found_child || folded);
            }
            ProcRow::Process { activity_id, .. } => {
                // Find parent derivation's is_last among visible rows.
                let mut parent_last = false;
                for &rk in vis[..vi].iter().rev() {
                    match &proc_rows[rk] {
                        ProcRow::Derivation { .. } => {
                            parent_last = *is_last_deriv_vis.get(&rk).unwrap_or(&false);
                            break;
                        }
                        ProcRow::NixCommand { .. } => break,
                        _ => {}
                    }
                }
                parent_deriv_last_vis.insert(ri, parent_last);

                // Is this the last Process with the same activity_id in visible rows?
                let mut found_next = false;
                for &rj in vis[(vi + 1)..].iter() {
                    match &proc_rows[rj] {
                        ProcRow::Process { activity_id: aid, .. } if aid == activity_id => {
                            found_next = true;
                            break;
                        }
                        _ => break,
                    }
                }
                is_last_proc_vis.insert(ri, !found_next);
            }
        }
    }

    let rows: Vec<Row> = vis
        .iter()
        .enumerate()
        .map(|(vi, &ri)| {
            let row = &proc_rows[ri];
            let style = if vi == app.proc_selected {
                Style::default().bg(GRV_BG1).fg(GRV_FG)
            } else {
                Style::default()
            };

            match row {
                ProcRow::NixCommand { pid, cmdline } => {
                    let folded = is_folded_map.get(&ri).copied().unwrap_or(false);
                    let indicator = if folded { "\u{25b6}" } else { "\u{25b8}" }; // ▶ / ▸
                    Row::new(vec![
                        Cell::from(format!(" {indicator} {pid}")),
                        Cell::from(""),
                        Cell::from(format!("{cmdline} (root)")),
                    ])
                    .style(style.bold())
                }
                ProcRow::Derivation { activity_id: _, drv, is_frozen } => {
                    let st = if *is_frozen {
                        Cell::from("frozen").style(Style::default().fg(GRV_YELLOW))
                    } else {
                        Cell::from("")
                    };
                    let is_last = is_last_deriv_vis.get(&ri).copied().unwrap_or(false);
                    let has_kids = has_children_vis.get(&ri).copied().unwrap_or(false);
                    let folded = is_folded_map.get(&ri).copied().unwrap_or(false);
                    let cap = if has_kids {
                        if folded { "\u{25b6}" } else { "\u{2510}" } // ▶ or ┐
                    } else {
                        " "
                    };
                    let branch_char = if is_last { "\u{2514}\u{2500}" } else { "\u{251c}\u{2500}" };
                    Row::new(vec![
                        Cell::from(format!("   {branch_char}{cap}")),
                        st,
                        Cell::from(Line::from(vec![
                            Span::styled(format!("{drv}.drv"), Style::default().fg(GRV_AQUA)),
                        ])),
                    ])
                    .style(style)
                }
                ProcRow::Process { info, .. } => {
                    let (st_label, st_style) = match info.state {
                        'T' => (process_state_label('T'), Style::default().fg(GRV_YELLOW)),
                        'R' => (process_state_label('R'), Style::default().fg(GRV_GREEN)),
                        'D' => (process_state_label('D'), Style::default().fg(GRV_RED)),
                        'Z' => (process_state_label('Z'), Style::default().fg(GRV_RED)),
                        other => (process_state_label(other), Style::default()),
                    };
                    let cmd = if info.cmdline.is_empty() {
                        format!("[{}]", info.name)
                    } else {
                        info.cmdline.clone()
                    };
                    let parent_last = parent_deriv_last_vis.get(&ri).copied().unwrap_or(false);
                    let is_last = is_last_proc_vis.get(&ri).copied().unwrap_or(true);
                    let outer = if parent_last { "    " } else { "   \u{2502}" };
                    let inner = if is_last { "\u{2514} " } else { "\u{251c} " };
                    Row::new(vec![
                        Cell::from(format!("{outer} {inner}{}", info.pid)),
                        Cell::from(st_label).style(st_style),
                        Cell::from(cmd),
                    ])
                    .style(style)
                }
            }
        })
        .collect();

    if rows.is_empty() {
        let empty_rows = vec![Row::new(vec![
            Cell::from(" No processes data (waiting for cgroup assignment)"),
            Cell::from(""),
            Cell::from(""),
        ])
        .style(Style::default().fg(GRV_GRAY))];
        let table = Table::new(
            empty_rows,
            [
                Constraint::Length(14),
                Constraint::Length(10),
                Constraint::Min(30),
            ],
        )
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Processes (p to go back)")
                .border_style(Style::default().fg(GRV_GRAY)),
        );
        frame.render_widget(table, area);
        return;
    }

    let table = Table::new(
        rows,
        [
            Constraint::Length(14),
            Constraint::Length(10),
            Constraint::Min(30),
        ],
    )
    .header(header)
    .row_highlight_style(Style::default().bg(GRV_BG1).fg(GRV_FG))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Processes (p to go back)")
            .border_style(Style::default().fg(GRV_GRAY)),
    );

    let mut table_state = TableState::default().with_selected(Some(app.proc_selected));
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn render_machines(frame: &mut Frame, app: &App, area: Rect) {
    let machines = app.machines();
    if machines.is_empty() {
        let p = Paragraph::new(" No remote builders configured")
            .style(Style::default().fg(GRV_GRAY))
            .block(Block::default().borders(Borders::ALL).title("Remote Builders")
                .border_style(Style::default().fg(GRV_GRAY)));
        frame.render_widget(p, area);
        return;
    }

    let rows: Vec<Row> = machines
        .iter()
        .map(|m| {
            Row::new(vec![
                Cell::from(short_uri(&m.store_uri)),
                Cell::from(format!("{}/{}", m.active_slots, m.max_jobs)),
                Cell::from(format!("x{:.1}", m.speed_factor)),
                Cell::from(m.system_types.join(",")),
            ])
        })
        .collect();

    let header = Row::new(vec![
        Cell::from("URI"),
        Cell::from("Slots"),
        Cell::from("Speed"),
        Cell::from("Systems"),
    ])
    .style(Style::default().bold());

    let table = Table::new(
        rows,
        [
            Constraint::Min(20),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Min(15),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title("Remote Builders")
        .border_style(Style::default().fg(GRV_GRAY)));

    frame.render_widget(table, area);
}

fn render_history(frame: &mut Frame, app: &App, area: Rect) {
    let history = app.history();
    if history.is_empty() {
        let p = Paragraph::new(" No completed builds yet")
            .style(Style::default().fg(GRV_GRAY))
            .block(Block::default().borders(Borders::ALL).title("History (h to close)")
                .border_style(Style::default().fg(GRV_GRAY)));
        frame.render_widget(p, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from(" Derivation"),
        Cell::from("Result"),
        Cell::from("Duration"),
        Cell::from("CPU usr"),
        Cell::from("CPU sys"),
        Cell::from("User"),
    ])
    .style(Style::default().bold());

    let rows: Vec<Row> = history
        .iter()
        .map(|c| {
            let name = drv_display_name(&c.build);
            let (result_text, result_style) = if c.success {
                ("ok", Style::default().fg(GRV_GREEN))
            } else {
                ("FAIL", Style::default().fg(GRV_RED).bold())
            };
            let duration = format_duration_secs(c.duration.as_secs());
            let cpu_usr = format_duration_secs(c.cpu_user_total_us / 1_000_000);
            let cpu_sys = format_duration_secs(c.cpu_system_total_us / 1_000_000);
            let user = c.build.user.as_deref().unwrap_or("-");

            Row::new(vec![
                Cell::from(format!(" {name}")),
                Cell::from(result_text).style(result_style),
                Cell::from(duration),
                Cell::from(cpu_usr),
                Cell::from(cpu_sys),
                Cell::from(user.to_string()),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(25),     // Derivation
            Constraint::Length(6),    // Result
            Constraint::Length(8),    // Duration
            Constraint::Length(8),    // CPU usr
            Constraint::Length(8),    // CPU sys
            Constraint::Length(8),    // User
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title("History (h to close)")
        .border_style(Style::default().fg(GRV_GRAY)));

    frame.render_widget(table, area);
}

fn format_duration_secs(secs: u64) -> String {
    let mins = secs / 60;
    let secs = secs % 60;
    if mins > 0 {
        format!("{mins}m{secs:02}s")
    } else {
        format!("{secs}s")
    }
}

fn render_log(frame: &mut Frame, app: &App, area: Rect) {
    let title = app
        .selected_build()
        .map(|b| format!("Log: {} ({})", drv_display_name(b), b.phase.as_deref().unwrap_or("?")))
        .unwrap_or_else(|| "Log".to_string());

    // Visible height inside the block (minus borders).
    let inner_height = area.height.saturating_sub(2) as usize;

    // Scroll: log_scroll=0 means show latest (bottom), higher values scroll up.
    let total = app.log_lines.len();
    let end = total.saturating_sub(app.log_scroll);
    let start = end.saturating_sub(inner_height);

    let text: Vec<Line> = app.log_lines[start..end]
        .iter()
        .map(|l| Line::from(format!(" > {l}")))
        .collect();

    let scroll_indicator = if app.log_scroll > 0 {
        format!(" (scroll: +{}, [/] to scroll)", app.log_scroll)
    } else {
        String::new()
    };

    let paragraph = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(format!("{title}{scroll_indicator}"))
            .border_style(Style::default().fg(GRV_GRAY)))
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

/// Extract a short display name from a derivation path.
/// "/nix/store/abc...-firefox-128.0.drv" → "firefox-128.0"
fn drv_display_name(build: &Build) -> String {
    // Show the nix command line only for container root types.
    if let Some(ref cmd) = build.command_line {
        if matches!(
            build.activity_type,
            ActivityType::Builds | ActivityType::CopyPaths
        ) {
            return cmd.clone();
        }
    }
    if let Some(ref drv) = build.drv_path {
        drv.rsplit('/')
            .next()
            .and_then(|name| name.split_once('-').map(|(_, rest)| rest))
            .map(|s| s.trim_end_matches(".drv").to_string())
            .unwrap_or_else(|| drv.clone())
    } else if !build.description.is_empty() {
        let desc = &build.description;
        if looks_like_bare_filename(desc) {
            format!("[{}] {}", build.activity_type, desc)
        } else {
            desc.clone()
        }
    } else {
        // For container types (Builds, CopyPaths, etc.) show a readable label
        // instead of a raw activity ID.
        match build.activity_type {
            ActivityType::Builds => "builds".to_string(),
            ActivityType::CopyPaths => "copy paths".to_string(),
            ActivityType::Realise => "realise".to_string(),
            ActivityType::VerifyPaths => "verify paths".to_string(),
            ActivityType::OptimiseStore => "optimise store".to_string(),
            _ => format!("activity-{}", build.activity_id),
        }
    }
}

/// Check if a string looks like a bare filename (no spaces, ends with common archive extension).
fn looks_like_bare_filename(s: &str) -> bool {
    const ARCHIVE_EXTS: &[&str] = &[
        ".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst", ".tgz", ".tbz2",
        ".zip", ".gz", ".xz", ".bz2", ".zst",
    ];
    !s.contains(' ') && ARCHIVE_EXTS.iter().any(|ext| s.ends_with(ext))
}

/// Shorten a store URI for display.
/// "ssh-ng://builder.internal" → "builder.internal"
fn short_uri(uri: &str) -> String {
    uri.split("://")
        .nth(1)
        .unwrap_or(uri)
        .to_string()
}

fn format_duration_us(us: u64) -> String {
    let secs = us / 1_000_000;
    let mins = secs / 60;
    let secs = secs % 60;
    if mins > 0 {
        format!("{mins}m{secs:02}s")
    } else {
        format!("{secs}s")
    }
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.0}M", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.0}K", bytes as f64 / 1024.0)
    } else {
        format!("{bytes}B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix_analytics_common::event::ActivityType;
    use nix_analytics_common::types::Progress;
    use std::collections::VecDeque;

    fn make_build(id: u64, drv_path: Option<&str>, description: &str) -> Build {
        Build {
            activity_id: id,
            activity_type: ActivityType::Build,
            drv_path: drv_path.map(String::from),
            description: description.to_string(),
            parent_id: None,
            command_line: None,
            started_at_us: 1000,
            phase: None,
            recent_log: VecDeque::new(),
            user: None,
            user_pid: None,
            user_uid: None,
            cgroup_path: None,
            cpu_user_us: 0,
            cpu_system_us: 0,
            memory_current: None,
            progress: None,
            machine: BuildMachine::Local,
            is_frozen: false,
        }
    }

    #[test]
    fn drv_display_name_with_drv_path() {
        let build = make_build(1, Some("/nix/store/abc123-firefox-128.0.drv"), "building firefox");
        assert_eq!(drv_display_name(&build), "firefox-128.0");
    }

    #[test]
    fn drv_display_name_without_drv_path_falls_back_to_description() {
        let build = make_build(1, None, "fetching something");
        assert_eq!(drv_display_name(&build), "fetching something");
    }

    #[test]
    fn drv_display_name_neither_falls_back_to_activity_id() {
        let build = make_build(42, None, "");
        assert_eq!(drv_display_name(&build), "activity-42");
    }

    #[test]
    fn short_uri_strips_scheme() {
        assert_eq!(short_uri("ssh-ng://builder.internal"), "builder.internal");
        assert_eq!(short_uri("ssh://host"), "host");
    }

    #[test]
    fn short_uri_no_scheme() {
        assert_eq!(short_uri("localhost"), "localhost");
    }

    #[test]
    fn format_duration_us_zero() {
        assert_eq!(format_duration_us(0), "0s");
    }

    #[test]
    fn format_duration_us_seconds_only() {
        assert_eq!(format_duration_us(45_000_000), "45s");
    }

    #[test]
    fn format_duration_us_minutes_and_seconds() {
        assert_eq!(format_duration_us(90_000_000), "1m30s");
    }

    #[test]
    fn format_bytes_bytes() {
        assert_eq!(format_bytes(512), "512B");
    }

    #[test]
    fn format_bytes_kilobytes() {
        assert_eq!(format_bytes(1024), "1K");
        assert_eq!(format_bytes(2048), "2K");
    }

    #[test]
    fn format_bytes_megabytes() {
        assert_eq!(format_bytes(1024 * 1024), "1M");
        assert_eq!(format_bytes(50 * 1024 * 1024), "50M");
    }

    #[test]
    fn format_bytes_gigabytes() {
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0G");
        assert_eq!(format_bytes(4 * 1024 * 1024 * 1024), "4.0G");
    }

    // --- Tree view helpers and tests ---

    fn make_tree_build(
        id: u64,
        activity_type: ActivityType,
        parent_id: Option<u64>,
        started_at_us: u64,
    ) -> Build {
        let mut b = make_build(id, None, "");
        b.activity_type = activity_type;
        b.parent_id = parent_id;
        b.started_at_us = started_at_us;
        b.description = format!("activity-{id}");
        b
    }

    #[test]
    fn tree_order_empty() {
        let result = tree_order_builds(vec![]);
        assert!(result.builds.is_empty());
        assert!(result.had_realise_parent.is_empty());
    }

    #[test]
    fn tree_order_single_root() {
        let builds = vec![make_tree_build(1, ActivityType::Build, None, 1000)];
        let result = tree_order_builds(builds);
        assert_eq!(result.builds.len(), 1);
        assert_eq!(result.builds[0].activity_id, 1);
    }

    #[test]
    fn tree_order_parent_child() {
        // Child before parent in input — output should be parent first.
        let builds = vec![
            make_tree_build(2, ActivityType::Build, Some(1), 2000),
            make_tree_build(1, ActivityType::Builds, None, 1000),
        ];
        let result = tree_order_builds(builds);
        assert_eq!(result.builds.len(), 2);
        assert_eq!(result.builds[0].activity_id, 1);
        assert_eq!(result.builds[1].activity_id, 2);
    }

    #[test]
    fn tree_order_orphan_is_root() {
        // Build with parent_id pointing to non-existent activity, and no Builds root
        // to adopt it, becomes a root.
        let builds = vec![make_tree_build(5, ActivityType::Unknown, Some(999), 1000)];
        let result = tree_order_builds(builds);
        assert_eq!(result.builds.len(), 1);
        assert_eq!(result.builds[0].activity_id, 5);
    }

    #[test]
    fn tree_order_orphan_adopted_by_builds_root() {
        // Simulates the common case: a Realise activity has finished (gone from
        // active set), leaving its child Build orphaned. The orphan should be
        // adopted under the Builds-type root.
        let builds = vec![
            make_tree_build(1, ActivityType::Builds, None, 1000),
            make_tree_build(2, ActivityType::Build, Some(999), 2000), // parent 999 is gone
        ];
        let result = tree_order_builds(builds);
        assert_eq!(result.builds.len(), 2);
        assert_eq!(result.builds[0].activity_id, 1);
        assert_eq!(result.builds[1].activity_id, 2);
        // Orphan should be adopted under Builds root.
        assert_eq!(result.builds[1].parent_id, Some(1));
    }

    #[test]
    fn tree_order_parentless_build_adopted_by_builds_root() {
        // Build activities with parent_id=None should also be adopted under
        // the Builds root (Nix sometimes emits them with parent_id=0).
        let builds = vec![
            make_tree_build(1, ActivityType::Builds, None, 1000),
            make_tree_build(2, ActivityType::Build, None, 2000),
            make_tree_build(3, ActivityType::Build, None, 3000),
        ];
        let result = tree_order_builds(builds);
        assert_eq!(result.builds.len(), 3);
        assert_eq!(result.builds[0].activity_id, 1);
        // Both Build activities should be children of the Builds root.
        assert_eq!(result.builds[1].parent_id, Some(1));
        assert_eq!(result.builds[2].parent_id, Some(1));
    }

    #[test]
    fn tree_order_realise_collapsed() {
        // Realise node (id=2) with a child (id=3) → Realise removed, child re-parented to 1.
        let builds = vec![
            make_tree_build(1, ActivityType::Builds, None, 1000),
            make_tree_build(2, ActivityType::Realise, Some(1), 2000),
            make_tree_build(3, ActivityType::Build, Some(2), 3000),
        ];
        let result = tree_order_builds(builds);
        assert_eq!(result.builds.len(), 2);
        assert_eq!(result.builds[0].activity_id, 1);
        assert_eq!(result.builds[1].activity_id, 3);
        // Child 3 should be re-parented to 1.
        assert_eq!(result.builds[1].parent_id, Some(1));
        // Child 3 should be in had_realise_parent.
        assert!(result.had_realise_parent.contains(&3));
    }

    #[test]
    fn tree_order_childless_realise_kept() {
        // Realise node with no children stays in the tree.
        let builds = vec![
            make_tree_build(1, ActivityType::Builds, None, 1000),
            make_tree_build(2, ActivityType::Realise, Some(1), 2000),
        ];
        let result = tree_order_builds(builds);
        assert_eq!(result.builds.len(), 2);
        let ids: Vec<u64> = result.builds.iter().map(|b| b.activity_id).collect();
        assert!(ids.contains(&2));
    }

    #[test]
    fn tree_prefixes_roots_no_prefix() {
        let builds = vec![
            make_tree_build(1, ActivityType::Build, None, 1000),
            make_tree_build(2, ActivityType::Build, None, 2000),
        ];
        let result = tree_order_builds(builds);
        let prefixes = compute_tree_prefixes(&result.builds);
        assert_eq!(prefixes[0], "");
        assert_eq!(prefixes[1], "");
    }

    #[test]
    fn tree_prefixes_single_child() {
        let builds = vec![
            make_tree_build(1, ActivityType::Builds, None, 1000),
            make_tree_build(2, ActivityType::Build, Some(1), 2000),
        ];
        let result = tree_order_builds(builds);
        let prefixes = compute_tree_prefixes(&result.builds);
        assert_eq!(prefixes[0], "");
        assert_eq!(prefixes[1], "└─ ");
    }

    #[test]
    fn tree_prefixes_two_children() {
        let builds = vec![
            make_tree_build(1, ActivityType::Builds, None, 1000),
            make_tree_build(2, ActivityType::Build, Some(1), 2000),
            make_tree_build(3, ActivityType::Build, Some(1), 3000),
        ];
        let result = tree_order_builds(builds);
        let prefixes = compute_tree_prefixes(&result.builds);
        assert_eq!(prefixes[0], "");       // root
        assert_eq!(prefixes[1], "├─ ");    // first child
        assert_eq!(prefixes[2], "└─ ");    // last child
    }

    #[test]
    fn tree_prefixes_nested() {
        // Root → child A (not last) → grandchild
        //      → child B (last)
        let builds = vec![
            make_tree_build(1, ActivityType::Builds, None, 1000),
            make_tree_build(2, ActivityType::Build, Some(1), 2000),
            make_tree_build(3, ActivityType::Build, Some(1), 4000),
            make_tree_build(4, ActivityType::Build, Some(2), 3000),
        ];
        let result = tree_order_builds(builds);
        let prefixes = compute_tree_prefixes(&result.builds);
        // Expected order: 1, 2, 4, 3
        assert_eq!(result.builds[0].activity_id, 1);
        assert_eq!(result.builds[1].activity_id, 2);
        assert_eq!(result.builds[2].activity_id, 4);
        assert_eq!(result.builds[3].activity_id, 3);

        assert_eq!(prefixes[0], "");           // root
        assert_eq!(prefixes[1], "├─ ");        // child A (not last)
        assert_eq!(prefixes[2], "│  └─ ");     // grandchild under A (A has sibling B)
        assert_eq!(prefixes[3], "└─ ");        // child B (last)
    }

    fn make_tree_build_with_cmd(
        id: u64,
        activity_type: ActivityType,
        parent_id: Option<u64>,
        started_at_us: u64,
        command_line: Option<&str>,
    ) -> Build {
        let mut b = make_tree_build(id, activity_type, parent_id, started_at_us);
        b.command_line = command_line.map(String::from);
        b
    }

    #[test]
    fn tree_order_containers_merged() {
        // Builds (id=1) and CopyPaths (id=2) share the same command_line.
        // They should be merged into one root (Builds preferred), and
        // CopyPaths's child should be reparented.
        let mut builds_root = make_tree_build_with_cmd(
            1,
            ActivityType::Builds,
            None,
            1000,
            Some("nix build ...#tdf"),
        );
        builds_root.description = String::new();

        let mut copy_root = make_tree_build_with_cmd(
            2,
            ActivityType::CopyPaths,
            None,
            1001,
            Some("nix build ...#tdf"),
        );
        copy_root.description = String::new();

        let mut child_build = make_tree_build(3, ActivityType::Build, Some(1), 2000);
        child_build.drv_path = Some("/nix/store/abc-tdf-1.0.drv".to_string());

        let mut child_copy = make_tree_build(4, ActivityType::CopyPath, Some(2), 2001);
        child_copy.description = "copying path /nix/store/xyz".to_string();

        let builds = vec![builds_root, copy_root, child_build, child_copy];
        let result = tree_order_builds(builds);

        // CopyPaths root (id=2) should be removed.
        let ids: Vec<u64> = result.builds.iter().map(|b| b.activity_id).collect();
        assert!(!ids.contains(&2), "CopyPaths container should be merged away");
        assert!(ids.contains(&1), "Builds container should be the command root");
        assert!(ids.contains(&3));
        assert!(ids.contains(&4));

        // Builds root should be a command root.
        assert!(result.command_root_ids.contains(&1));

        // Child of CopyPaths should be reparented to Builds root.
        let copy_child = result.builds.iter().find(|b| b.activity_id == 4).unwrap();
        assert_eq!(copy_child.parent_id, Some(1));
    }

    #[test]
    fn tree_order_filetransfer_hoisted() {
        // A Build (id=2) with exactly one FileTransfer child (id=3).
        // The FileTransfer should be removed and the parent flagged as hoisted_download.
        let builds_root = make_tree_build_with_cmd(
            1,
            ActivityType::Builds,
            None,
            1000,
            Some("nix build ...#tdf"),
        );

        let mut substitute = make_tree_build(2, ActivityType::Substitute, Some(1), 2000);
        substitute.drv_path = Some("/nix/store/abc-zlib-1.3.1.drv".to_string());

        let mut ft = make_tree_build(3, ActivityType::FileTransfer, Some(2), 2001);
        ft.progress = Some(Progress {
            done: 12 * 1024 * 1024,
            expected: 50 * 1024 * 1024,
            running: 1,
            failed: 0,
        });

        let builds = vec![builds_root, substitute, ft];
        let result = tree_order_builds(builds);

        // FileTransfer (id=3) should be removed.
        let ids: Vec<u64> = result.builds.iter().map(|b| b.activity_id).collect();
        assert!(!ids.contains(&3), "FileTransfer should be hoisted away");

        // Parent (id=2) should be in hoisted_download.
        assert!(result.hoisted_download.contains(&2));

        // Hoisted progress should contain the FileTransfer's progress.
        let prog = result.hoisted_progress.get(&2).unwrap();
        assert_eq!(prog.done, 12 * 1024 * 1024);
        assert_eq!(prog.expected, 50 * 1024 * 1024);
    }

    #[test]
    fn tree_order_non_container_toplevel_grouped() {
        // Two non-container top-level activities with the same command_line
        // (e.g., "evaluating derivation" and "copying to store" during eval phase).
        // No Builds/CopyPaths container exists yet.
        // A synthetic command root should be created and both become children.
        let mut eval = make_tree_build_with_cmd(
            1,
            ActivityType::Unknown,
            None,
            1000,
            Some("nix build ...#tdf"),
        );
        eval.description = "evaluating derivation 'github:nixOS/nixpkgs#tdf'".to_string();

        let mut copy = make_tree_build_with_cmd(
            2,
            ActivityType::CopyPaths,
            None,
            1001,
            Some("nix build ...#tdf"),
        );
        copy.description = "copying '...' to the store".to_string();

        let builds = vec![eval, copy];
        let result = tree_order_builds(builds);

        // Should have 3 items: synthetic root + 2 children.
        // The CopyPaths (id=2) becomes the command root since it's a container type.
        // Actually — CopyPaths IS a container type, so it should be picked as the representative.
        let root = result
            .builds
            .iter()
            .find(|b| result.command_root_ids.contains(&b.activity_id))
            .expect("should have a command root");
        assert_eq!(root.command_line.as_deref(), Some("nix build ...#tdf"));

        // The non-container activity (eval, id=1) should be a child of the root.
        let eval_build = result.builds.iter().find(|b| b.activity_id == 1).unwrap();
        assert_eq!(eval_build.parent_id, Some(root.activity_id));
    }

    #[test]
    fn tree_order_single_non_container_gets_virtual_root() {
        // A single non-container top-level activity with command_line.
        // Should get a synthetic command root.
        let mut eval = make_tree_build_with_cmd(
            1,
            ActivityType::Unknown,
            None,
            1000,
            Some("nix build ...#tdf"),
        );
        eval.description = "evaluating derivation 'github:nixOS/nixpkgs#tdf'".to_string();

        let builds = vec![eval];
        let result = tree_order_builds(builds);

        // Should have 2 items: synthetic root + the original.
        assert_eq!(result.builds.len(), 2);

        let root = result
            .builds
            .iter()
            .find(|b| result.command_root_ids.contains(&b.activity_id))
            .expect("should have a command root");
        assert_eq!(root.command_line.as_deref(), Some("nix build ...#tdf"));

        let eval_build = result.builds.iter().find(|b| b.activity_id == 1).unwrap();
        assert_eq!(eval_build.parent_id, Some(root.activity_id));
    }

    #[test]
    fn tree_order_multi_command() {
        // Two Builds containers with different command_lines → two separate roots.
        let root1 = make_tree_build_with_cmd(
            1,
            ActivityType::Builds,
            None,
            1000,
            Some("nix build ...#tdf"),
        );
        let root2 = make_tree_build_with_cmd(
            2,
            ActivityType::Builds,
            None,
            1001,
            Some("nix build ...#hello"),
        );

        let mut child1 = make_tree_build(3, ActivityType::Build, Some(1), 2000);
        child1.drv_path = Some("/nix/store/abc-tdf-1.0.drv".to_string());

        let mut child2 = make_tree_build(4, ActivityType::Build, Some(2), 2001);
        child2.drv_path = Some("/nix/store/abc-hello-2.12.drv".to_string());

        let builds = vec![root1, root2, child1, child2];
        let result = tree_order_builds(builds);

        // Both roots should remain.
        let ids: Vec<u64> = result.builds.iter().map(|b| b.activity_id).collect();
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));
        assert_eq!(result.builds.len(), 4);

        // Children should still be under their respective parents.
        let c1 = result.builds.iter().find(|b| b.activity_id == 3).unwrap();
        let c2 = result.builds.iter().find(|b| b.activity_id == 4).unwrap();
        assert_eq!(c1.parent_id, Some(1));
        assert_eq!(c2.parent_id, Some(2));
    }
}
