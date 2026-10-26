//! Read-only access to the repository through gix. No process spawns here.

use std::path::{Path, PathBuf};

use anyhow::Result;
use gix::refs::TargetRef;
use smallvec::SmallVec;

use super::Oid;

pub struct Repo {
    pub repo: gix::Repository,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub workdir: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct CommitInfo {
    pub parents: SmallVec<[Oid; 2]>,
    pub summary: String,
    pub author: String,
    pub time: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RefKind {
    Local,
    Remote,
    Tag,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefInfo {
    /// Full name, e.g. `refs/heads/main`.
    pub name: String,
    /// Display name, e.g. `main` or `origin/main`.
    pub short: String,
    pub kind: RefKind,
    pub target: Oid,
}

impl Repo {
    pub fn discover(dir: impl AsRef<Path>) -> Result<Repo> {
        let mut repo = gix::discover(dir.as_ref())?;
        repo.object_cache_size_if_unset(4 * 1024 * 1024);
        let abs = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        Ok(Repo {
            git_dir: abs(repo.git_dir()),
            common_dir: abs(repo.common_dir()),
            workdir: repo.workdir().map(abs),
            repo,
        })
    }

    pub fn commit(&self, id: Oid) -> Option<CommitInfo> {
        let commit = self.repo.find_object(id).ok()?.try_into_commit().ok()?;
        let d = commit.decode().ok()?;
        Some(CommitInfo {
            parents: d.parents().collect(),
            summary: d.message().summary().to_string(),
            author: d.author().map(|a| a.name.to_string()).unwrap_or_default(),
            time: d.time().map(|t| t.seconds).unwrap_or(0),
        })
    }

    /// Branches, remote-tracking branches and tags, peeled to the commit they name.
    pub fn refs(&self) -> Vec<RefInfo> {
        let mut out = Vec::new();
        let Ok(platform) = self.repo.references() else { return out };
        let Ok(iter) = platform.all() else { return out };
        for r in iter.flatten() {
            let mut r = r;
            if matches!(r.target(), TargetRef::Symbolic(_)) {
                continue; // refs/remotes/origin/HEAD and friends
            }
            let name = r.name().as_bstr().to_string();
            let (kind, short) = if let Some(s) = name.strip_prefix("refs/heads/") {
                (RefKind::Local, s.to_string())
            } else if let Some(s) = name.strip_prefix("refs/remotes/") {
                (RefKind::Remote, s.to_string())
            } else if let Some(s) = name.strip_prefix("refs/tags/") {
                (RefKind::Tag, s.to_string())
            } else {
                continue;
            };
            if let Ok(id) = r.peel_to_id() {
                out.push(RefInfo { name, short, kind, target: id.detach() });
            }
        }
        out
    }

    /// `(branch full name, commit)`; branch is None when detached, commit is None when unborn.
    pub fn head(&self) -> (Option<String>, Option<Oid>) {
        match self.repo.head() {
            Ok(head) => {
                let branch = head.referent_name().map(|n| n.as_bstr().to_string());
                let id = head.id().map(|id| id.detach());
                (branch, id)
            }
            Err(_) => (None, None),
        }
    }

    /// The remote-tracking ref a local branch pulls from, e.g. `refs/remotes/origin/main`.
    pub fn upstream(&self, branch_short: &str) -> Option<String> {
        let config = self.repo.config_snapshot();
        let remote = config.string(format!("branch.{branch_short}.remote").as_str())?;
        let merge = config.string(format!("branch.{branch_short}.merge").as_str())?;
        let merge = merge.to_string();
        let merge = merge.strip_prefix("refs/heads/").unwrap_or(&merge);
        if remote == "." {
            return Some(format!("refs/heads/{merge}"));
        }
        Some(format!("refs/remotes/{remote}/{merge}"))
    }

    /// Read a single-oid file like MERGE_HEAD, ORIG_HEAD or REBASE_HEAD.
    pub fn read_oid_file(&self, name: &str) -> Vec<Oid> {
        std::fs::read_to_string(self.git_dir.join(name))
            .map(|s| {
                s.lines()
                    .filter_map(|l| Oid::from_hex(l.split_whitespace().next()?.as_bytes()).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The message of the latest HEAD reflog entry, e.g. `merge feature: Fast-forward`.
    pub fn last_reflog_message(&self) -> Option<String> {
        let log = std::fs::read_to_string(self.git_dir.join("logs/HEAD")).ok()?;
        let line = log.lines().last()?;
        Some(line.split_once('\t')?.1.to_string())
    }

    pub fn display_name(&self) -> String {
        self.workdir
            .as_deref()
            .unwrap_or(&self.common_dir)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".into())
    }
}

/// Resolve a user-supplied name (as given on a git command line) against a ref list.
pub fn resolve_name<'a>(refs: &'a [RefInfo], name: &str) -> Option<&'a RefInfo> {
    let name = name.trim_start_matches('+');
    let candidates = [
        name.to_string(),
        format!("refs/heads/{name}"),
        format!("refs/remotes/{name}"),
        format!("refs/tags/{name}"),
        format!("refs/remotes/{name}/HEAD"),
    ];
    candidates.iter().find_map(|c| refs.iter().find(|r| &r.name == c))
}
