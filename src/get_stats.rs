// note: bailing on btreemap because I want sorted by builder number, not string
use std::{
    cell::RefCell,
    cmp::Ordering,
    collections::{hash_map::Entry, BTreeSet, HashMap, HashSet},
    ops::Deref,
};

#[allow(clippy::unnecessary_literal_unwrap)]
pub fn nll_todo<T>() -> T {
    None.unwrap()
}

use derivative::Derivative;
use lazy_static::lazy_static;

use ratatui::text::Text;
use sysinfo::{Pid, Process, System, Users};
use tui_tree_widget::TreeItem;

lazy_static! {
    /// This is an example for using doc comment attributes
    pub static ref NIX_USERS: HashSet<String> = {
        get_nix_users(&USERS).into_iter().collect()
    };
    pub static ref USERS: Users = {
        Users::new_with_refreshed_list()
    };
    pub static ref SORTED_NIX_USERS: Vec<String> = {
        get_sorted_nix_users()
    };
}

pub fn get_nix_users(users: &Users) -> HashSet<String> {
    users
        .list()
        .iter()
        .map(|u| u.name().to_string())
        .filter(|x| x.contains("nixbld"))
        .collect()
}

pub fn get_sorted_nix_users() -> Vec<String> {
    let mut nix_users: Vec<_> = Deref::deref(&NIX_USERS).iter().cloned().collect();
    nix_users.sort_by(|x, y| {
        let offset = if x.starts_with('_') { 7 } else { 6 };
        let x_num: usize = x[offset..].parse().unwrap();
        let y_num: usize = y[offset..].parse().unwrap();
        x_num.partial_cmp(&y_num).unwrap()
    });
    nix_users
}

#[derive(Debug, Clone, Hash)]
pub struct ProcMetadata {
    pub id: Pid,
    pub name: String,
    pub env: Vec<String>,
    pub parent: Option<Pid>,
    pub p_mem: u64,
    pub v_mem: u64,
    pub run_time: u64,
    pub cmd: Vec<String>,
}

impl PartialEq for ProcMetadata {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl PartialOrd for ProcMetadata {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.id.cmp(&other.id))
    }
}

impl Ord for ProcMetadata {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id.cmp(&other.id)
    }
}

impl Eq for ProcMetadata {}

pub fn from_proc(proc: &Process) -> Option<ProcMetadata> {
    let user_id = proc.effective_user_id()?;
    let user = Deref::deref(&USERS).get_user_by_id(user_id)?;
    let uname = user.name().to_string();
    let pid = proc.pid();
    Some(ProcMetadata {
        name: uname,
        id: pid,
        env: proc.environ().into(),
        // ignore this is useless
        parent: proc.parent(), /* .map(|p| p.to_string()), */
        p_mem: proc.memory(),
        v_mem: proc.virtual_memory(),
        run_time: proc.run_time(),
        cmd: proc.cmd().into(),
    })
}

pub fn get_active_users_and_pids() -> HashMap<String, BTreeSet<ProcMetadata>> {
    let mut map = HashMap::<String, BTreeSet<ProcMetadata>>::new();
    for user in Deref::deref(&NIX_USERS) {
        map.insert(user.to_string(), BTreeSet::default());
    }
    let system = System::new_all();

    // requires sudo to work on macos anyway
    // might as well assume that you have root
    system
        .processes()
        .iter()
        .filter_map(move |(_pid, proc)| {
            let pd = from_proc(proc)?;
            NIX_USERS.contains(&pd.name).then_some({
                (
                    pd.name.clone(),
                    // TODO should probably query on-demand instead of carrying all this around
                    from_proc(proc)?,
                )
            })
        })
        .for_each(|(name, proc_metadata)| {
            // println!("{:?}", proc_metadata);
            match map.entry(name) {
                Entry::Occupied(mut o) => {
                    let entry: &mut BTreeSet<ProcMetadata> = o.get_mut();
                    entry.insert(proc_metadata);
                }
                Entry::Vacant(_v) => {
                    unreachable!("How did this happen");
                }
            };
        });
    map
}

// TODO what we should have is two views
// the proc metadata should be in one view
// the treenode should only contain pids
// and point to the metadata, which can be looked up elsewhere

#[derive(Derivative, Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    proc_metadata: ProcMetadata,
    #[derivative(PartialEq = "ignore")]
    children: HashSet<TreeNode>,
}

impl std::hash::Hash for TreeNode {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.proc_metadata.id.hash(state);
    }
}

pub fn merge_trees(t1: &mut TreeNode, t2: &TreeNode) {
    let t1_cur = t1;
    let t2_cur = t2;

    let t1_childs = &mut t1_cur.children;
    let t2_childs = &t2_cur.children;

    for t2_child in t2_childs {
        if t1_childs.contains(t2_child) {
            let mut t1_child = t1_childs.take(t2_child).unwrap();
            merge_trees(&mut t1_child, t2_child);
            t1_childs.insert(t1_child);
        } else {
            t1_childs.insert(t2_child.clone());
        }
    }
}

pub fn construct_tree(new_procs: HashSet<ProcMetadata>) -> HashMap<Pid, TreeNode> {
    let mut roots = HashMap::<Pid, TreeNode>::new();
    'top: for proc in new_procs {
        let mut cur_proc = proc.clone();
        let mut proc_subtree: HashSet<TreeNode> = HashSet::new();
        loop {
            match cur_proc.parent {
                Some(p_pid) => {
                    // TODO new_procs should be hashmap pid -> val, go off that so we don't need to
                    // look up every time
                    let mut system = System::new_all();
                    system.refresh_all();
                    match system.process(p_pid) {
                        Some(p_metadata) => {
                            let pd = from_proc(p_metadata).unwrap();
                            let mut new_proc_subtree = HashSet::new();
                            new_proc_subtree.insert(TreeNode {
                                proc_metadata: cur_proc.clone(),
                                children: proc_subtree,
                            });
                            proc_subtree = new_proc_subtree;
                            cur_proc = pd;
                        }
                        None => {
                            // we've failed. parent no longer exists
                            continue;
                        }
                    }
                }
                None => {
                    let root_pid = cur_proc.id;
                    let new_tree_root = TreeNode {
                        proc_metadata: cur_proc,
                        // the root should always be one up from what we are iterating on
                        children: proc_subtree,
                    };
                    if let Entry::Vacant(e) = roots.entry(root_pid) {
                        e.insert(new_tree_root);
                    } else {
                        let cur_root_tree = roots.get_mut(&root_pid).unwrap();
                        merge_trees(cur_root_tree, &new_tree_root);
                    }
                    continue 'top;
                }
            }
        }
    }
    roots
}

pub fn update_nix_builder_set(
    mut nix_builder_sets: &mut HashMap<String, BTreeSet<ProcMetadata>>,
    new_proc_list: BTreeSet<ProcMetadata>,
) {
}

pub fn gen_ui_by_parent_proc(root: &TreeNode) -> Vec<TreeItem<'_, String>> {
    todo!()
}

// TODO there's definitely some optimization here to not query/process every time
// probably need to introduce some global state that we tweak every time
// utilizing refcell
pub fn gen_ui_by_nix_builder(
    user_map: &HashMap<String, BTreeSet<ProcMetadata>>,
) -> Vec<TreeItem<'_, String>> {
    let mut r_vec = Vec::new();

    let mut sorted_user_map: Vec<_> = user_map.iter().collect();

    // TODO refactor to a function, pass in to this function, ...
    sorted_user_map.sort_by(|&x, &y| {
        let offset = if x.0.starts_with('_') { 7 } else { 6 };
        let x_num: usize = x.0[offset..].parse().unwrap();
        let y_num: usize = y.0[offset..].parse().unwrap();
        x_num.partial_cmp(&y_num).unwrap()
    });

    for (user, map) in sorted_user_map {
        let mut leaves = Vec::new();
        for pid in map {
            // gross there's definitely a better way
            let t_pid = Text::from(pid.id.to_string());
            leaves.push(TreeItem::new_leaf(pid.id.to_string(), t_pid));
        }
        let t_user = Text::from(format!("{} ({})", user.clone(), map.len()));
        let root = TreeItem::new(user.clone(), t_user, leaves).unwrap();
        r_vec.push(root);
    }

    r_vec
}
