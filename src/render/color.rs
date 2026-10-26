//! Colors, terminal capability detection and the theme.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const WHITE: Rgb = Rgb(255, 255, 255);

    pub fn hex(v: u32) -> Rgb {
        Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    pub fn lerp(self, other: Rgb, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Rgb(m(self.0, other.0), m(self.1, other.1), m(self.2, other.2))
    }

    fn luma(self) -> f32 {
        0.2126 * self.0 as f32 + 0.7152 * self.1 as f32 + 0.0722 * self.2 as f32
    }

    /// Nearest xterm-256 index.
    pub fn to_256(self) -> u8 {
        let Rgb(r, g, b) = self;
        let q = |v: u8| if v < 48 { 0 } else if v < 115 { 1 } else { (v - 35) / 40 };
        let (qr, qg, qb) = (q(r), q(g), q(b));
        let lv = [0u8, 95, 135, 175, 215, 255];
        let cube = Rgb(lv[qr as usize], lv[qg as usize], lv[qb as usize]);
        let avg = (r as u16 + g as u16 + b as u16) / 3;
        let gi = if avg > 238 { 23 } else { avg.saturating_sub(3) / 10 } as u8;
        let gv = 8 + 10 * gi;
        let dist = |c: Rgb| {
            let d = |a: u8, b: u8| (a as i32 - b as i32).pow(2);
            d(c.0, r) + d(c.1, g) + d(c.2, b)
        };
        if dist(Rgb(gv, gv, gv)) < dist(cube) { 232 + gi } else { 16 + 36 * qr + 6 * qg + qb }
    }

    /// Nearest of the 16 basic ANSI colors, as an SGR foreground code (30-37, 90-97).
    pub fn to_16(self) -> u8 {
        const BASE: [(u8, Rgb); 16] = [
            (30, Rgb(0, 0, 0)),
            (31, Rgb(205, 49, 49)),
            (32, Rgb(13, 188, 121)),
            (33, Rgb(229, 229, 16)),
            (34, Rgb(36, 114, 200)),
            (35, Rgb(188, 63, 188)),
            (36, Rgb(17, 168, 205)),
            (37, Rgb(229, 229, 229)),
            (90, Rgb(102, 102, 102)),
            (91, Rgb(241, 76, 76)),
            (92, Rgb(35, 209, 139)),
            (93, Rgb(245, 245, 67)),
            (94, Rgb(59, 142, 234)),
            (95, Rgb(214, 112, 214)),
            (96, Rgb(41, 184, 219)),
            (97, Rgb(255, 255, 255)),
        ];
        let d = |c: Rgb| {
            let d = |a: u8, b: u8| (a as i32 - b as i32).pow(2);
            d(c.0, self.0) + d(c.1, self.1) + d(c.2, self.2)
        };
        BASE.iter().min_by_key(|(_, c)| d(*c)).map(|(code, _)| *code).unwrap_or(37)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    True,
    C256,
    C16,
    Mono,
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub depth: Depth,
    pub unicode: bool,
    pub light: bool,
    pub bg: Rgb,
    pub fg: Rgb,
    pub muted: Rgb,
    pub oid: Rgb,
    pub lanes: [Rgb; 8],
    pub local: Rgb,
    pub remote: Rgb,
    pub tag: Rgb,
    pub head: Rgb,
    pub pill_fg: Rgb,
    pub ok: Rgb,
    pub warn: Rgb,
    pub err: Rgb,
    pub accent: Rgb,
}

/// The colors a theme is derived from.
struct Palette {
    name: &'static str,
    light: bool,
    bg: u32,
    fg: u32,
    muted: u32,
    red: u32,
    green: u32,
    yellow: u32,
    blue: u32,
    magenta: u32,
    cyan: u32,
    orange: u32,
    teal: u32,
}

#[rustfmt::skip]
const PALETTES: &[Palette] = &[
    Palette { name: "tokyonight", light: false, bg: 0x1a1b26, fg: 0xc0caf5, muted: 0x565f89, red: 0xf7768e, green: 0x9ece6a, yellow: 0xe0af68, blue: 0x7aa2f7, magenta: 0xbb9af7, cyan: 0x7dcfff, orange: 0xff9e64, teal: 0x73daca },
    Palette { name: "tokyonight-day", light: true, bg: 0xf5f5f5, fg: 0x343b58, muted: 0x8990b3, red: 0xc64343, green: 0x587539, yellow: 0x8c6c3e, blue: 0x2e7de9, magenta: 0x9854f1, cyan: 0x007197, orange: 0xb15c00, teal: 0x387068 },
    Palette { name: "catppuccin", light: false, bg: 0x1e1e2e, fg: 0xcdd6f4, muted: 0x6c7086, red: 0xf38ba8, green: 0xa6e3a1, yellow: 0xf9e2af, blue: 0x89b4fa, magenta: 0xcba6f7, cyan: 0x89dceb, orange: 0xfab387, teal: 0x94e2d5 },
    Palette { name: "catppuccin-latte", light: true, bg: 0xeff1f5, fg: 0x4c4f69, muted: 0x8c8fa1, red: 0xd20f39, green: 0x40a02b, yellow: 0xdf8e1d, blue: 0x1e66f5, magenta: 0x8839ef, cyan: 0x04a5e5, orange: 0xfe640b, teal: 0x179299 },
    Palette { name: "gruvbox", light: false, bg: 0x282828, fg: 0xebdbb2, muted: 0x928374, red: 0xfb4934, green: 0xb8bb26, yellow: 0xfabd2f, blue: 0x83a598, magenta: 0xd3869b, cyan: 0x8ec07c, orange: 0xfe8019, teal: 0x689d6a },
    Palette { name: "nord", light: false, bg: 0x2e3440, fg: 0xd8dee9, muted: 0x616e88, red: 0xbf616a, green: 0xa3be8c, yellow: 0xebcb8b, blue: 0x81a1c1, magenta: 0xb48ead, cyan: 0x88c0d0, orange: 0xd08770, teal: 0x8fbcbb },
    Palette { name: "dracula", light: false, bg: 0x282a36, fg: 0xf8f8f2, muted: 0x6272a4, red: 0xff5555, green: 0x50fa7b, yellow: 0xf1fa8c, blue: 0xbd93f9, magenta: 0xff79c6, cyan: 0x8be9fd, orange: 0xffb86c, teal: 0xa4ffff },
    Palette { name: "solarized-light", light: true, bg: 0xfdf6e3, fg: 0x586e75, muted: 0x93a1a1, red: 0xdc322f, green: 0x859900, yellow: 0xb58900, blue: 0x268bd2, magenta: 0xd33682, cyan: 0x2aa198, orange: 0xcb4b16, teal: 0x6c71c4 },
];

pub fn preset_names() -> impl Iterator<Item = &'static str> {
    PALETTES.iter().map(|p| p.name)
}

impl Theme {
    /// Pick a theme: `GRAFT_THEME`, then the configured name, then the terminal
    /// background. `colors` are `[colors]` overrides from the config file.
    pub fn detect(ascii: bool, bg: Option<Rgb>, configured: Option<&str>, colors: &[(String, Vec<String>)]) -> Theme {
        let depth = detect_depth();
        let unicode = !ascii && unicode_locale();
        let env = std::env::var("GRAFT_THEME").ok();
        let name = env.as_deref().or(configured).unwrap_or("auto");
        let auto_light = || bg.map(|b| b.luma() > 140.0).or_else(colorfgbg_light).unwrap_or(false);
        let preset = match name {
            "light" => "tokyonight-day",
            "dark" => "tokyonight",
            n if PALETTES.iter().any(|p| p.name == n) => n,
            _ if auto_light() => "tokyonight-day",
            _ => "tokyonight",
        };
        let mut theme = Theme::preset(preset, depth, unicode).expect("built-in preset");
        // Fades blend toward the real background whatever the palette says.
        if let Some(bg) = bg {
            theme.bg = bg;
        }
        theme.apply(colors);
        theme
    }

    pub fn preset(name: &str, depth: Depth, unicode: bool) -> Option<Theme> {
        let p = PALETTES.iter().find(|p| p.name == name)?;
        let c = Rgb::hex;
        Some(Theme {
            depth,
            unicode,
            light: p.light,
            bg: c(p.bg),
            fg: c(p.fg),
            muted: c(p.muted),
            oid: c(p.yellow),
            lanes: [c(p.blue), c(p.green), c(p.magenta), c(p.orange), c(p.cyan), c(p.red), c(p.teal), c(p.yellow)],
            local: c(p.green),
            remote: c(p.red),
            tag: c(p.yellow),
            head: c(p.cyan),
            pill_fg: if p.light { Rgb::WHITE } else { c(p.bg).lerp(Rgb(0, 0, 0), 0.3) },
            ok: c(p.green),
            warn: c(p.yellow),
            err: c(p.red),
            accent: c(p.cyan),
        })
    }

    #[cfg(test)]
    pub fn dark(depth: Depth, unicode: bool) -> Theme {
        Theme::preset("tokyonight", depth, unicode).unwrap()
    }

    /// Apply `[colors]` overrides; unknown names and bad values are ignored.
    fn apply(&mut self, colors: &[(String, Vec<String>)]) {
        for (key, values) in colors {
            let parsed: Vec<Rgb> = values.iter().filter_map(|v| parse_hex(v)).collect();
            let Some(&first) = parsed.first() else { continue };
            let slot = match key.as_str() {
                "lanes" => {
                    for (i, c) in parsed.iter().take(self.lanes.len()).enumerate() {
                        self.lanes[i] = *c;
                    }
                    continue;
                }
                "fg" => &mut self.fg,
                "muted" => &mut self.muted,
                "oid" => &mut self.oid,
                "local" => &mut self.local,
                "remote" => &mut self.remote,
                "tag" => &mut self.tag,
                "head" => &mut self.head,
                "ok" => &mut self.ok,
                "warn" => &mut self.warn,
                "err" => &mut self.err,
                "accent" => &mut self.accent,
                _ => continue,
            };
            *slot = first;
        }
    }

    pub fn lane(&self, lane: usize) -> Rgb {
        self.lanes[lane % self.lanes.len()]
    }

    /// Fade a color toward the background.
    pub fn fade(&self, c: Rgb, alpha: f32) -> Rgb {
        self.bg.lerp(c, alpha)
    }

    /// The "glow" color: toward white on dark backgrounds, toward full saturation on light.
    pub fn glow(&self, c: Rgb, amount: f32) -> Rgb {
        let target = if self.light { Rgb::hex(0x000000) } else { Rgb::WHITE };
        c.lerp(target, amount * 0.75)
    }
}

fn detect_depth() -> Depth {
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return Depth::Mono;
    }
    let colorterm = std::env::var("COLORTERM").unwrap_or_default().to_lowercase();
    if colorterm.contains("truecolor") || colorterm.contains("24bit") {
        return Depth::True;
    }
    let term = std::env::var("TERM").unwrap_or_default();
    if term == "dumb" {
        return Depth::Mono;
    }
    if ["kitty", "alacritty", "wezterm", "ghostty", "foot", "iterm", "direct"].iter().any(|t| term.contains(t))
        || std::env::var_os("WT_SESSION").is_some()
    {
        return Depth::True;
    }
    if term.contains("256") {
        return Depth::C256;
    }
    Depth::C16
}

fn unicode_locale() -> bool {
    for var in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(v) = std::env::var(var)
            && !v.is_empty() {
                let v = v.to_lowercase();
                return v.contains("utf-8") || v.contains("utf8");
            }
    }
    // No locale at all: most modern terminals are UTF-8 anyway.
    true
}

fn parse_hex(s: &str) -> Option<Rgb> {
    let h = s.strip_prefix('#').unwrap_or(s);
    (h.len() == 6).then(|| u32::from_str_radix(h, 16).ok().map(Rgb::hex)).flatten()
}

fn colorfgbg_light() -> Option<bool> {
    let v = std::env::var("COLORFGBG").ok()?;
    let bg: u8 = v.rsplit(';').next()?.parse().ok()?;
    Some(matches!(bg, 7 | 9..=15))
}
