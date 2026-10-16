use std::collections::HashMap;

use super::{Kind, Tree, NONE};
use crate::engine::fsx;
use crate::engine::programs;

pub struct Owner {
    pub name: String,
    pub size: u64,
    pub dirs: Vec<u32>,
}

#[derive(Default)]
pub struct Owners {
    pub list: Vec<Owner>,
    root_of: HashMap<u32, usize>,
}

fn app_roots() -> Vec<String> {
    let mut v = Vec::new();
    for name in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData", "LOCALAPPDATA", "APPDATA"] {
        if let Ok(p) = std::env::var(name) {
            v.push(p.trim_end_matches('\\').to_lowercase());
        }
    }
    if let Ok(p) = std::env::var("LOCALAPPDATA") {
        v.push(format!("{}\\programs", p.trim_end_matches('\\').to_lowercase()));
    }
    v.sort();
    v.dedup();
    v
}

pub fn build(tree: &Tree) -> Owners {
    let exact: HashMap<String, String> = programs::install_dirs().into_iter().collect();
    let roots = app_roots();
    let mut owners = Owners::default();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut stack: Vec<(u32, String)> = vec![(0, tree.root.to_string_lossy().trim_end_matches('\\').to_lowercase())];
    while let Some((idx, path)) = stack.pop() {
        let under_root = roots.contains(&path);
        for c in tree.children(idx) {
            let n = &tree.nodes[c as usize];
            if n.kind != Kind::Dir {
                continue;
            }
            let child_name = tree.name(n);
            let child_path = format!("{}\\{}", path, child_name.to_lowercase());
            let label = if let Some(name) = exact.get(&child_path) {
                Some(name.clone())
            } else if under_root && !fsx::under_windir(&child_path) {
                Some(child_name.to_string())
            } else {
                None
            };
            match label {
                Some(label) => {
                    let key = label.to_lowercase();
                    let oi = *index.entry(key).or_insert_with(|| {
                        owners.list.push(Owner { name: label, size: 0, dirs: Vec::new() });
                        owners.list.len() - 1
                    });
                    owners.list[oi].size += n.size;
                    owners.list[oi].dirs.push(c);
                    owners.root_of.insert(c, oi);
                }
                None => stack.push((c, child_path)),
            }
        }
    }
    let mut order: Vec<usize> = (0..owners.list.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(owners.list[i].size));
    let mut pos = vec![0usize; order.len()];
    for (new, &old) in order.iter().enumerate() {
        pos[old] = new;
    }
    for v in owners.root_of.values_mut() {
        *v = pos[*v];
    }
    let mut taken: Vec<Option<Owner>> = std::mem::take(&mut owners.list).into_iter().map(Some).collect();
    owners.list = order.iter().filter_map(|&i| taken[i].take()).collect();
    owners
}

impl Owners {
    pub fn of(&self, tree: &Tree, idx: u32) -> Option<usize> {
        let mut i = idx;
        while i != NONE {
            if let Some(o) = self.root_of.get(&i) {
                return Some(*o);
            }
            i = tree.nodes[i as usize].parent;
        }
        None
    }

}
