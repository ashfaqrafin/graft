//! Shell integration: a `git` function that sends git commands through graft
//! (so plain `git merge` animates), and a key binding that previews the command
//! you are typing before you run it.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

const BEGIN: &str = "# >>> graft: animated git >>>";
const END: &str = "# <<< graft <<<";

pub fn init(shell: Shell) -> String {
    match shell {
        Shell::Bash => r#"# graft shell integration (bash): eval "$(git-graft init bash)"
git() {
  if [ -t 1 ] && command -v git-graft >/dev/null 2>&1; then
    command git-graft --as-git "$@"
  else
    command git "$@"
  fi
}
# Alt+V: preview what the git command you are typing would do.
__graft_preview() {
  case "$READLINE_LINE" in
    git|git\ *) ;;
    *) return ;;
  esac
  printf '\n'
  command git-graft preview --line "$READLINE_LINE" </dev/tty
}
[[ $- == *i* ]] && bind -x '"\ev": __graft_preview' 2>/dev/null
"#
        .to_string(),
        Shell::Zsh => r#"# graft shell integration (zsh): eval "$(git-graft init zsh)"
git() {
  if [[ -t 1 ]] && (( $+commands[git-graft] )); then
    command git-graft --as-git "$@"
  else
    command git "$@"
  fi
}
# Alt+V: preview what the git command you are typing would do.
__graft_preview() {
  [[ $BUFFER == git || $BUFFER == git\ * ]] || return 0
  zle -I
  print
  command git-graft preview --line "$BUFFER" </dev/tty
}
if [[ -o interactive ]]; then
  zle -N __graft_preview
  bindkey '\ev' __graft_preview
fi
"#
        .to_string(),
        Shell::Fish => r#"# graft shell integration (fish): git-graft init fish | source
function git --wraps git
    if isatty stdout; and command -q git-graft
        command git-graft --as-git $argv
    else
        command git $argv
    end
end
# Alt+G: preview what the git command you are typing would do.
function __graft_preview
    set -l line (commandline)
    string match -qr '^git(\s|$)' -- $line; or return
    echo
    command git-graft preview --line "$line" </dev/tty
    commandline -f repaint
end
if status is-interactive
    bind \eg __graft_preview
end
"#
        .to_string(),
    }
}

pub fn detect() -> Option<Shell> {
    let shell = std::env::var("SHELL").ok()?;
    match std::path::Path::new(&shell).file_name()?.to_str()? {
        "bash" => Some(Shell::Bash),
        "zsh" => Some(Shell::Zsh),
        "fish" => Some(Shell::Fish),
        _ => None,
    }
}

fn rc_file(shell: Shell) -> Result<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
    Ok(match shell {
        Shell::Bash => home.join(".bashrc"),
        Shell::Zsh => std::env::var_os("ZDOTDIR").map(PathBuf::from).unwrap_or(home).join(".zshrc"),
        Shell::Fish => std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("fish/conf.d/graft.fish"),
    })
}

/// The block `git graft setup` adds. `path_dir` is added to PATH when graft would
/// not otherwise be found.
fn snippet(shell: Shell, path_dir: Option<&str>) -> String {
    let mut s = format!("{BEGIN}\n");
    if let Some(dir) = path_dir {
        s += &match shell {
            Shell::Fish => format!("fish_add_path -g {dir}\n"),
            _ => format!("export PATH=\"{dir}:$PATH\"\n"),
        };
    }
    s += match shell {
        Shell::Bash => "command -v git-graft >/dev/null 2>&1 && eval \"$(git-graft init bash)\"\n",
        Shell::Zsh => "(( $+commands[git-graft] )) && eval \"$(git-graft init zsh)\"\n",
        Shell::Fish => "command -q git-graft; and git-graft init fish | source\n",
    };
    s + END + "\n"
}

/// graft's own directory, if it is not already on PATH (written with `$HOME`).
fn missing_path_dir() -> Option<String> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let dir = exe.parent()?;
    let on_path = std::env::split_paths(&std::env::var_os("PATH")?)
        .any(|p| p.canonicalize().is_ok_and(|p| p == dir));
    if on_path || dir.ends_with("target/debug") || dir.ends_with("target/release") {
        return None;
    }
    let home = std::env::var("HOME").ok();
    let shown = match home.as_deref().and_then(|h| dir.strip_prefix(h).ok()) {
        Some(rest) => format!("$HOME/{}", rest.display()),
        None => dir.display().to_string(),
    };
    Some(shown)
}

/// `git graft setup`: add (or with `undo`, remove) the integration in the shell's startup file.
pub fn setup(shell: Option<Shell>, yes: bool, undo: bool) -> Result<i32> {
    let shell = shell.or_else(detect).context("could not tell which shell you use; pass --shell bash|zsh|fish")?;
    let path = rc_file(shell)?;
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let installed = current.contains(BEGIN);

    if undo {
        if !installed {
            println!("graft is not set up in {}", path.display());
            return Ok(0);
        }
        let mut out = String::new();
        let mut inside = false;
        for line in current.lines() {
            if line == BEGIN {
                inside = true;
                continue;
            }
            if inside {
                inside = line != END;
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        std::fs::write(&path, out.trim_end_matches('\n').to_string() + "\n")?;
        println!("removed graft from {} (open a new shell to finish)", path.display());
        return Ok(0);
    }

    if installed {
        println!("graft is already set up in {}", path.display());
        return Ok(0);
    }
    if !yes {
        if !std::io::stdin().is_terminal() {
            bail!("run `git graft setup --yes` to change {} without asking", path.display());
        }
        print!("Make plain `git` commands animate by adding a few lines to {}? [Y/n] ", path.display());
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if matches!(answer.trim(), "n" | "N" | "no") {
            println!("left {} alone; you can run `git graft setup` any time", path.display());
            return Ok(0);
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = current;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(&snippet(shell, missing_path_dir().as_deref()));
    std::fs::write(&path, text)?;
    let key = if shell == Shell::Fish { "Alt+G" } else { "Alt+V" };
    println!("added graft to {}", path.display());
    println!("open a new terminal (or `source {}`), then:", path.display());
    println!("  git merge <branch>    animates as usual git");
    println!("  {key:<20}  while typing a git command: preview what it would do");
    println!("  git graft live        type git commands with a live preview");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_are_marked_for_undo() {
        for s in [Shell::Bash, Shell::Zsh, Shell::Fish] {
            for dir in [None, Some("$HOME/.local/bin")] {
                let block = snippet(s, dir);
                assert!(block.starts_with(BEGIN) && block.trim_end().ends_with(END));
                assert_eq!(block.lines().count(), 3 + dir.is_some() as usize);
            }
            assert!(init(s).contains("--as-git"));
        }
    }
}
