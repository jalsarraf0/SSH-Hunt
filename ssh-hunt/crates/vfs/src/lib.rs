#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeKind {
    Dir,
    File,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VfsPerms {
    pub mode: u16,
}

impl Default for VfsPerms {
    fn default() -> Self {
        Self { mode: 0o755 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VfsMeta {
    pub owner: String,
    pub group: String,
    pub perms: VfsPerms,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl VfsMeta {
    fn now(owner: &str, is_dir: bool) -> Self {
        Self {
            owner: owner.to_owned(),
            group: owner.to_owned(),
            perms: VfsPerms {
                mode: if is_dir { 0o755 } else { 0o644 },
            },
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VfsNode {
    pub path: String,
    pub kind: NodeKind,
    pub meta: VfsMeta,
    pub content: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VfsError {
    #[error("path not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("not a directory: {0}")]
    NotDirectory(String),
    #[error("not a file: {0}")]
    NotFile(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("cannot remove root")]
    RemoveRoot,
    #[error("invalid path")]
    InvalidPath,
}

#[derive(Debug, Clone)]
pub struct Vfs {
    nodes: BTreeMap<String, VfsNode>,
}

impl Default for Vfs {
    fn default() -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            "/".to_owned(),
            VfsNode {
                path: "/".to_owned(),
                kind: NodeKind::Dir,
                meta: VfsMeta::now("root", true),
                content: None,
            },
        );
        Self { nodes }
    }
}

impl Vfs {
    pub fn ensure_dir(&mut self, path: &str, owner: &str) -> Result<(), VfsError> {
        let normalized = normalize_path("/", path)?;
        if self.nodes.contains_key(&normalized) {
            return Ok(());
        }
        let parent = parent_path(&normalized);
        let parent_node = self
            .nodes
            .get(parent)
            .ok_or_else(|| VfsError::NotFound(parent.to_owned()))?;
        if parent_node.kind != NodeKind::Dir {
            return Err(VfsError::NotDirectory(parent.to_owned()));
        }
        self.nodes.insert(
            normalized.clone(),
            VfsNode {
                path: normalized,
                kind: NodeKind::Dir,
                meta: VfsMeta::now(owner, true),
                content: None,
            },
        );
        Ok(())
    }

    pub fn mkdir_p(&mut self, cwd: &str, path: &str, owner: &str) -> Result<String, VfsError> {
        let normalized = normalize_path(cwd, path)?;
        if normalized == "/" {
            return Ok(normalized);
        }

        let mut cur = String::from("/");
        for segment in normalized.split('/').filter(|s| !s.is_empty()) {
            if cur != "/" {
                cur.push('/');
            }
            cur.push_str(segment);
            self.ensure_dir(&cur, owner)?;
        }
        Ok(normalized)
    }

    pub fn touch(&mut self, cwd: &str, path: &str, owner: &str) -> Result<String, VfsError> {
        let normalized = normalize_path(cwd, path)?;
        if self.nodes.contains_key(&normalized) {
            if let Some(node) = self.nodes.get_mut(&normalized) {
                node.meta.updated_at = Utc::now();
            }
            return Ok(normalized);
        }
        let parent = parent_path(&normalized);
        let parent_node = self
            .nodes
            .get(parent)
            .ok_or_else(|| VfsError::NotFound(parent.to_owned()))?;
        if parent_node.kind != NodeKind::Dir {
            return Err(VfsError::NotDirectory(parent.to_owned()));
        }
        self.nodes.insert(
            normalized.clone(),
            VfsNode {
                path: normalized.clone(),
                kind: NodeKind::File,
                meta: VfsMeta::now(owner, false),
                content: Some(String::new()),
            },
        );
        Ok(normalized)
    }

    pub fn write_file(
        &mut self,
        cwd: &str,
        path: &str,
        data: &str,
        append: bool,
        owner: &str,
    ) -> Result<String, VfsError> {
        let normalized = self.touch(cwd, path, owner)?;
        let node = self
            .nodes
            .get_mut(&normalized)
            .ok_or_else(|| VfsError::NotFound(normalized.clone()))?;
        if node.kind != NodeKind::File {
            return Err(VfsError::NotFile(normalized));
        }
        let content = node.content.get_or_insert_with(String::new);
        if append {
            content.push_str(data);
        } else {
            *content = data.to_owned();
        }
        node.meta.updated_at = Utc::now();
        Ok(node.path.clone())
    }

    pub fn read_file(&self, cwd: &str, path: &str) -> Result<String, VfsError> {
        let normalized = normalize_path(cwd, path)?;
        let node = self
            .nodes
            .get(&normalized)
            .ok_or_else(|| VfsError::NotFound(normalized.clone()))?;
        if node.kind != NodeKind::File {
            return Err(VfsError::NotFile(normalized));
        }
        Ok(node.content.clone().unwrap_or_default())
    }

    pub fn remove(&mut self, cwd: &str, path: &str) -> Result<(), VfsError> {
        let normalized = normalize_path(cwd, path)?;
        if normalized == "/" {
            return Err(VfsError::RemoveRoot);
        }
        if !self.nodes.contains_key(&normalized) {
            return Err(VfsError::NotFound(normalized));
        }
        let prefix = format!("{normalized}/");
        self.nodes
            .retain(|k, _| k != &normalized && !k.starts_with(&prefix));
        Ok(())
    }

    pub fn copy(&mut self, cwd: &str, from: &str, to: &str) -> Result<(), VfsError> {
        let src = normalize_path(cwd, from)?;
        let dst = normalize_path(cwd, to)?;
        let src_node = self
            .nodes
            .get(&src)
            .cloned()
            .ok_or_else(|| VfsError::NotFound(src.clone()))?;
        if src_node.kind != NodeKind::File {
            return Err(VfsError::NotFile(src));
        }
        self.write_file(
            "/",
            &dst,
            &src_node.content.unwrap_or_default(),
            false,
            "system",
        )?;
        Ok(())
    }

    pub fn mv(&mut self, cwd: &str, from: &str, to: &str) -> Result<(), VfsError> {
        self.copy(cwd, from, to)?;
        self.remove(cwd, from)
    }

    /// Recursively copy the tree rooted at `from` to `to`.
    /// If `from` is a plain file this behaves identically to `copy()`.
    pub fn copy_tree(&mut self, cwd: &str, from: &str, to: &str) -> Result<(), VfsError> {
        let src = normalize_path(cwd, from)?;
        let dst = normalize_path(cwd, to)?;

        let src_node = self
            .nodes
            .get(&src)
            .cloned()
            .ok_or_else(|| VfsError::NotFound(src.clone()))?;

        if src_node.kind == NodeKind::File {
            return self.copy(cwd, from, to);
        }

        // Snapshot everything under `src` before we mutate the map.
        let src_prefix = format!("{src}/");
        let to_copy: Vec<(String, VfsNode)> = self
            .nodes
            .iter()
            .filter(|(k, _)| *k == &src || k.starts_with(&src_prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        for (path, mut node) in to_copy {
            let relative = if path == src { "" } else { &path[src.len()..] };
            let new_path = format!("{dst}{relative}");
            node.path = new_path.clone();
            self.nodes.insert(new_path, node);
        }
        Ok(())
    }

    /// Return the `VfsNode` for `path` without consuming it.
    pub fn stat(&self, cwd: &str, path: &str) -> Result<VfsNode, VfsError> {
        let normalized = normalize_path(cwd, path)?;
        self.nodes
            .get(&normalized)
            .cloned()
            .ok_or(VfsError::NotFound(normalized))
    }

    /// Like `ls()` but returns full `VfsNode` records for each direct child.
    pub fn ls_nodes(&self, cwd: &str, path: Option<&str>) -> Result<Vec<VfsNode>, VfsError> {
        let target = normalize_path(cwd, path.unwrap_or("."))?;
        let node = self
            .nodes
            .get(&target)
            .ok_or_else(|| VfsError::NotFound(target.clone()))?;
        match node.kind {
            NodeKind::File => Ok(vec![node.clone()]),
            NodeKind::Dir => {
                let prefix = if target == "/" {
                    "/".to_owned()
                } else {
                    format!("{target}/")
                };
                let mut result = Vec::new();
                for (key, n) in &self.nodes {
                    if key == &target {
                        continue;
                    }
                    if !key.starts_with(&prefix) {
                        continue;
                    }
                    let remain = &key[prefix.len()..];
                    // Only direct children (no further slashes in the remainder).
                    if remain.is_empty() || remain.contains('/') {
                        continue;
                    }
                    result.push(n.clone());
                }
                Ok(result)
            }
        }
    }

    /// Update the permission bits on a node.
    pub fn chmod(&mut self, cwd: &str, path: &str, mode: u16) -> Result<(), VfsError> {
        let normalized = normalize_path(cwd, path)?;
        let node = self
            .nodes
            .get_mut(&normalized)
            .ok_or_else(|| VfsError::NotFound(normalized.clone()))?;
        node.meta.perms.mode = mode;
        node.meta.updated_at = Utc::now();
        Ok(())
    }

    pub fn ls(&self, cwd: &str, path: Option<&str>) -> Result<Vec<String>, VfsError> {
        let target = normalize_path(cwd, path.unwrap_or("."))?;
        let node = self
            .nodes
            .get(&target)
            .ok_or_else(|| VfsError::NotFound(target.clone()))?;
        match node.kind {
            NodeKind::File => Ok(vec![target]),
            NodeKind::Dir => {
                let mut entries = BTreeSet::new();
                let prefix = if target == "/" {
                    "/".to_owned()
                } else {
                    format!("{target}/")
                };
                for key in self.nodes.keys() {
                    if !key.starts_with(&prefix) || key == &target {
                        continue;
                    }
                    let remain = &key[prefix.len()..];
                    if remain.is_empty() {
                        continue;
                    }
                    let name = remain.split('/').next().unwrap_or(remain);
                    entries.insert(name.to_owned());
                }
                Ok(entries.into_iter().collect())
            }
        }
    }

    pub fn cd(&self, cwd: &str, path: &str) -> Result<String, VfsError> {
        let target = normalize_path(cwd, path)?;
        let node = self
            .nodes
            .get(&target)
            .ok_or_else(|| VfsError::NotFound(target.clone()))?;
        if node.kind != NodeKind::Dir {
            return Err(VfsError::NotDirectory(target));
        }
        Ok(target)
    }

    pub fn find(
        &self,
        cwd: &str,
        root: &str,
        pattern: Option<&str>,
    ) -> Result<Vec<String>, VfsError> {
        let root_path = normalize_path(cwd, root)?;
        if !self.nodes.contains_key(&root_path) {
            return Err(VfsError::NotFound(root_path));
        }
        let regex = pattern.map(glob_to_regex).transpose()?;

        let mut out = Vec::new();
        let prefix = if root_path == "/" {
            "/".to_owned()
        } else {
            format!("{root_path}/")
        };

        for key in self.nodes.keys() {
            if key == &root_path || key.starts_with(&prefix) {
                if let Some(re) = &regex {
                    if re.is_match(key.rsplit('/').next().unwrap_or(key)) {
                        out.push(key.clone());
                    }
                } else {
                    out.push(key.clone());
                }
            }
        }
        Ok(out)
    }

    pub fn glob(&self, cwd: &str, pattern: &str) -> Result<Vec<String>, VfsError> {
        let normalized = normalize_path(cwd, pattern)?;
        let re = glob_to_regex(&normalized)?;
        let mut out = Vec::new();
        for key in self.nodes.keys() {
            if re.is_match(key) {
                out.push(key.clone());
            }
        }
        Ok(out)
    }
}

fn parent_path(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(idx) => &path[..idx],
        None => "/",
    }
}

pub fn normalize_path(cwd: &str, path: &str) -> Result<String, VfsError> {
    if path.is_empty() {
        return Err(VfsError::InvalidPath);
    }

    let mut parts = Vec::new();
    let source = if path.starts_with('/') {
        path.to_owned()
    } else if cwd == "/" {
        format!("/{path}")
    } else {
        format!("{cwd}/{path}")
    };

    for part in source.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            value => parts.push(value),
        }
    }

    if parts.is_empty() {
        Ok("/".to_owned())
    } else {
        Ok(format!("/{}", parts.join("/")))
    }
}

fn glob_to_regex(pattern: &str) -> Result<Regex, VfsError> {
    let mut out = String::from("^");
    let mut chars = pattern.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '[' => {
                out.push('[');
                for c in chars.by_ref() {
                    out.push(c);
                    if c == ']' {
                        break;
                    }
                }
            }
            '.' | '(' | ')' | '+' | '|' | '^' | '$' | '{' | '}' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }

    out.push('$');
    Regex::new(&out).map_err(|_| VfsError::InvalidPath)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_paths() {
        assert_eq!(normalize_path("/", "a/b").unwrap(), "/a/b");
        assert_eq!(normalize_path("/a", "../b").unwrap(), "/b");
        assert_eq!(normalize_path("/a/b", "./c").unwrap(), "/a/b/c");
    }

    #[test]
    fn create_and_read_file() {
        let mut vfs = Vfs::default();
        vfs.mkdir_p("/", "home/player", "player").unwrap();
        vfs.write_file("/", "/home/player/log.txt", "hello", false, "player")
            .unwrap();
        assert_eq!(vfs.read_file("/", "/home/player/log.txt").unwrap(), "hello");
    }

    #[test]
    fn copy_and_move_file() {
        let mut vfs = Vfs::default();
        vfs.mkdir_p("/", "tmp", "player").unwrap();
        vfs.write_file("/", "/tmp/a", "x", false, "player").unwrap();
        vfs.copy("/", "/tmp/a", "/tmp/b").unwrap();
        assert_eq!(vfs.read_file("/", "/tmp/b").unwrap(), "x");
        vfs.mv("/", "/tmp/b", "/tmp/c").unwrap();
        assert!(matches!(
            vfs.read_file("/", "/tmp/b"),
            Err(VfsError::NotFound(_))
        ));
    }

    #[test]
    fn find_with_pattern() {
        let mut vfs = Vfs::default();
        vfs.mkdir_p("/", "logs", "sys").unwrap();
        vfs.write_file("/", "/logs/a.log", "a", false, "sys")
            .unwrap();
        vfs.write_file("/", "/logs/b.txt", "b", false, "sys")
            .unwrap();
        let out = vfs.find("/", "/logs", Some("*.log")).unwrap();
        assert_eq!(out, vec!["/logs/a.log".to_string()]);
    }

    #[test]
    fn copy_tree_recursively_copies_directory() {
        let mut vfs = Vfs::default();
        vfs.mkdir_p("/", "src/sub", "sys").unwrap();
        vfs.write_file("/", "/src/a.txt", "alpha", false, "sys")
            .unwrap();
        vfs.write_file("/", "/src/sub/b.txt", "beta", false, "sys")
            .unwrap();

        vfs.copy_tree("/", "/src", "/dst").unwrap();

        assert_eq!(vfs.read_file("/", "/dst/a.txt").unwrap(), "alpha");
        assert_eq!(vfs.read_file("/", "/dst/sub/b.txt").unwrap(), "beta");
        // Source must still exist.
        assert_eq!(vfs.read_file("/", "/src/a.txt").unwrap(), "alpha");
    }

    #[test]
    fn stat_returns_node_metadata() {
        let mut vfs = Vfs::default();
        vfs.write_file("/", "/note.txt", "hello", false, "sys")
            .unwrap();
        let node = vfs.stat("/", "/note.txt").unwrap();
        assert_eq!(node.kind, NodeKind::File);
        assert_eq!(node.content.unwrap(), "hello");
        assert_eq!(node.meta.owner, "sys");
    }

    #[test]
    fn chmod_updates_permission_bits() {
        let mut vfs = Vfs::default();
        vfs.write_file("/", "/exec.sh", "#!/bin/sh", false, "sys")
            .unwrap();
        vfs.chmod("/", "/exec.sh", 0o755).unwrap();
        let node = vfs.stat("/", "/exec.sh").unwrap();
        assert_eq!(node.meta.perms.mode, 0o755);
    }

    #[test]
    fn ls_nodes_returns_direct_children_only() {
        let mut vfs = Vfs::default();
        vfs.mkdir_p("/", "top/sub", "sys").unwrap();
        vfs.write_file("/", "/top/file.txt", "x", false, "sys")
            .unwrap();
        vfs.write_file("/", "/top/sub/nested.txt", "y", false, "sys")
            .unwrap();
        let nodes = vfs.ls_nodes("/", Some("/top")).unwrap();
        // Direct children of /top: "file.txt" and "sub" only.
        assert_eq!(nodes.len(), 2);
        let names: Vec<_> = nodes
            .iter()
            .map(|n| n.path.rsplit('/').next().unwrap_or(""))
            .collect();
        assert!(names.contains(&"file.txt"));
        assert!(names.contains(&"sub"));
    }
}
