use std::collections::{hash_map::Entry, HashMap, HashSet};

use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System, Users};

pub fn get_nix_users(users: &Users) -> HashSet<String> {
    users
        .list()
        .into_iter()
        .map(|u| u.name().to_string())
        .filter(|x| x.contains("nixbld"))
        .collect()
}

pub fn get_active_users_and_pids() -> HashMap<String, HashSet<Pid>> {
    let users = Users::new_with_refreshed_list();
    let nix_users = get_nix_users(&users);
    let mut map = HashMap::<String, HashSet<Pid>>::new();
    let mut system = System::new_all();

    //     new_with_specifics(
    //     RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
    // );

    system
        .processes()
        .into_iter()
        .filter_map(move |(pid, proc)| {
            println!("pid: {pid:?} proc: {proc:?}");
            let user_id = proc.effective_user_id()?;
            let user = users.get_user_by_id(&user_id)?;
            let name = user.name().to_string();
            // println!("name: {:?}, pid {}, proc {:?}", name, pid, proc);
            nix_users.contains(&name).then(|| (name, pid))
        })
        .for_each(|(name, pid)| match map.entry(name) {
            Entry::Occupied(mut o) => {
                let entry: &mut HashSet<Pid> = o.get_mut();
                entry.insert(*pid);
            }
            Entry::Vacant(v) => {
                let mut entry_new = HashSet::new();
                entry_new.insert(*pid);
                v.insert(entry_new);
            }
        });
    map
}
