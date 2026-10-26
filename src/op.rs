//! Which git operation is being animated, from the command line (wrap mode) or
//! from the HEAD reflog (watch mode).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Commit,
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Reset,
    Checkout,
    Stash,
    Fetch,
    Push,
    Pull,
    Branch,
    Tag,
    /// add / rm / mv / restore: only the index and working tree change.
    Stage,
    Other,
}

impl Op {
    /// Subcommands that may change something worth animating. Anything else is
    /// passed straight to git with no snapshot overhead.
    pub fn from_subcommand(sub: &str) -> Option<Op> {
        Some(match sub {
            "commit" | "am" => Op::Commit,
            "merge" => Op::Merge,
            "rebase" => Op::Rebase,
            "cherry-pick" => Op::CherryPick,
            "revert" => Op::Revert,
            "reset" => Op::Reset,
            "checkout" | "switch" => Op::Checkout,
            "stash" => Op::Stash,
            "fetch" => Op::Fetch,
            "push" => Op::Push,
            "pull" => Op::Pull,
            "branch" => Op::Branch,
            "tag" => Op::Tag,
            "add" | "rm" | "mv" | "restore" => Op::Stage,
            _ => return None,
        })
    }

    /// Guess the operation from a HEAD reflog message such as `merge feature: Fast-forward`.
    pub fn from_reflog(msg: &str) -> Op {
        let head = msg.split(':').next().unwrap_or("");
        let word = head.split([' ', '(']).next().unwrap_or("");
        match word {
            "commit" if head.contains("merge") => Op::Merge,
            "commit" => Op::Commit,
            "merge" => Op::Merge,
            "rebase" => Op::Rebase,
            "cherry-pick" => Op::CherryPick,
            "revert" => Op::Revert,
            "reset" => Op::Reset,
            "checkout" | "switch" => Op::Checkout,
            "pull" => Op::Pull,
            _ => Op::Other,
        }
    }

    /// Does this operation touch the working tree or index (so a status read is worth it)?
    pub fn needs_status(self) -> bool {
        !matches!(self, Op::Fetch | Op::Push | Op::Branch | Op::Tag)
    }

    pub fn verb(self) -> &'static str {
        match self {
            Op::Commit => "commit",
            Op::Merge => "merge",
            Op::Rebase => "rebase",
            Op::CherryPick => "cherry-pick",
            Op::Revert => "revert",
            Op::Reset => "reset",
            Op::Checkout => "switch",
            Op::Stash => "stash",
            Op::Fetch => "fetch",
            Op::Push => "push",
            Op::Pull => "pull",
            Op::Branch => "branch",
            Op::Tag => "tag",
            Op::Stage => "stage",
            Op::Other => "git",
        }
    }
}

#[derive(Clone, Debug)]
pub struct OpInfo {
    pub op: Op,
    /// The git arguments, subcommand first.
    pub args: Vec<String>,
    pub exit_code: i32,
    /// Nothing ran: this is a `--dry-run` prediction.
    pub preview: bool,
    /// Replaces the caption: (text, is a warning).
    pub note: Option<(String, bool)>,
}

impl OpInfo {
    pub fn new(op: Op, args: Vec<String>) -> OpInfo {
        OpInfo { op, args, exit_code: 0, preview: false, note: None }
    }

    /// Positional arguments after the subcommand (branch names, remotes, commits).
    pub fn positionals(&self) -> Vec<&str> {
        let mut out = Vec::new();
        let mut after_dashdash = false;
        for a in self.args.iter().skip(1) {
            if a == "--" {
                after_dashdash = true;
                continue;
            }
            if after_dashdash || !a.starts_with('-') {
                out.push(a.as_str());
            }
        }
        out
    }

    pub fn has_flag(&self, flag: &str) -> bool {
        self.args.iter().skip(1).any(|a| a == flag)
    }

    pub fn stash_action(&self) -> &str {
        self.args.get(1).map(String::as_str).filter(|a| !a.starts_with('-')).unwrap_or("push")
    }

    /// `--soft`, `--mixed` or `--hard`; None when we did not see the command line.
    pub fn reset_mode(&self) -> Option<&'static str> {
        if self.args.is_empty() {
            None
        } else if self.has_flag("--hard") {
            Some("hard")
        } else if self.has_flag("--soft") {
            Some("soft")
        } else {
            Some("mixed")
        }
    }

    /// Rebuild a plausible command line from a HEAD reflog message (watch mode),
    /// e.g. `merge topic: Fast-forward` -> `merge topic`.
    pub fn from_reflog(msg: &str) -> OpInfo {
        let op = Op::from_reflog(msg);
        let (head, rest) = msg.split_once(": ").unwrap_or((msg, ""));
        let mut args = Vec::new();
        match op {
            Op::Merge if head.starts_with("merge ") => {
                args = vec!["merge".to_string(), head["merge ".len()..].to_string()];
            }
            Op::Checkout => {
                if let Some(to) = rest.rsplit(" to ").next() {
                    args = vec!["checkout".to_string(), to.to_string()];
                }
            }
            _ => {}
        }
        OpInfo::new(op, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_reflog() {
        assert_eq!(Op::from_reflog("merge feature: Fast-forward"), Op::Merge);
        assert_eq!(Op::from_reflog("commit (amend): fix"), Op::Commit);
        assert_eq!(Op::from_reflog("commit (merge): Merge branch"), Op::Merge);
        assert_eq!(Op::from_reflog("rebase (finish): returning to refs/heads/x"), Op::Rebase);
        assert_eq!(Op::from_reflog("checkout: moving from main to x"), Op::Checkout);
    }

    #[test]
    fn positionals_skip_flags() {
        let info = OpInfo::new(Op::Merge, vec!["merge".into(), "--no-ff".into(), "feature".into()]);
        assert_eq!(info.positionals(), vec!["feature"]);
    }
}
