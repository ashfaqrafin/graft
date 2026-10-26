//! Understand a git command line the way git does: global options first
//! (`-C dir`, `-c k=v`, `--no-pager`, …), then the subcommand, which may be one
//! of the user's aliases.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::git::runner;

#[derive(Debug, Clone)]
pub struct GitCmd {
    /// Everything as typed, handed to git unchanged.
    pub all: Vec<OsString>,
    /// Index of the subcommand in `all`.
    pub sub_index: Option<usize>,
    /// Working directory after applying every `-C`.
    pub dir: PathBuf,
    /// `-c key=value` pairs, passed along when graft asks git questions.
    pub config: Vec<String>,
    /// False when `--git-dir`, `--work-tree` or `--bare` point git somewhere graft
    /// would not find on its own; such commands run without animation.
    pub trackable: bool,
}

/// Global options that take a separate value.
const WITH_VALUE: &[&str] = &["-C", "-c", "--git-dir", "--work-tree", "--namespace", "--super-prefix", "--config-env", "--exec-path", "--list-cmds"];

pub fn parse(all: Vec<OsString>) -> GitCmd {
    let mut dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut config = Vec::new();
    let mut trackable = true;
    let mut sub_index = None;
    let mut i = 0;
    while i < all.len() {
        let arg = all[i].to_string_lossy();
        if !arg.starts_with('-') {
            sub_index = Some(i);
            break;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n.to_string(), Some(v.to_string())),
            _ => (arg.to_string(), None),
        };
        if matches!(name.as_str(), "--git-dir" | "--work-tree" | "--bare" | "--namespace") {
            trackable = false;
        }
        if WITH_VALUE.contains(&name.as_str()) && inline.is_none() {
            let value = all.get(i + 1).map(|v| v.to_string_lossy().into_owned()).unwrap_or_default();
            match name.as_str() {
                "-C" => dir = dir.join(&value),
                "-c" => config.push(value),
                _ => {}
            }
            i += 2;
            continue;
        }
        if name == "--version" || name == "--help" || name == "-h" {
            break; // `git --version` and friends: nothing to animate
        }
        i += 1;
    }
    GitCmd { all, sub_index, dir, config, trackable }
}

impl GitCmd {
    pub fn sub(&self) -> Option<String> {
        self.sub_index.map(|i| self.all[i].to_string_lossy().into_owned())
    }

    /// The subcommand and its arguments, with an alias expanded.
    pub fn command(&self) -> Vec<String> {
        let Some(i) = self.sub_index else { return Vec::new() };
        let rest: Vec<String> = self.all[i + 1..].iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let sub = self.all[i].to_string_lossy().into_owned();
        let mut out = match self.alias(&sub) {
            Some(expansion) => expansion,
            None => vec![sub],
        };
        out.extend(rest);
        out
    }

    /// Expand `git config alias.<name>`; shell aliases (`!…`) are not followed.
    fn alias(&self, name: &str) -> Option<Vec<String>> {
        if BUILTINS.contains(&name) {
            return None;
        }
        let mut args: Vec<String> = Vec::new();
        for c in &self.config {
            args.push("-c".into());
            args.push(c.clone());
        }
        args.extend(["config".into(), "--get".into(), format!("alias.{name}")]);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let (code, out) = runner::output(Some(&self.dir), &refs)?;
        let value = String::from_utf8_lossy(&out).trim().to_string();
        if code != 0 || value.is_empty() || value.starts_with('!') {
            return None;
        }
        Some(split_words(&value))
    }
}

pub fn is_builtin(name: &str) -> bool {
    BUILTINS.contains(&name)
}

/// Every command git ships (`git --list-cmds=main`); never looked up as aliases,
/// since git does not let aliases override them.
const BUILTINS: &[&str] = &[
    "add", "am", "annotate", "apply", "archimport", "archive", "backfill", "bisect", "blame", "branch",
    "bugreport", "bundle", "cat-file", "check-attr", "check-ignore", "check-mailmap", "check-ref-format",
    "checkout", "checkout--worker", "checkout-index", "cherry", "cherry-pick", "citool", "clean", "clone",
    "column", "commit", "commit-graph", "commit-tree", "config", "count-objects", "credential", "credential-cache",
    "credential-cache--daemon", "credential-libsecret", "credential-store", "cvsexportcommit", "cvsimport",
    "cvsserver", "daemon", "describe", "diagnose", "diff", "diff-files", "diff-index", "diff-pairs", "diff-tree",
    "difftool", "difftool--helper", "fast-export", "fast-import", "fetch", "fetch-pack", "filter-branch",
    "fmt-merge-msg", "for-each-ref", "for-each-repo", "format-patch", "format-rev", "fsck", "fsck-objects",
    "fsmonitor--daemon", "gc", "get-tar-commit-id", "grep", "gui", "gui--askpass", "gui--askyesno", "hash-object",
    "help", "history", "hook", "http-backend", "http-fetch", "http-push", "imap-send", "index-pack", "init",
    "init-db", "instaweb", "interpret-trailers", "last-modified", "log", "ls-files", "ls-remote", "ls-tree",
    "mailinfo", "mailsplit", "maintenance", "merge", "merge-base", "merge-file", "merge-index", "merge-octopus",
    "merge-one-file", "merge-ours", "merge-recursive", "merge-recursive-ours", "merge-recursive-theirs",
    "merge-resolve", "merge-subtree", "merge-tree", "mergetool", "mktag", "mktree", "multi-pack-index", "mv",
    "name-rev", "notes", "p4", "pack-objects", "pack-redundant", "pack-refs", "patch-id", "pickaxe", "prune",
    "prune-packed", "pull", "push", "quiltimport", "range-diff", "read-tree", "rebase", "receive-pack", "reflog",
    "refs", "remote", "remote-ext", "remote-fd", "remote-ftp", "remote-ftps", "remote-http", "remote-https",
    "repack", "replace", "replay", "repo", "request-pull", "rerere", "reset", "restore", "rev-list", "rev-parse",
    "revert", "rm", "send-email", "send-pack", "sh-i18n--envsubst", "shell", "shortlog", "show", "show-branch",
    "show-index", "show-ref", "sparse-checkout", "stage", "stash", "status", "stripspace", "submodule",
    "submodule--helper", "subtree", "svn", "switch", "symbolic-ref", "tag", "unpack-file", "unpack-objects",
    "update-index", "update-ref", "update-server-info", "upload-archive", "upload-archive--writer", "upload-pack",
    "url-parse", "var", "verify-commit", "verify-pack", "verify-tag", "version", "web--browse", "whatchanged",
    "worktree", "write-tree",
];

/// Split a command line like a POSIX shell would for plain words and quotes.
/// Nothing is expanded or executed: `$(…)` and globs stay literal.
pub fn split_words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for d in chars.by_ref() {
                    if d == '\'' {
                        break;
                    }
                    cur.push(d);
                }
            }
            '"' => {
                in_word = true;
                while let Some(d) = chars.next() {
                    match d {
                        '"' => break,
                        '\\' => {
                            if let Some(e) = chars.next() {
                                cur.push(e);
                            }
                        }
                        _ => cur.push(d),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(e) = chars.next() {
                    cur.push(e);
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn finds_subcommand_after_global_options() {
        let c = parse(os(&["-C", "/tmp", "-c", "user.name=x", "--no-pager", "merge", "feature"]));
        assert_eq!(c.sub().as_deref(), Some("merge"));
        assert_eq!(c.dir, PathBuf::from("/tmp"));
        assert_eq!(c.config, vec!["user.name=x".to_string()]);
        assert!(c.trackable);
        let c = parse(os(&["--git-dir=/x/.git", "commit"]));
        assert!(!c.trackable);
        assert_eq!(parse(os(&["--version"])).sub(), None);
    }

    #[test]
    fn splits_like_a_shell() {
        assert_eq!(split_words(r#"commit -m "fix the \"bug\"" -a"#), vec!["commit", "-m", "fix the \"bug\"", "-a"]);
        assert_eq!(split_words("merge  'my branch' x\\ y"), vec!["merge", "my branch", "x y"]);
        assert_eq!(split_words("log $(rm -rf /)"), vec!["log", "$(rm", "-rf", "/)"]);
    }
}
