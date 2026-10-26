//! graft: see what git does. Runs the real git, then animates the commit graph
//! from before to after.

mod anim;
mod app;
mod config;
mod demo;
mod git;
mod gitargs;
mod hooks;
mod layout;
mod live;
mod model;
mod op;
mod preview;
mod render;
mod shell;
#[cfg(test)]
mod tests;
mod watch;

use std::ffi::OsString;

use clap::{CommandFactory, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "graft",
    version,
    about = "See what git does: animated commit graphs in your terminal",
    long_about = "Run any git command through graft and watch the commit graph animate from \
                  before to after.\n\nExamples:\n  graft merge feature\n  graft merge feature --dry-run\n  \
                  graft rebase main\n  graft watch\n\ngv flags go before the git subcommand: graft --speed 2 merge feature",
    allow_external_subcommands = true
)]
struct Cli {
    /// Run git without any animation
    #[arg(long, global = true)]
    no_anim: bool,
    /// Animation speed multiplier (also GRAFT_SPEED)
    #[arg(long, global = true, value_name = "X")]
    speed: Option<f32>,
    /// Plain ASCII glyphs instead of Unicode box drawing
    #[arg(long, global = true)]
    ascii: bool,
    /// Graph orientation (also GRAFT_LAYOUT)
    #[arg(long, global = true, value_enum)]
    layout: Option<LayoutArg>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print the commit graph around HEAD
    Log {
        /// Include every local branch
        #[arg(long)]
        all: bool,
        /// Commits of context below the merge base
        #[arg(short = 'n', long, default_value_t = 3)]
        context: usize,
    },
    /// Type git commands with the predicted result animating as you type
    Live,
    /// Live view that animates whenever refs change, from any tool
    Watch,
    /// Make plain `git` commands animate (edits your shell's startup file)
    Setup {
        /// bash, zsh or fish (default: your $SHELL)
        #[arg(long, value_enum)]
        shell: Option<shell::Shell>,
        /// Don't ask before editing the file
        #[arg(long, short)]
        yes: bool,
        /// Remove the integration again
        #[arg(long)]
        undo: bool,
    },
    /// Print the shell integration code (used by `graft setup`)
    Init {
        #[arg(value_enum)]
        shell: shell::Shell,
    },
    /// Show what a git command would do, without running it
    #[command(arg_required_else_help = true)]
    Preview {
        /// The command line as typed, e.g. "git rebase main" (for key bindings)
        #[arg(long)]
        line: Option<String>,
        /// The git command, e.g. `graft preview rebase main`
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Install or remove the reference-transaction hook used by `graft watch`
    Hooks {
        #[command(subcommand)]
        action: HookAction,
    },
    /// Play every scene in a throwaway repository
    Demo {
        /// Only play scenes whose name contains this
        filter: Option<String>,
        /// List the scenes
        #[arg(long)]
        list: bool,
    },
    /// Show or create the config file
    Config {
        /// Write a commented template if no config file exists
        #[arg(long)]
        init: bool,
    },
    /// Print the man page (roff)
    #[command(hide = true)]
    Man,
    /// Print shell completions
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    #[command(external_subcommand)]
    Git(Vec<OsString>),
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum LayoutArg {
    Vertical,
    Horizontal,
}

#[derive(Subcommand)]
enum HookAction {
    Install,
    Uninstall,
}

fn main() {
    // Behave like other CLI tools when piped into `head`: exit quietly.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    // `git` itself, routed here by the shell integration: every argument belongs
    // to git, so graft's own subcommands (log, …) must not be recognised.
    let argv: Vec<OsString> = std::env::args_os().collect();
    if argv.get(1).is_some_and(|a| a == "--as-git") {
        let rest = argv[2..].to_vec();
        if std::env::var_os("GRAFT_ACTIVE").is_some() {
            // Running under a git that graft started (hooks, aliases): stay out of the way.
            std::process::exit(app::passthrough_now(&rest));
        }
        let settings = app::Settings::new(app::Flags::default(), config::load());
        std::process::exit(app::wrap(rest, &settings));
    }

    let cli = Cli::parse();
    let flags = app::Flags {
        speed: cli.speed,
        no_anim: cli.no_anim,
        ascii: cli.ascii,
        horizontal: cli.layout.map(|l| matches!(l, LayoutArg::Horizontal)),
    };
    let settings = app::Settings::new(flags, config::load());
    let result = match cli.cmd {
        Some(Cmd::Git(args)) => Ok(app::wrap(args, &settings)),
        Some(Cmd::Log { all, context }) => app::log(&settings, all, context),
        Some(Cmd::Live) => live::run(&settings),
        Some(Cmd::Watch) => watch::run(&settings),
        Some(Cmd::Setup { shell, yes, undo }) => shell::setup(shell, yes, undo),
        Some(Cmd::Preview { line, args }) => match line {
            Some(line) => app::preview_line(&line, &settings),
            None => {
                let cmd = gitargs::parse(args.iter().map(OsString::from).collect());
                let words = cmd.command();
                app::preview(&cmd.dir, &words, &settings, true)
            }
        },
        Some(Cmd::Init { shell }) => {
            print!("{}", shell::init(shell));
            Ok(0)
        }
        Some(Cmd::Hooks { action: HookAction::Install }) => hooks::install(),
        Some(Cmd::Hooks { action: HookAction::Uninstall }) => hooks::uninstall(),
        Some(Cmd::Demo { filter, list }) => demo::run(&settings, filter.as_deref(), list),
        Some(Cmd::Config { init }) => config_cmd(init),
        Some(Cmd::Man) => clap_mangen::Man::new(Cli::command()).render(&mut std::io::stdout()).map(|_| 0).map_err(Into::into),
        Some(Cmd::Completions { shell }) => {
            clap_complete::generate(shell, &mut Cli::command(), "graft", &mut std::io::stdout());
            Ok(0)
        }
        None => {
            let _ = Cli::command().print_help();
            Ok(0)
        }
    };
    let code = match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("graft: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

fn config_cmd(init: bool) -> anyhow::Result<i32> {
    let Some(path) = config::path() else { anyhow::bail!("could not work out a config path (is HOME set?)") };
    if init {
        if path.exists() {
            println!("{} already exists", path.display());
        } else {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, config::TEMPLATE)?;
            println!("wrote {}", path.display());
        }
        return Ok(0);
    }
    let state = if path.exists() { "" } else { "  (not created yet; graft config --init)" };
    println!("{}{state}", path.display());
    println!("themes: auto, {}", render::color::preset_names().collect::<Vec<_>>().join(", "));
    Ok(0)
}
