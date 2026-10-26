//! The `reference-transaction` hook (git 2.28+) fires on every ref update from any
//! client, IDEs and GUIs included. Ours appends the updates to `$GIT_COMMON_DIR/graft/events`,
//! which `graft watch` listens to.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::git::runner;

const MARKER: &str = "# installed by graft";

fn hook_path() -> Result<PathBuf> {
    let (code, out) = runner::output(None, &["rev-parse", "--git-path", "hooks/reference-transaction"])
        .context("could not run git")?;
    if code != 0 {
        bail!("not a git repository");
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out).trim()))
}

fn script() -> String {
    format!(
        r#"#!/bin/sh
{MARKER}
# Records committed ref updates for `graft watch`. Safe to delete.
[ "$1" = committed ] || exit 0
dir="$(git rev-parse --git-common-dir)/graft"
mkdir -p "$dir" || exit 0
log="$dir/events"
if [ -f "$log" ] && [ "$(wc -c < "$log")" -gt 262144 ]; then
  mv -f "$log" "$log.old"
fi
cat >> "$log"
exit 0
"#
    )
}

pub fn install() -> Result<i32> {
    let path = hook_path()?;
    if let Ok(existing) = std::fs::read_to_string(&path)
        && !existing.contains(MARKER) {
            bail!("{} already exists and was not installed by graft; leaving it alone", path.display());
        }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, script())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    println!("installed {}", path.display());
    Ok(0)
}

pub fn uninstall() -> Result<i32> {
    let path = hook_path()?;
    match std::fs::read_to_string(&path) {
        Ok(s) if s.contains(MARKER) => {
            std::fs::remove_file(&path)?;
            println!("removed {}", path.display());
        }
        Ok(_) => bail!("{} was not installed by graft; leaving it alone", path.display()),
        Err(_) => println!("no graft hook installed"),
    }
    Ok(0)
}
