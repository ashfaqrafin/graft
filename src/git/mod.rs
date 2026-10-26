//! Everything that talks to git: fast in-process reads through gix, and the
//! real `git` binary for anything that mutates the repository.

pub mod reader;
pub mod runner;
pub mod snapshot;
pub mod status;

pub use gix::ObjectId as Oid;
pub use reader::{CommitInfo, RefInfo, RefKind, Repo};
pub use snapshot::{HeadState, RepoState, Snapshot, StashEntry};
pub use status::{FileEntry, FileKind, WorktreeStatus};
