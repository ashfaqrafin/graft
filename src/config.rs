//! `~/.config/graft/config.toml`. Command-line flags beat environment variables,
//! which beat this file.
//!
//! The format is a small subset of TOML: `key = value` lines, `[section]`
//! headers, `#` comments; values are strings, numbers, booleans or arrays of
//! strings.

use std::path::PathBuf;

use anyhow::{Result, bail};

pub const TEMPLATE: &str = r##"# graft configuration

# Animation speed multiplier.
# speed = 1.0

# "vertical" (newest on top) or "horizontal" (newest on the right).
# layout = "vertical"

# "auto" picks a light or dark theme from your terminal's background.
# Presets: tokyonight, tokyonight-day, catppuccin, catppuccin-latte, gruvbox,
# nord, dracula, solarized-light.
# theme = "auto"

# Plain ASCII glyphs.
# ascii = false

# Show the final frame without motion.
# reduced_motion = false

# Set to false to make graft a plain git passthrough.
# animate = true

# Override individual colors of the chosen theme.
[colors]
# local = "#9ece6a"
# remote = "#f7768e"
# tag = "#e0af68"
# head = "#7dcfff"
# lanes = ["#7aa2f7", "#9ece6a", "#bb9af7", "#ff9e64"]
"##;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub speed: Option<f32>,
    pub layout: Option<String>,
    pub theme: Option<String>,
    pub ascii: Option<bool>,
    pub reduced_motion: Option<bool>,
    pub animate: Option<bool>,
    /// `[colors]` overrides, as (name, values).
    pub colors: Vec<(String, Vec<String>)>,
}

pub fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("GRAFT_CONFIG") {
        return Some(PathBuf::from(p));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("graft").join("config.toml"))
}

/// Load the config file. A missing file is an empty config; a broken one is
/// reported once and ignored, so a typo never stops git from running.
pub fn load() -> Config {
    let Some(path) = path() else { return Config::default() };
    let Ok(text) = std::fs::read_to_string(&path) else { return Config::default() };
    match parse(&text) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("graft: ignoring {}: {e}", path.display());
            Config::default()
        }
    }
}

#[derive(Debug, PartialEq)]
enum Value {
    Str(String),
    Num(f32),
    Bool(bool),
    List(Vec<String>),
}

pub fn parse(text: &str) -> Result<Config> {
    let mut c = Config::default();
    let mut section = String::new();
    for (n, raw) in text.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let at = |msg: &str| anyhow::anyhow!("line {}: {msg}", n + 1);
        if let Some(name) = line.strip_prefix('[') {
            section = name.strip_suffix(']').ok_or_else(|| at("unclosed section header"))?.trim().to_string();
            if section != "colors" {
                bail!(at(&format!("unknown section [{section}]")));
            }
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| at("expected key = value"))?;
        let key = key.trim();
        let value = parse_value(value.trim()).ok_or_else(|| at("could not read the value"))?;
        if section == "colors" {
            let values = match value {
                Value::Str(s) => vec![s],
                Value::List(l) => l,
                _ => bail!(at("colors are strings like \"#7aa2f7\"")),
            };
            c.colors.push((key.to_string(), values));
            continue;
        }
        match (key, value) {
            ("speed", Value::Num(v)) if v > 0.0 => c.speed = Some(v),
            ("layout", Value::Str(s)) if s == "vertical" || s == "horizontal" => c.layout = Some(s),
            ("theme", Value::Str(s)) => c.theme = Some(s),
            ("ascii", Value::Bool(b)) => c.ascii = Some(b),
            ("reduced_motion", Value::Bool(b)) => c.reduced_motion = Some(b),
            ("animate", Value::Bool(b)) => c.animate = Some(b),
            ("speed", _) => bail!(at("speed must be a positive number")),
            ("layout", _) => bail!(at("layout must be \"vertical\" or \"horizontal\"")),
            ("theme", _) => bail!(at("theme must be a string")),
            ("ascii" | "reduced_motion" | "animate", _) => bail!(at(&format!("{key} must be true or false"))),
            (k, _) => bail!(at(&format!("unknown setting '{k}'"))),
        }
    }
    Ok(c)
}

/// Drop a trailing `# comment`, leaving `#` inside quotes alone.
fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    for (i, ch) in line.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..i],
            _ => {}
        }
    }
    line
}

fn parse_value(v: &str) -> Option<Value> {
    if let Some(inner) = v.strip_prefix('"') {
        return Some(Value::Str(inner.strip_suffix('"')?.to_string()));
    }
    if let Some(inner) = v.strip_prefix('[') {
        let inner = inner.strip_suffix(']')?;
        let items = inner
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.strip_prefix('"')?.strip_suffix('"').map(str::to_string))
            .collect::<Option<Vec<_>>>()?;
        return Some(Value::List(items));
    }
    match v {
        "true" => Some(Value::Bool(true)),
        "false" => Some(Value::Bool(false)),
        _ => v.parse().ok().map(Value::Num),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_settings_and_colors() {
        let c = parse(
            "# hi\nspeed = 1.5\nlayout = \"horizontal\" # trailing\ntheme = \"nord\"\nascii = false\n\n[colors]\nlocal = \"#00ff00\"\nlanes = [\"#111111\", \"#222222\"]\n",
        )
        .unwrap();
        assert_eq!(c.speed, Some(1.5));
        assert_eq!(c.layout.as_deref(), Some("horizontal"));
        assert_eq!(c.theme.as_deref(), Some("nord"));
        assert_eq!(c.ascii, Some(false));
        assert_eq!(c.colors[0], ("local".into(), vec!["#00ff00".into()]));
        assert_eq!(c.colors[1].1.len(), 2);
    }

    #[test]
    fn template_is_valid() {
        assert_eq!(parse(TEMPLATE).unwrap(), Config::default());
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(parse("sped = 2").is_err());
        assert!(parse("layout = \"diagonal\"").is_err());
        assert!(parse("[colours]").is_err());
    }
}
