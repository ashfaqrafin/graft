//! Spawning the real `git` binary. Mutations always go through here so that
//! hooks, editors, credential prompts and every config knob behave exactly as
//! the user expects.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

/// Run git with inherited stdio and return its exit code. Ctrl-C is delivered to
/// git (same process group) while we ignore it, so we can still report git's status.
pub fn run<I, S>(args: I) -> i32
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut child = match Command::new("git").args(args).env("GRAFT_ACTIVE", "1").spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("graft: failed to run git: {e}");
            return 127;
        }
    };
    let _guard = SigintIgnored::new();
    match child.wait() {
        Ok(status) => exit_code(status),
        Err(_) => 1,
    }
}

/// Run git in `dir` with stdout/stderr captured. Used for porcelain queries.
pub fn output(dir: Option<&Path>, args: &[&str]) -> Option<(i32, Vec<u8>)> {
    let mut cmd = Command::new("git");
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    let out = cmd.args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    Some((exit_code(out.status), out.stdout))
}

/// Like `output`, with extra environment variables.
pub fn output_env(dir: Option<&Path>, args: &[&str], env: &[(&str, &str)]) -> Option<(i32, Vec<u8>)> {
    let mut cmd = Command::new("git");
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    cmd.envs(env.iter().copied());
    let out = cmd.args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    Some((exit_code(out.status), out.stdout))
}

/// Run git quietly (used by the demo). Returns the exit code.
pub fn quiet(dir: &Path, args: &[&str]) -> i32 {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_EDITOR", "true")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(exit_code)
        .unwrap_or(127)
}

pub fn exit_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    1
}

struct SigintIgnored {
    #[cfg(unix)]
    prev: libc::sighandler_t,
}

impl SigintIgnored {
    fn new() -> Self {
        #[cfg(unix)]
        unsafe {
            SigintIgnored { prev: libc::signal(libc::SIGINT, libc::SIG_IGN) }
        }
        #[cfg(not(unix))]
        SigintIgnored {}
    }
}

impl Drop for SigintIgnored {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::signal(libc::SIGINT, self.prev);
        }
    }
}
