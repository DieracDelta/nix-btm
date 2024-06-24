// note: bailing on btreemap because I want sorted by builder number, not string
use std::{
    cmp::Ordering,
    collections::{hash_map::Entry, BTreeSet, HashMap, HashSet},
};

use ratatui::text::Text;
use sysinfo::{Pid, System, Users};
use tui_tree_widget::TreeItem;

pub fn get_nix_users(users: &Users) -> HashSet<String> {
    users
        .list()
        .iter()
        .map(|u| u.name().to_string())
        .filter(|x| x.contains("nixbld"))
        .collect()
}

#[derive(Debug, Clone)]
pub struct ProcMetadata {
    pub id: Pid,
    pub env: Vec<String>,
    pub parent: String,
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

pub fn get_active_users_and_pids() -> HashMap<String, BTreeSet<ProcMetadata>> {
    let users = Users::new_with_refreshed_list();
    let nix_users = get_nix_users(&users);
    let mut map = HashMap::<String, BTreeSet<ProcMetadata>>::new();
    for user in &nix_users {
        map.insert(user.to_string(), BTreeSet::default());
    }
    let system = System::new_all();

    // requires sudo to work on macos anyway
    // might as well assume that you have root
    system
        .processes()
        .iter()
        .filter_map(move |(pid, proc)| {
            let user_id = proc.effective_user_id()?;
            let user = users.get_user_by_id(user_id)?;
            let name = user.name().to_string();
            // println!("name: {:?}, pid {}, proc {:?}", name, pid, proc);
            nix_users.contains(&name).then_some((
                name,
                // TODO should probably query on-demand instead of carrying all this around
                ProcMetadata {
                    id: *pid,
                    env: proc.environ().into(),
                    // ignore this is useless
                    parent: proc
                        .parent()
                        .map(|p| p.to_string())
                        .unwrap_or("".to_string()),
                    p_mem: proc.memory(),
                    v_mem: proc.virtual_memory(),
                    run_time: proc.run_time(),
                    cmd: proc.cmd().into(),
                },
            ))
        })
        .for_each(|(name, proc_metadata)| {
            // println!("{:?}", proc_metadata);
            match map.entry(name) {
                Entry::Occupied(mut o) => {
                    let entry: &mut BTreeSet<ProcMetadata> = o.get_mut();
                    entry.insert(proc_metadata);
                }
                Entry::Vacant(v) => {
                    // TODO nuke this case it's never hit since key is pre-inserted
                    let mut entry_new = BTreeSet::new();
                    entry_new.insert(proc_metadata);
                    v.insert(entry_new);
                }
            };
        });
    map
}

// TODO there's definitely some optimization here to not query/process every time
pub fn gen_tree(user_map: &HashMap<String, BTreeSet<ProcMetadata>>) -> Vec<TreeItem<'_, String>> {
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
