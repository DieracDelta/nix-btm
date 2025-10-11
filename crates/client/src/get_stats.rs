// note: bailing on btreemap because I want sorted by builder number, not string
use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashMap, HashSet, VecDeque, hash_map::Entry},
    hash::Hash,
    ops::Deref,
    process::Command,
};

use procfs::process::Process as ProcFsProcess;

#[allow(clippy::unnecessary_literal_unwrap)]
pub fn nll_todo<T>() -> T {
    None.unwrap()
}

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
    let mut nix_users: Vec<_> =
        Deref::deref(&NIX_USERS).iter().cloned().collect();
    nix_users.sort_by(|x, y| {
        let offset = if x.starts_with('_') { 7 } else { 6 };
        let x_num: usize = x[offset..].parse().unwrap();
        let y_num: usize = y[offset..].parse().unwrap();
        x_num.partial_cmp(&y_num).unwrap()
    });
    nix_users
}

#[derive(Debug, Clone)]
pub struct ProcMetadata {
    pub id: Pid,
    pub owner: String,
    pub env: Vec<String>,
    pub parent: Option<Pid>,
    pub p_mem: u64,
    pub v_mem: u64,
    pub run_time: u64,
    pub cmd: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DrvRoot {
    pub drv: Drv,
    pub procs: TreeNode,
}

impl DrvRoot {
    pub fn new(drv: Drv, procs: TreeNode) -> Self {
        DrvRoot { drv, procs }
    }

    pub fn print_drv_root(&self) -> String {
        format!("{}^*", self.drv.drv)
    }
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord)]
pub struct Drv {
    pub drv: String,
    pub human_readable_drv: String,
}

impl std::hash::Hash for Drv {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.drv.hash(state);
    }
}

// possibly very cursed
impl Hash for ProcMetadata {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
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

pub fn construct_pid_map(
    set: HashSet<ProcMetadata>,
) -> HashMap<Pid, ProcMetadata> {
    let mut r_map = HashMap::new();
    for ele in set {
        r_map.insert(ele.id, ele);
    }
    r_map
}

pub fn from_proc(proc: &Process) -> Option<ProcMetadata> {
    let user_id = proc.effective_user_id()?;
    let user = Deref::deref(&USERS).get_user_by_id(user_id)?;
    let uname = user.name().to_string();
    let pid = proc.pid();
    Some(ProcMetadata {
        owner: uname,
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
            NIX_USERS.contains(&pd.owner).then_some({
                (
                    pd.owner.clone(),
                    // TODO should probably query on-demand instead of carrying
                    // all this around
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

#[derive(Clone, Debug)]
pub struct DrvNode {
    pub drv: Drv,
    pub children: HashSet<String>,
}

impl PartialEq for DrvNode {
    fn eq(&self, other: &Self) -> bool {
        self.drv == other.drv
    }
}

impl Eq for DrvNode {}

impl std::hash::Hash for DrvNode {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.drv.hash(state);
    }
}

// probably *could* make this generic and reuse it for the drv hierarchy
//
#[derive(Debug, Clone, Eq)]
pub struct TreeNode {
    pid: Pid,
    children: HashSet<TreeNode>,
}

#[derive(Debug, Clone)]
pub struct ThickerTreeNode<'a> {
    proc: &'a ProcMetadata,
    children: HashSet<ThickerTreeNode<'a>>,
}

impl<'a> PartialEq for ThickerTreeNode<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.proc.id == other.proc.id
    }
}

impl<'a> Eq for ThickerTreeNode<'a> {}

impl<'a> Hash for ThickerTreeNode<'a> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.proc.hash(state);
    }
}

impl PartialEq for TreeNode {
    fn eq(&self, other: &Self) -> bool {
        self.pid == other.pid
    }
}

impl Hash for TreeNode {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.pid.hash(state);
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

pub enum PidParent {
    DoesntExist,
    IsDead,
    IsAlive(Pid),
}

pub fn get_parent(
    pid: Pid,
    pid_map: &mut HashMap<Pid, ProcMetadata>,
) -> PidParent {
    // TODO a bit more finnicking.
    // Make pid_map mutable reference
    // perform a query if not in the pid map
    match pid_map.get(&pid) {
        Some(proc) => match proc.parent {
            Some(parent) => PidParent::IsAlive(parent),
            None => PidParent::DoesntExist,
        },
        None => {
            let mut system = System::new_all();
            system.refresh_all();
            match system.process(pid) {
                Some(proc) => {
                    // TODO sus to unwrap here. Should just have less info
                    pid_map.insert(pid, from_proc(proc).unwrap());
                    match proc.parent() {
                        Some(parent) => PidParent::IsAlive(parent),
                        None => PidParent::DoesntExist,
                    }
                }
                None => PidParent::IsDead,
            }
        }
    }

    //     .map(|proc| proc.parent).flatten() {
    //     Some(p_pid) => IsAlive(Pid),
    //     None => {
    //
    //     }
    // }
}

pub fn construct_tree(
    procs: HashSet<Pid>,
    pid_map: &mut HashMap<Pid, ProcMetadata>,
) -> HashMap<Pid, TreeNode> {
    let mut roots = HashMap::<Pid, TreeNode>::new();
    'top: for pid in procs {
        let mut cur_pid = pid.clone();
        let mut proc_subtree: HashSet<TreeNode> = HashSet::new();
        loop {
            match get_parent(cur_pid, pid_map) {
                PidParent::IsAlive(p_pid) => {
                    let mut new_proc_subtree = HashSet::new();
                    new_proc_subtree.insert(TreeNode {
                        pid: cur_pid.clone(),
                        children: proc_subtree,
                    });
                    cur_pid = p_pid;
                    proc_subtree = new_proc_subtree;
                }
                PidParent::DoesntExist => {
                    let root_pid = cur_pid;
                    let new_tree_root = TreeNode {
                        pid: cur_pid,
                        // the root should always be one up from what we are
                        // iterating on
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
                PidParent::IsDead => {
                    continue 'top;
                }
            }
        }
    }
    roots
}

pub fn update_nix_builder_set(
    nix_builder_sets: &mut HashMap<String, BTreeSet<ProcMetadata>>,
    new_proc_list: BTreeSet<ProcMetadata>,
) {
}

pub fn gen_ui_by_parent_proc(root: &TreeNode) -> Vec<TreeItem<'_, String>> {
    todo!()
}

// TODO there's definitely some optimization here to not query/process every
// time probably need to introduce some global state that we tweak every time
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

pub fn convert_to_thicker_tree_node<'a>(
    tree_node: &TreeNode,
    map: &'a HashMap<Pid, ProcMetadata>,
) -> ThickerTreeNode<'a> {
    let mut root = ThickerTreeNode {
        proc: map.get(&tree_node.pid).unwrap(),
        children: HashSet::new(),
    };
    for child in &tree_node.children {
        let tmp = convert_to_thicker_tree_node(child, map);
        root.children.insert(tmp);
    }
    root
}

pub fn dump_pids(
    tree_nodes: &HashMap<Pid, TreeNode>,
    map: &HashMap<Pid, ProcMetadata>,
) {
    for tree_node in tree_nodes {
        let thicker_tree_node = convert_to_thicker_tree_node(tree_node.1, map);
        println!("{thicker_tree_node:#?}");
    }
}

pub fn strip_tf_outta_tree(
    tree_node: TreeNode,
    _pid_map: &HashMap<Pid, ProcMetadata>,
) -> HashMap<Pid, TreeNode> {
    // go two levels deeper
    // pid_map passed in exactly for this purpose
    // TODO add in an assert
    //                 cmd: [ "nix-daemon", "--daemon",
    //                  "/run/current-system/systemd/lib/systemd/systemd",
    let real_roots = tree_node.children.into_iter().next().unwrap().children;
    let mut root_map = HashMap::new();
    real_roots.into_iter().for_each(|root| {
        root_map.insert(root.pid, root);
    });
    root_map
}

// function that converts string of form
// "/nix/var/log/nix/drvs/z4/ps207hnvyh0lsrlmgkqyyfj3bbf37l-helix-24.03.drv.bz2"
// to string of form
// "/nix/store/z4ps207hnvyh0lsrlmgkqyyfj3bbf37l-helix-24.03.drv"
fn bz2_to_drv(input: &str) -> String {
    let mut result = "/nix/store/".to_string();
    for ele in input.split('/') {
        match ele {
            "nix" | "var" | "log" | "drvs" => {}
            s => result.push_str(s),
        }
    }
    result.chars().take(result.len() - 4).collect()
}

fn drv_to_readable_drv(input: &str, has_postfix: bool) -> String {
    let mut result = "".to_string();
    for ele in input.split('/') {
        // println!("ele: {}", ele);
        match ele {
            "nix" | "store" | "" => {}
            s => {
                let mut start_recording = false;
                for c in s.chars() {
                    // println!("C: {}", c);
                    if !start_recording {
                        if c == '-' {
                            start_recording = true;
                        }
                    } else {
                        result.push(c);
                    }
                }
                let offset = if has_postfix { 4 } else { 0 };
                return result.chars().take(result.len() - offset).collect();
            }
        }
    }
    unreachable!()
}

// TODO error handling
// TODO macos support
pub fn create_drv_root(root: TreeNode) -> DrvRoot {
    let root_pid = root.pid;
    // this can totally fail
    let proc = ProcFsProcess::new(root_pid.as_u32() as i32).unwrap();
    let fds = proc.fd().unwrap();
    for fd in fds {
        let Ok(fd) = fd else { continue };
        match fd.target {
            procfs::process::FDTarget::Path(path) => {
                if path.to_str().unwrap().starts_with("/nix/var/log/nix/drvs/")
                {
                    let drv_name = bz2_to_drv(path.to_str().unwrap());
                    let readable = drv_to_readable_drv(&drv_name, true);
                    return DrvRoot {
                        drv: Drv {
                            drv: drv_name,
                            human_readable_drv: readable,
                        },
                        procs: root,
                    };
                }
            }
            _ => continue,
        }
    }
    unreachable!()
}

pub fn get_drvs(map: HashMap<Pid, TreeNode>) -> HashMap<Pid, DrvRoot> {
    map.into_iter()
        .map(|(k, v)| (k, create_drv_root(v)))
        .collect::<HashMap<_, _>>()
}

#[derive(Clone, Debug)]
pub struct DrvPath(Vec<Drv>);
impl Deref for DrvPath {
    type Target = Vec<Drv>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// might be better longer term to either make a query or statically link
pub fn invoke_why_depends(
    drv1: &Drv,
    drv2: &Drv,
) -> Option<(HashMap<String, DrvNode>, String)> {
    let output = Command::new("nix")
        .arg("why-depends")
        .arg(&drv1.drv)
        .arg(&drv2.drv)
        .output()
        .expect("Failed to execute command");

    let mut cur_node_id: Option<String> = None;
    let mut root = None;
    let mut all_nodes = HashMap::new();

    if output.status.success() {
        let path = strip_ansi_escapes::strip_str(
            String::from_utf8_lossy(&output.stdout).trim(),
        )
        .to_string()
        .replace(['└', '─'], "")
        .trim()
        .to_string();
        if path.contains("does not depend on") {
            return None;
        }

        for line in path.lines() {
            let drv = parse_drv(line);
            match cur_node_id {
                Some(tree_inner) => {
                    let new_node = DrvNode {
                        drv,
                        children: HashSet::default(),
                    };
                    let mut cur_node: DrvNode =
                        all_nodes.remove(&tree_inner).unwrap();
                    cur_node.children.insert(new_node.drv.drv.clone());
                    all_nodes.insert(tree_inner, cur_node);

                    cur_node_id = Some(new_node.drv.drv.clone());
                    all_nodes.insert(new_node.drv.drv.clone(), new_node);
                }
                None => {
                    root = Some(drv.drv.clone());
                    let new_node = DrvNode {
                        drv,
                        children: HashSet::new(),
                    };
                    cur_node_id = Some(new_node.drv.drv.clone());
                    all_nodes.insert(new_node.drv.drv.clone(), new_node);
                }
            }
        }
    }

    root.map(|t| (all_nodes, t))
}

fn parse_drv(line: &str) -> Drv {
    Drv {
        drv: line.to_string(),
        human_readable_drv: drv_to_readable_drv(line, false),
    }
}

fn dump_dep_tree((nodes, root_id): &(HashMap<String, DrvNode>, String)) {
    let mut node_ids = VecDeque::new();
    let level = 0;
    node_ids.push_front((root_id.clone(), level));
    while let Some((cur_node_id, cur_level)) = node_ids.pop_back() {
        let cur_node = nodes.get(&cur_node_id).unwrap();
        println!(
            "{} {}",
            "\t".repeat(cur_level),
            cur_node.drv.human_readable_drv
        );
        let childs: Vec<_> = cur_node
            .children
            .clone()
            .into_iter()
            .map(|id| (id, cur_level + 1))
            .collect();
        node_ids.extend(childs);
    }
}

// passed in a bunch of drvs, want to construct graph
pub fn create_dep_tree(
    input_drvs: HashSet<&Drv>,
) -> Vec<(HashMap<String, DrvNode>, String)> {
    let mut roots: Vec<(HashMap<String, DrvNode>, String)> = Vec::new();

    for drv1 in &input_drvs {
        for drv2 in &input_drvs {
            if *drv1 != *drv2 {
                let maybe_fragment = invoke_why_depends(drv1, drv2)
                    .or_else(|| invoke_why_depends(drv2, drv1));

                if let Some(frag) = maybe_fragment {
                    let mut found_subtree = false;
                    roots = roots
                        .into_iter()
                        .filter_map(|root| {
                            let new_node = merge_drv_trees(&frag, &root)
                                .or_else(|| merge_drv_trees(&root, &frag));
                            if new_node.is_some() {
                                found_subtree = true;
                            }
                            new_node
                        })
                        .collect();
                    if !found_subtree {
                        roots.push(frag);
                    }
                }
            }
        }
    }
    roots
}

// if drv2 is a subtree of drv1, create a new subtree that is the two subtrees
// merged together otherwise return None
pub fn merge_drv_trees(
    (drv1_nodes, drv1_root): &(HashMap<String, DrvNode>, String),
    (drv2_nodes, drv2_root): &(HashMap<String, DrvNode>, String),
) -> Option<(HashMap<String, DrvNode>, String)> {
    let mut nodes_to_search = {
        let mut deque = VecDeque::new();
        deque.push_back(drv1_root);
        deque
    };
    // bfs this mf
    while let Some(node_id) = nodes_to_search.pop_front() {
        // we have found the subtree position
        if node_id == drv2_root {
            let mut ret_nodes = drv1_nodes.clone();

            let mut stack = vec![(
                drv1_nodes.get(node_id).unwrap(),
                drv2_nodes.get(node_id).unwrap(),
                node_id.clone(),
            )];

            // ref_node is the node in the returned tree
            while let Some((node_1, node_2, ref_node)) = stack.pop() {
                let node_1_children = &node_1.children;
                let node_2_children = &node_2.children;
                // must explore these nodes
                for node_1_child in node_1_children {
                    if let Some(node_2_child) =
                        node_2_children.get(node_1_child)
                    {
                        let the_child = ret_nodes
                            .get(&ref_node)
                            .unwrap()
                            .children
                            .get(node_1_child)
                            .unwrap()
                            .clone();
                        stack.push((
                            drv1_nodes.get(node_1_child).unwrap(),
                            drv2_nodes.get(node_2_child).unwrap(),
                            the_child,
                        ));
                    }
                }
                for node_2_child in node_2_children {
                    if !node_1_children.contains(node_2_child) {
                        ret_nodes
                            .remove(&ref_node)
                            .unwrap()
                            .children
                            .insert(node_2_child.clone());
                    }
                }
            }
            return Some((ret_nodes, drv1_root.clone()));
        } else {
            // keep exploring
            let node = drv1_nodes.get(node_id).unwrap();
            nodes_to_search.extend(node.children.iter());
        }
    }

    None
}

pub fn construct_everything() {
    let sets = get_active_users_and_pids();
    let mut total_set = HashSet::new();
    for (_, set) in sets {
        let sett: HashSet<_> = set.into_iter().collect();
        let unioned = total_set.union(&sett).cloned();
        total_set = unioned.collect::<HashSet<_>>();
    }
    let mut map = construct_pid_map(total_set.clone());
    let total_tree = construct_tree(map.keys().cloned().collect(), &mut map);
    println!("total_tree {:?}", total_tree);
    let total_tree = total_tree.into_iter().next().unwrap().1;
    let real_roots = strip_tf_outta_tree(total_tree, &map);
    println!("real roots {:?}", real_roots);
    let drvs_roots = get_drvs(real_roots.clone());
    println!("drvs roots {:?}", drvs_roots);
    let dep_view: HashSet<&Drv> = drvs_roots.values().map(|v| &v.drv).collect();
    println!("DEP VIEW: {:?}", dep_view);
    let nodes = create_dep_tree(dep_view);
    println!("DEP TREE: {:?}", nodes);
    nodes.iter().for_each(dump_dep_tree);

    // dump_pids(&real_roots, &map);
}

#[cfg(test)]
mod tests {
    // TODO fix test so it can run on any computer. This requires pre-fetching
    // the drvs
    #[test]
    pub fn test_invoke_why_depends() {
        let parent = super::Drv {
            drv: "/nix/store/qyw7qc22j2ngf9wip8sxagaxb0387gnq-cargo-1.78.0"
                .to_string(),
            human_readable_drv: "cargo-1.78.0".to_string(),
        };
        let child = super::Drv {
            drv: "/nix/store/8bdd933v69w05k5v8hfcq74bi1f9545k-openssl-3.0.13"
                .to_string(),
            human_readable_drv: "openssl-3.0.13".to_string(),
        };
        // invoke the bash command `nix why-depends parent child` and get output
        // into string
        let result = super::invoke_why_depends(&parent, &child);
        println!("{result:?}");
        assert!(result.is_some());
        let result_ = result.unwrap();
        // TODO fix this test
        assert_eq!(result_.1, parent.drv);
        assert_eq!(result_.0.get(&parent.drv).unwrap().drv, parent);
        assert_eq!(result_.0.get(&parent.drv).unwrap().children.len(), 1);
        assert_eq!(
            *result_
                .0
                .get(&parent.drv)
                .unwrap()
                .children
                .iter()
                .next()
                .unwrap(),
            child.drv
        );
        // assert!(result_[1] == child);
    }

    #[test]
    pub fn test_create_dep_tree() {
        // fuck testing stick it into the cli and see what happens
        // let parent = super::Drv {
        //     drv: "/nix/store/0bfvsp2s2pkj8ihzkn4mdgpapgfab3gs-vimplugin-treesitter-grammar-hoon"
        //     human_readable_drv: vimplugin-treesitter-grammar-hoon
        //
        // };
        // let child = super::Drv {
        //     drv: "/nix/store/d4lbynl52arvqw3amz8mdz2nqvzdw4xf-hoon-grammar-0.
        // 0.0+rev=a24c5a3" };
        //
        //
    }
}
