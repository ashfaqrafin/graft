//! `git graft demo`: build a throwaway repository (plus a bare "remote" and a second
//! clone to play a teammate) and play every scene.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Result, bail};

use crate::app::{self, Settings};
use crate::git::runner;
use crate::render::term;

struct Step {
    name: &'static str,
    /// Shown as the command being run.
    cmd: &'static [&'static str],
    /// Quiet preparation before the animated command.
    setup: fn(&Demo),
}

struct Demo {
    root: PathBuf,
    work: PathBuf,
    mate: PathBuf,
}

impl Demo {
    fn git(&self, dir: &Path, args: &[&str]) {
        runner::quiet(dir, args);
    }
    fn write(&self, dir: &Path, file: &str, text: &str) {
        let path = dir.join(file);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, text);
    }
    /// Commit a file in `dir`.
    fn commit(&self, dir: &Path, file: &str, text: &str, msg: &str) {
        self.write(dir, file, text);
        self.git(dir, &["add", file]);
        self.git(dir, &["commit", "-q", "-m", msg]);
    }
    fn configure(&self, dir: &Path) {
        for (k, v) in [
            ("user.name", "Graft Demo"),
            ("user.email", "demo@graft.invalid"),
            ("commit.gpgsign", "false"),
            ("tag.gpgsign", "false"),
            ("core.hooksPath", "/dev/null"),
            ("pull.rebase", "false"),
            ("advice.detachedHead", "false"),
        ] {
            self.git(dir, &["config", k, v]);
        }
    }
}

const STEPS: &[Step] = &[
    Step {
        name: "commit",
        cmd: &["commit", "-m", "Add the parser"],
        setup: |d| {
            d.write(&d.work, "src/parser.rs", "pub fn parse() {}\n");
            d.write(&d.work, "src/lexer.rs", "pub fn lex() {}\n");
            d.git(&d.work, &["add", "src"]);
        },
    },
    Step { name: "branch", cmd: &["branch", "feature"], setup: |_| {} },
    Step { name: "switch", cmd: &["switch", "feature"], setup: |_| {} },
    Step {
        name: "commit on a branch",
        cmd: &["commit", "-am", "Teach the parser about loops"],
        setup: |d| d.write(&d.work, "src/parser.rs", "pub fn parse() { loop {} }\n"),
    },
    Step {
        name: "stage",
        cmd: &["add", "README.md"],
        setup: |d| d.write(&d.work, "README.md", "# demo\nNow with loops.\n"),
    },
    Step { name: "commit again", cmd: &["commit", "-m", "Document loops"], setup: |_| {} },
    Step {
        name: "switch back",
        cmd: &["switch", "main"],
        setup: |_| {},
    },
    Step {
        name: "diverge",
        cmd: &["commit", "-am", "Speed up the lexer"],
        setup: |d| d.write(&d.work, "src/lexer.rs", "pub fn lex() { /* fast */ }\n"),
    },
    Step { name: "merge preview", cmd: &["merge", "feature", "--dry-run"], setup: |_| {} },
    Step { name: "merge", cmd: &["merge", "--no-edit", "feature"], setup: |_| {} },
    Step { name: "reset", cmd: &["reset", "--hard", "HEAD~1"], setup: |_| {} },
    Step {
        name: "rebase",
        cmd: &["rebase", "main"],
        setup: |d| d.git(&d.work, &["switch", "-q", "feature"]),
    },
    Step {
        name: "fast-forward",
        cmd: &["merge", "feature"],
        setup: |d| d.git(&d.work, &["switch", "-q", "main"]),
    },
    Step {
        name: "cherry-pick",
        cmd: &["cherry-pick", "hotfix"],
        setup: |d| {
            d.git(&d.work, &["switch", "-q", "-c", "hotfix", "HEAD~3"]);
            d.commit(&d.work, "SECURITY.md", "report to security@\n", "Fix the security contact");
            d.git(&d.work, &["switch", "-q", "main"]);
        },
    },
    Step {
        name: "stash",
        cmd: &["stash"],
        setup: |d| {
            d.write(&d.work, "src/parser.rs", "pub fn parse() { todo!() }\n");
            d.write(&d.work, "notes.txt", "wip\n");
            d.git(&d.work, &["add", "notes.txt"]);
        },
    },
    Step { name: "stash pop", cmd: &["stash", "pop"], setup: |_| {} },
    Step {
        name: "push",
        cmd: &["push"],
        setup: |d| {
            d.git(&d.work, &["stash", "-q"]);
            d.git(&d.work, &["stash", "drop", "-q"]);
        },
    },
    Step {
        name: "fetch",
        cmd: &["fetch"],
        setup: |d| {
            d.git(&d.mate, &["pull", "-q"]);
            d.commit(&d.mate, "CHANGELOG.md", "## 0.2\n", "Start the changelog");
            d.commit(&d.mate, "CHANGELOG.md", "## 0.2\n- loops\n", "Mention loops");
            d.git(&d.mate, &["push", "-q"]);
        },
    },
    Step { name: "pull", cmd: &["pull", "--no-edit"], setup: |d| d.commit(&d.work, "LICENSE", "MIT\n", "Add a license") },
    Step {
        name: "push rejected",
        cmd: &["push"],
        setup: |d| {
            d.git(&d.mate, &["pull", "-q"]);
            d.commit(&d.mate, "CHANGELOG.md", "## 0.3\n", "Open 0.3");
            d.git(&d.mate, &["push", "-q"]);
            d.git(&d.work, &["push", "-q"]);
            d.commit(&d.work, "src/lexer.rs", "pub fn lex() { /* faster */ }\n", "Make the lexer faster");
        },
    },
    Step {
        name: "merge conflict",
        cmd: &["merge", "--no-edit", "clash"],
        setup: |d| {
            d.git(&d.work, &["switch", "-q", "-c", "clash", "HEAD~1"]);
            d.commit(&d.work, "src/lexer.rs", "pub fn lex() { /* slow but simple */ }\n", "Simplify the lexer");
            d.git(&d.work, &["switch", "-q", "main"]);
        },
    },
];

pub fn run(settings: &Settings, filter: Option<&str>, list: bool) -> Result<i32> {
    if list {
        for s in STEPS {
            println!("{:<20} git {}", s.name, s.cmd.join(" "));
        }
        return Ok(0);
    }
    if !term::is_interactive() {
        bail!("git graft demo needs an interactive terminal");
    }
    let root = std::env::temp_dir().join(format!("graft-demo-{}", std::process::id()));
    let d = Demo { work: root.join("work"), mate: root.join("teammate"), root };
    let _cleanup = Cleanup(d.root.clone());
    std::fs::create_dir_all(&d.work)?;
    let remote = d.root.join("origin.git");
    d.git(&d.root, &["init", "-q", "--bare", "-b", "main", remote.to_str().unwrap()]);
    d.git(&d.work, &["init", "-q", "-b", "main"]);
    d.configure(&d.work);
    d.commit(&d.work, "README.md", "# demo\n", "Initial commit");
    d.commit(&d.work, "src/main.rs", "fn main() {}\n", "Add main");
    d.commit(&d.work, "Cargo.toml", "[package]\nname = \"demo\"\n", "Add the manifest");
    d.git(&d.work, &["remote", "add", "origin", remote.to_str().unwrap()]);
    d.git(&d.work, &["push", "-q", "-u", "origin", "main"]);
    d.git(&d.root, &["clone", "-q", remote.to_str().unwrap(), d.mate.to_str().unwrap()]);
    d.configure(&d.mate);
    std::env::set_current_dir(&d.work)?;

    let quiet = Settings { no_anim: false, ..settings.clone() };
    let mut played = 0;
    for step in STEPS {
        let selected = filter.is_none_or(|f| step.name.contains(f));
        (step.setup)(&d);
        if !selected {
            // Keep the repository in the same state the later steps expect.
            runner::quiet(&d.work, step.cmd);
            if step.name == "merge conflict" {
                runner::quiet(&d.work, &["merge", "--abort"]);
            }
            continue;
        }
        let prompt = if settings.ascii { ">" } else { "❯" };
        println!("\x1b[1;35m{prompt}\x1b[0m \x1b[2m{}\x1b[0m  \x1b[1mgit {}\x1b[0m", step.name, step.cmd.join(" "));
        app::wrap_quiet(step.cmd, &quiet);
        played += 1;
        if step.name == "merge conflict" {
            runner::quiet(&d.work, &["merge", "--abort"]);
        }
        let pause = Duration::from_millis((450.0 / settings.speed.max(0.1)) as u64);
        if app::QUIT.load(Ordering::SeqCst) || quit_pressed(pause) {
            break;
        }
    }
    if played == 0 {
        println!("no scene matches; try git graft demo --list");
    }
    Ok(0)
}

/// Wait between scenes, listening for q / Esc / Ctrl-C so the demo can stop
/// cleanly (and remove its temporary repositories).
fn quit_pressed(pause: Duration) -> bool {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    if term::enter_raw().is_err() {
        std::thread::sleep(pause);
        return false;
    }
    let end = std::time::Instant::now() + pause;
    let mut quit = false;
    while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
        match event::poll(left) {
            Ok(true) => {
                if let Ok(Event::Key(k)) = event::read() {
                    quit = k.kind != KeyEventKind::Release
                        && (matches!(k.code, KeyCode::Char('q') | KeyCode::Esc)
                            || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL)));
                    if quit {
                        break;
                    }
                }
            }
            _ => break,
        }
    }
    term::restore();
    quit
}

struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(std::env::temp_dir());
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
