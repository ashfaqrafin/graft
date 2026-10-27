//! Working tree state from `git status --porcelain=v2 -z`, which benefits from
//! fsmonitor and the untracked cache.

use std::path::Path;

use super::runner;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileEntry {
    pub path: String,
    pub kind: FileKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorktreeStatus {
    pub staged: Vec<FileEntry>,
    pub unstaged: Vec<FileEntry>,
    pub untracked: Vec<FileEntry>,
    pub conflicted: Vec<FileEntry>,
}

impl WorktreeStatus {
    pub fn read(workdir: &Path) -> Option<WorktreeStatus> {
        // --no-optional-locks keeps status from rewriting the index, which would
        // otherwise wake up `git graft watch` in an endless loop.
        let (code, out) = runner::output(
            Some(workdir),
            &["--no-optional-locks", "status", "--porcelain=v2", "-z", "--untracked-files=normal"],
        )?;
        (code == 0).then(|| parse(&out))
    }

    pub fn dirty_count(&self) -> usize {
        self.staged.len() + self.unstaged.len() + self.untracked.len() + self.conflicted.len()
    }
}

pub fn parse(out: &[u8]) -> WorktreeStatus {
    let mut st = WorktreeStatus::default();
    let text = String::from_utf8_lossy(out);
    let mut records = text.split('\0');
    while let Some(rec) = records.next() {
        let Some(kind) = rec.chars().next() else { continue };
        match kind {
            '1' | '2' => {
                let fields = if kind == '1' { 9 } else { 10 };
                let parts: Vec<&str> = rec.splitn(fields, ' ').collect();
                if parts.len() < fields {
                    continue;
                }
                if kind == '2' {
                    records.next(); // original path of a rename
                }
                let xy = parts[1].as_bytes();
                let path = parts[fields - 1].to_string();
                if let Some(k) = code_kind(xy[0]) {
                    st.staged.push(FileEntry { path: path.clone(), kind: k });
                }
                if let Some(k) = code_kind(xy[1]) {
                    st.unstaged.push(FileEntry { path, kind: k });
                }
            }
            'u' => {
                if let Some(path) = rec.splitn(11, ' ').nth(10) {
                    st.conflicted.push(FileEntry { path: path.to_string(), kind: FileKind::Conflicted });
                }
            }
            '?' => st.untracked.push(FileEntry { path: rec[2..].to_string(), kind: FileKind::Untracked }),
            _ => {}
        }
    }
    st
}

fn code_kind(c: u8) -> Option<FileKind> {
    match c {
        b'A' => Some(FileKind::Added),
        b'M' | b'T' => Some(FileKind::Modified),
        b'D' => Some(FileKind::Deleted),
        b'R' | b'C' => Some(FileKind::Renamed),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2() {
        let raw = b"1 M. N... 100644 100644 100644 aaa bbb src/main.rs\0\
1 .M N... 100644 100644 100644 aaa bbb with space.txt\0\
2 R. N... 100644 100644 100644 aaa bbb R100 new.rs\0old.rs\0\
u UU N... 100644 100644 100644 100644 a b c conflict.rs\0\
? notes.md\0";
        let st = parse(raw);
        assert_eq!(st.staged.len(), 2);
        assert_eq!(st.staged[1].path, "new.rs");
        assert_eq!(st.unstaged[0].path, "with space.txt");
        assert_eq!(st.conflicted[0].path, "conflict.rs");
        assert_eq!(st.untracked[0].path, "notes.md");
    }
}
