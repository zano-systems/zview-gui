//! A captured selection shapes memory requests. A dump selection only filters
//! complete rows; it must never change those requests (or their denominators).
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const GROUPS: [&str; 7] = [
    "threads",
    "heaps",
    "semaphores",
    "mutexes",
    "msgqs",
    "mem_slabs",
    "workqs",
];
type Pattern = (Option<String>, String);

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Selection {
    groups: Option<Vec<String>>,
    skipped: Vec<String>,
    include: Vec<Pattern>,
    exclude: Vec<Pattern>,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            groups: None,
            skipped: vec![],
            include: vec![(None, "*".into())],
            exclude: vec![],
        }
    }
}

impl Selection {
    pub fn parse(line: &str) -> Result<Self> {
        let mut s = Self {
            include: vec![],
            ..Self::default()
        };
        for p in line.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (scope, pat) = p
                .split_once(':')
                .map_or((None, p), |(g, p)| (Some(g.to_owned()), p));
            let (list, pat) = match pat.strip_prefix('!') {
                Some(p) => (&mut s.exclude, p),
                None => (&mut s.include, pat),
            };
            list.push((scope, pat.to_owned()));
        }
        s.validate()?;
        Ok(s)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        for group in self.groups.iter().flatten().chain(&self.skipped).chain(
            self.include
                .iter()
                .chain(&self.exclude)
                .filter_map(|(g, _)| g.as_ref()),
        ) {
            ensure!(GROUPS.contains(&group.as_str()), "unknown group {group}");
        }
        Ok(())
    }
    fn patterns<'a>(
        list: &'a [Pattern],
        group: &'a str,
        own: bool,
    ) -> impl Iterator<Item = &'a str> {
        list.iter()
            .filter(move |(scope, _)| scope.as_deref() == Some(group) || (!own && scope.is_none()))
            .map(|(_, p)| p.as_str())
    }
    fn includes<'a>(&'a self, g: &'a str) -> impl Iterator<Item = &'a str> {
        Self::patterns(
            &self.include,
            g,
            self.include.iter().any(|(s, _)| s.as_deref() == Some(g)),
        )
    }
    pub(crate) fn group(&self, g: &str) -> bool {
        self.groups
            .as_ref()
            .is_none_or(|gs| gs.iter().any(|x| x == g))
            && !self.skipped.iter().any(|x| x == g)
            && self.includes(g).next().is_some()
            && !Self::patterns(&self.exclude, g, false).any(|p| p == "*")
    }
    pub(crate) fn row(&self, g: &str, name: &str) -> bool {
        self.group(g)
            && self.includes(g).any(|p| matches(p, name))
            && !Self::patterns(&self.exclude, g, false).any(|p| matches(p, name))
    }
    pub(crate) fn notices(&self, inventory: &[(&str, Vec<&str>)]) -> Vec<String> {
        let mut out = Vec::new();
        if !inventory.iter().any(|(g, _)| self.group(g)) {
            out.push("Warning: the patterns read no group".into());
        }
        for (kind, list) in [("include", &self.include), ("exclude", &self.exclude)] {
            for (scope, pat) in list {
                if !inventory.iter().any(|(g, ns)| {
                    scope.as_deref().is_none_or(|s| s == *g) && ns.iter().any(|n| matches(pat, n))
                }) {
                    let prefix = scope.as_ref().map(|s| format!("{s}:")).unwrap_or_default();
                    out.push(format!(
                        "Warning: {kind} pattern {prefix}{pat} matches no row"
                    ));
                }
            }
        }
        out
    }
}

// fnmatchcase, not a filesystem glob: slash and dot are ordinary characters.
fn matches(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let p: Vec<_> = pattern.chars().collect();
    let n: Vec<_> = name.chars().collect();
    let mut matched = vec![false; n.len() + 1];
    matched[0] = true;
    let mut i = 0;
    while i < p.len() {
        let c = p[i];
        i += 1;
        if c == '*' {
            for j in 1..matched.len() {
                matched[j] |= matched[j - 1];
            }
            continue;
        }
        let mut class = None;
        if c == '[' {
            let neg = p.get(i) == Some(&'!');
            let start = i + usize::from(neg);
            let search = start + usize::from(p.get(start) == Some(&']'));
            if let Some(end) = (search..p.len()).find(|&k| p[k] == ']') {
                class = Some((&p[start..end], neg));
                i = end + 1;
            }
        }
        for j in (0..n.len()).rev() {
            let ch = n[j];
            let accepts = if let Some((chars, neg)) = class {
                let mut k = 0;
                let mut hit = false;
                while k < chars.len() {
                    if k + 2 < chars.len() && chars[k + 1] == '-' {
                        hit |= (chars[k]..=chars[k + 2]).contains(&ch);
                        k += 3;
                    } else {
                        hit |= chars[k] == ch;
                        k += 1;
                    }
                }
                hit != neg
            } else {
                c == '?' || c == ch
            };
            matched[j + 1] = matched[j] && accepts;
        }
        matched[0] = false;
    }
    matched[n.len()]
}
