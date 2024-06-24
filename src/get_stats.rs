// note: bailing on btreemap because I want sorted by builder number, not string
use std::{cmp::Ordering, collections::{hash_map::Entry, BTreeSet, HashMap, HashSet}};

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

pub fn get_active_users_and_pids() -> HashMap<String, BTreeSet<Pid>> {
    let users = Users::new_with_refreshed_list();
    let nix_users = get_nix_users(&users);
    let mut map = HashMap::<String, BTreeSet<Pid>>::new();
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
            nix_users.contains(&name).then_some((name, pid, proc))
        })
        .for_each(|(name, pid, proc)| {
            println!("pid: {pid:?} proc: {proc:?}");
            match map.entry(name) {
            Entry::Occupied(mut o) => {
                let entry: &mut BTreeSet<Pid> = o.get_mut();
                entry.insert(*pid);
            }
            Entry::Vacant(v) => {
                let mut entry_new = BTreeSet::new();
                entry_new.insert(*pid);
                v.insert(entry_new);
            }
        }});
    map
}

// TODO there's definitely some optimization here to not query/process every time
pub fn gen_tree(user_map: &HashMap<String, BTreeSet<Pid>>)
    -> Vec<TreeItem<'_, String>>
{

    let mut r_vec = Vec::new();

    let mut sorted_user_map : Vec<_> = user_map.iter().collect();

    // TODO refactor to a function, pass in to this function, ...
    sorted_user_map.sort_by(|&x, &y| {
        let x_num : usize = x.0[6..].parse().unwrap();
        let y_num : usize = y.0[6..].parse().unwrap();
        x_num.partial_cmp(&y_num).unwrap()
    });

    for (user, map) in sorted_user_map {
        let mut leaves = Vec::new();
        for pid in map {
            // gross there's definitely a better way
            let t_pid = Text::from(pid.to_string());
            leaves.push(TreeItem::new_leaf(pid.to_string(), t_pid));
        }
        let t_user = Text::from(format!("{} ({})", user.clone(), map.len()));
        let root = TreeItem::new(user.clone(), t_user, leaves).unwrap();
        r_vec.push(root);
    }

    r_vec
}

