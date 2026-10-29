//! OpenRC-style init system for tAbLeOS.
//!
//! Reads /etc/init.d/* scripts, builds a dependency graph from `depend()` blocks,
//! runs services in runlevel order: sysinit -> boot -> default.

use crate::e1fss;
use crate::println;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;

const RUNLEVELS: [&str; 3] = ["sysinit", "boot", "default"];

struct Service {
    name: String,
    path: String,
    level: u8, // 0=sysinit 1=boot 2=default
    deps: Vec<String>,
}

pub fn run() -> Result<(), &'static str> {
    let entries = e1fss::list_dir("etc/init.d")?;
    let mut services = Vec::new();

    for e in entries {
        let nlen = e.name.iter().position(|&c| c == 0).unwrap_or(59);
        let name = core::str::from_utf8(&e.name[..nlen]).unwrap_or("?");
        if name.ends_with("~") || name.starts_with(".") {
            continue;
        }
        let path = format!("etc/init.d/{}", name);
        let data = e1fss::read_file(&path)?;
        let script = core::str::from_utf8(&data).map_err(|_| "non-utf8 script")?;
        let deps = parse_deps(script);
        let level = parse_runlevel(script);
        services.push(Service {
            name: name.to_string(),
            path,
            level,
            deps,
        });
    }

    // topological sort
    let sorted = topo_sort(&services)?;

    for svc in sorted {
        let level_name = RUNLEVELS[svc.level as usize];
        println!("  [{}] starting {}...", level_name, svc.name);
        run_script(&svc.path)?;
    }
    Ok(())
}

fn parse_deps(script: &str) -> Vec<String> {
    let mut deps = Vec::new();
    for line in script.lines() {
        let line = line.trim();
        if line.starts_with("depend") {
            // parse "depend need net foo bar" style
            let mut it = line.split_whitespace();
            it.next(); // "depend"
            while let Some(word) = it.next() {
                if word == "need" || word == "use" || word == "want" || word == "after" || word == "before" {
                    continue;
                }
                deps.push(word.to_string());
            }
            break;
        }
    }
    deps
}

fn parse_runlevel(script: &str) -> u8 {
    for line in script.lines() {
        let line = line.trim();
        if line.starts_with("runlevel") {
            let mut it = line.split_whitespace();
            it.next();
            if let Some(lv) = it.next() {
                return match lv {
                    "sysinit" => 0,
                    "boot" => 1,
                    "default" => 2,
                    _ => 2,
                };
            }
        }
    }
    2 // default runlevel
}

fn topo_sort(services: &[Service]) -> Result<Vec<&Service>, &'static str> {
    let mut indegree: BTreeMap<&str, usize> = BTreeMap::new();
    let mut edges: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for svc in services {
        indegree.insert(&svc.name, 0);
    }
    for svc in services {
        for dep in &svc.deps {
            if let Some(d) = indegree.get_mut(dep.as_str()) {
                *d += 1;
                edges.entry(dep.as_str()).or_insert_with(Vec::new).push(&svc.name);
            }
        }
    }
    let mut queue: Vec<&Service> = services
        .iter()
        .filter(|s| indegree.get(s.name.as_str()) == Some(&0))
        .collect();
    let mut out = Vec::new();
    while let Some(svc) = queue.pop() {
        out.push(svc);
        if let Some(nexts) = edges.get(svc.name.as_str()) {
            for &n in nexts {
                if let Some(d) = indegree.get_mut(n) {
                    *d -= 1;
                    if *d == 0 {
                        if let Some(s) = services.iter().find(|s| s.name == n) {
                            queue.push(s);
                        }
                    }
                }
            }
        }
    }
    if out.len() != services.len() {
        return Err("circular dependency detected");
    }
    Ok(out)
}

fn run_script(path: &str) -> Result<(), &'static str> {
    // v1: only support simple `exec <path>` lines
    let data = e1fss::read_file(path)?;
    let script = core::str::from_utf8(&data).map_err(|_| "non-utf8 script")?;
    for line in script.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("#") || line.starts_with("depend") || line.starts_with("runlevel") {
            continue;
        }
        if line.starts_with("exec ") {
            let cmd = &line[5..];
            let mut it = cmd.split_whitespace();
            if let Some(bin) = it.next() {
                let _ = it.collect::<Vec<_>>(); // args ignored for v1
                if let Ok(data) = e1fss::read_file(bin) {
                    let _ = crate::sched::spawn_user(bin.as_bytes(), &data, 0x400000);
                }
            }
        }
    }
    Ok(())
}
