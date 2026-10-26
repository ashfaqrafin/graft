//! A cheap, comparable picture of the repository: refs, HEAD, stash, worktree.
//! The commit graph itself is loaded separately (see `model::graph`) from the
//! tips of *both* snapshots, so the before and after states share one layout.

use super::{Oid, RefInfo, Repo, WorktreeStatus};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeadState {
    /// On a branch (full ref name). `oid` is None on an unborn branch.
    Attached { branch: String, oid: Option<Oid> },
    Detached(Oid),
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoState {
    Clean,
    Merging,
    Rebasing,
    CherryPicking,
    Reverting,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StashEntry {
    pub oid: Oid,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub refs: Vec<RefInfo>,
    pub head: HeadState,
    pub worktree: Option<WorktreeStatus>,
    pub stash: Vec<StashEntry>,
    pub state: RepoState,
    /// MERGE_HEAD entries during a conflicted merge.
    pub merge_heads: Vec<Oid>,
    /// The commit being applied when a rebase, cherry-pick or revert stopped.
    pub pick_head: Option<Oid>,
}

impl Snapshot {
    pub fn take(repo: &Repo, with_status: bool) -> Snapshot {
        let refs = repo.refs();
        let head = match repo.head() {
            (Some(branch), oid) => HeadState::Attached { branch, oid },
            (None, Some(oid)) => HeadState::Detached(oid),
            (None, None) => HeadState::Unknown,
        };
        let worktree = if with_status { repo.workdir.as_deref().and_then(WorktreeStatus::read) } else { None };
        let gd = &repo.git_dir;
        let state = if gd.join("rebase-merge").exists() || gd.join("rebase-apply").exists() {
            RepoState::Rebasing
        } else if gd.join("MERGE_HEAD").exists() {
            RepoState::Merging
        } else if gd.join("CHERRY_PICK_HEAD").exists() {
            RepoState::CherryPicking
        } else if gd.join("REVERT_HEAD").exists() {
            RepoState::Reverting
        } else {
            RepoState::Clean
        };
        let pick_head = ["CHERRY_PICK_HEAD", "REBASE_HEAD", "REVERT_HEAD"]
            .iter()
            .find_map(|f| repo.read_oid_file(f).into_iter().next());
        Snapshot {
            refs,
            head,
            worktree,
            stash: read_stash(repo),
            state,
            merge_heads: repo.read_oid_file("MERGE_HEAD"),
            pick_head,
        }
    }

    pub fn head_oid(&self) -> Option<Oid> {
        match &self.head {
            HeadState::Attached { oid, .. } => *oid,
            HeadState::Detached(oid) => Some(*oid),
            HeadState::Unknown => None,
        }
    }

    /// Full name of the checked-out branch.
    pub fn head_branch(&self) -> Option<&str> {
        match &self.head {
            HeadState::Attached { branch, .. } => Some(branch),
            _ => None,
        }
    }

    pub fn find_ref(&self, full_name: &str) -> Option<&RefInfo> {
        self.refs.iter().find(|r| r.name == full_name)
    }

    pub fn conflicted_files(&self) -> Vec<String> {
        self.worktree
            .as_ref()
            .map(|w| w.conflicted.iter().map(|f| f.path.clone()).collect())
            .unwrap_or_default()
    }
}

/// Stash entries newest first, read straight from the `refs/stash` reflog.
fn read_stash(repo: &Repo) -> Vec<StashEntry> {
    let Ok(log) = std::fs::read_to_string(repo.common_dir.join("logs/refs/stash")) else {
        return Vec::new();
    };
    let mut entries: Vec<StashEntry> = log
        .lines()
        .filter_map(|line| {
            let (meta, message) = line.split_once('\t')?;
            let new = meta.split(' ').nth(1)?;
            Some(StashEntry { oid: Oid::from_hex(new.as_bytes()).ok()?, message: message.to_string() })
        })
        .collect();
    entries.reverse();
    entries
}
