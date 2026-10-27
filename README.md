# Graft

See what git does. Graft animates the commit graph in your terminal every
time you run a git command, and can show you what a command *would* do
before you run it.

```
  ◉─╮  HEAD   main  86e9807 Merge branch 'feature'
  │ │
  ● │               db4c4ce Speed up the lexer
  │ │
  │ ●  feature      df830e1 Document loops
  │ │
  │ ●               27a7efc Teach the parser about loops
  │ │
  ●─╯               13cc2ce Add the parser

 ✔ merged feature into main → 86e9807
```

You keep typing `git` as usual. Graft runs the real git, then animates the
graph from before to after. The final frame stays in your scrollback, and
any key skips the animation.

## Install

**Any Linux (x86_64 or arm64):**

```sh
curl -fsSL https://raw.githubusercontent.com/ashfaqrafin/graft/HEAD/install.sh | sh
```

The installer needs no root. It puts a static `git-graft` binary in `~/.local/bin`
(`/usr/local/bin` when run as root) and verifies its checksum. It also
installs shell completions and the man page, then asks whether plain `git`
commands should animate. Set `GRAFT_VERSION=v0.1.0` to pin a version, or
`GRAFT_SETUP=no` to skip the question.

**Arch Linux (AUR):**

```sh
yay -S git-graft        # or git-graft-git for the latest commit
```

**Debian/Ubuntu and Fedora/openSUSE:** download the `.deb` or `.rpm` from
the [releases page](https://github.com/ashfaqrafin/graft/releases), then run
`sudo apt install ./git-graft_*.deb` or `sudo dnf install ./git-graft-*.rpm`.

**From source:**

```sh
cargo install --locked --git https://github.com/ashfaqrafin/graft git-graft
# or, in a clone:
make && sudo make install          # PREFIX=~/.local for a user install
```

Then try `git graft demo`, which plays every animation in a throwaway repository.

The program is called `git-graft`, so git runs it as `git graft …`. Arch's
official repos already have an unrelated package named `graft`, so don't
install that one.

## Use it like git

```sh
git graft setup          # one time; asks before editing ~/.bashrc, ~/.zshrc or fish config
```

After that, plain git commands animate:

```sh
git merge feature
git rebase main
git push
```

Under the hood `git` becomes a small shell function that hands commands to
graft:
- **Unchanged behavior:** anything graft doesn't animate (`status`, `log`,
  `diff`, …) goes straight to git, at about 1 ms of extra startup.
- **Your git setup keeps working:** global options (`git -C dir …`,
  `git -c key=value …`), your aliases (`git co` → checkout), editors, pagers
  and exit codes all work as before.
- **Off when output isn't a terminal:** scripts and pipes get plain git.

To remove the integration, run `git graft setup --undo`.

## See it before you run it

Three ways to preview a command. None of them change anything in the
repository.

| How | What you do |
| --- | --- |
| **While typing** | Type a git command at your prompt, press **Alt+V** (Alt+G in fish). The predicted result animates below and your command line comes back untouched |
| **Live mode** | `git graft live`. Type git commands and watch the predicted tree update as you type. Enter runs the command for real. Tab completes branch names |
| **Flag** | `git rebase main --preview`, or `git merge feature --dry-run` |

Previews use git's own machinery wherever it exists:
- **Merges:** `git merge-tree` predicts the merge commit, or the exact files
  that would conflict.
- **Rebase, cherry-pick and revert:** `git replay --ref-action=print`
  computes the real rewritten commits without moving any ref, so you see a
  rebase stop at the exact commit that would conflict. This needs git 2.44
  or newer.
- **Everything else** (switch, branch, tag, reset, commit, stash, add/rm/restore
  and push) is worked out from refs and `git status`.
- **Push and pull** are predicted from what you last fetched, including a
  push the remote would reject. `fetch` has to ask the remote, so it is
  previewed only with `--preview`, never while typing.

## Other commands

| Command | What it does |
| --- | --- |
| `git graft watch` | Full-screen live view that animates whenever refs change, from any tool (IDE, GUI, another terminal) |
| `git graft log [--all] [-n N]` | Print the graph around HEAD |
| `git graft demo [scene]` | Play every scene in a throwaway repository (`--list` shows them) |
| `git graft config --init` | Write a commented config file |
| `git graft hooks install` | Add a `reference-transaction` hook that logs ref updates for `git graft watch` |
| `git-graft <git command>` | Run one git command through graft without the shell integration |

### What each operation looks like

| Operation | Animation |
| --- | --- |
| commit | Staged files fly out of the working tree panel into a new commit that pops in at HEAD. The branch label hops up |
| merge | Edges grow up from both parents and the merge commit pops in where they meet. Fast-forwards send a pulse up the branch while the label slides |
| conflict | Edges reach for the merge but stop short, a red ◌ shakes, and the conflicted files fly into the panel |
| rebase / cherry-pick | Originals ghost to ○ and copies fly onto the new base one by one. Then the old commits fade and the graph closes up |
| reset | The label slides back and orphaned commits stay dimmed. `--soft`/`--mixed` pour their changes back into the working tree |
| switch / checkout | HEAD hops to the other branch, then the lanes swap so the checked-out branch is first |
| stash / pop | Dirty files fly into the stash panel, and back out on pop |
| fetch / pull | New commits stream in from the remote box, then `origin/*` advances |
| push | Commits travel up to the remote and `origin/*` follows. A rejected push bounces back red |

### Watching other tools

`git graft watch` notices ref changes made by anything: your IDE, a GUI, another
terminal. `git graft hooks install` adds a `reference-transaction` hook (git 2.28+)
that also logs every ref update to `.git/graft/events`. `git graft hooks uninstall`
removes it. The installer never overwrites a hook it didn't create.

### Options

| Flag / variable | Effect |
| --- | --- |
| `--speed X`, `GRAFT_SPEED` | Animation speed multiplier |
| `--no-anim`, `GRAFT_NO_ANIM`, `CI` | Just run git |
| `--layout vertical\|horizontal`, `GRAFT_LAYOUT` | Newest on top, or newest on the right |
| `--ascii`, `GRAFT_ASCII` | Plain `\| - * /` glyphs |
| `NO_COLOR` | No colors |
| `GRAFT_THEME` | `light`, `dark` or a preset name (overrides background detection) |
| `GRAFT_REDUCED_MOTION=1` | Show the final frame without motion |
| `GRAFT_CONFIG` | Use a different config file |

Animation is skipped when stdout isn't a terminal. On terminals narrower
than 40 columns, graft prints a one-line summary instead.

### Config file

`git graft config --init` writes a commented template to `~/.config/graft/config.toml`
(or `$XDG_CONFIG_HOME/graft/config.toml`). Flags beat environment variables,
which beat the file. A broken file is reported once and ignored, so a typo
never stops git from running.

```toml
speed = 1.5
layout = "horizontal"
theme = "catppuccin"     # or auto, tokyonight, tokyonight-day, catppuccin-latte,
                         # gruvbox, nord, dracula, solarized-light

[colors]                 # override any color of the chosen theme
local = "#a6e3a1"
lanes = ["#89b4fa", "#f9e2af", "#cba6f7"]
```

Color names you can override: `fg`, `muted`, `oid`, `local`, `remote`, `tag`,
`head`, `ok`, `warn`, `err`, `accent` and `lanes`. Fades always blend toward
your terminal's real background, which graft reads with an OSC 11 query.

## How it works

Graft never simulates the commands it animates:

```
snapshot BEFORE → run real git → snapshot AFTER → diff → animate BEFORE → AFTER
```

- **Reading** uses [gix](https://github.com/GitoxideLabs/gitoxide) in-process.
  Only the *visible subgraph* is loaded: a paint-down walk from the
  interesting tips to their merge base, plus a few commits of context. Long
  linear runs collapse into `⋯ n more commits`.
- **The diff** (`model/delta.rs`) produces `Spawn`, `Vanish`, `RefMoved`,
  `Reparent`, `HeadMoved` and similar changes. Rewritten commits are paired
  with their originals by summary.
- **Layout** runs three times: the compact before graph, the union of both,
  and the compact after graph. The animation morphs between them, so the
  first and last frames match `git graft log`. Lane assignment knows nothing about
  the screen. An `Orientation` (vertical or horizontal) turns (lane, row) into
  cells, and edges are routed in the same transposed coordinates.
- **Animation** is a base state plus tweens. `sample(t)` is pure, which makes
  skipping, `--speed` and frame tests free.
- **Rendering** writes into a cell buffer and sends only the changed cells,
  wrapped in synchronized output. Edges are per-cell direction masks, so
  junctions pick the right box glyph on their own. Edges are revealed half a
  cell at a time, and particles leave braille trails. Wrap mode draws in an
  inline viewport, and `git graft watch` uses the alternate screen.

```
src/
  git/      reader (gix), runner (real git), snapshot, status
  model/    graph (visible subgraph, collapsing), delta
  layout/   lane assignment, orientations
  anim/     tween, timeline, scene, player, scenes/ (merge, remote, panels, captions)
  render/   buffer, color, draw, term
  hooks/    reference-transaction installer
  gitargs   git's global options and aliases     preview   predictions
  shell     shell integration, setup             live      `git graft live`
```

## Performance

Measured on git/git (82k commits) with a release build:

| Metric | Target | Measured |
| --- | --- | --- |
| `graft --version` | — | 1.6 ms |
| `git graft log` (process start → output) | < 20 ms | ~12 ms |
| Sample + draw one frame | < 2 ms | ~13 µs |
| Typical animation | 0.6–1.5 s | 0.6–1.6 s (a conflict runs ~2 s) |

## Testing

```sh
cargo test                                # everything
GRAFT_FRAMES=1 cargo test -- --nocapture     # also print sampled frames
cargo insta review                        # after an intentional visual change
```

- **Scenario tests** script fixture repos with the real git. They cover
  commit, fast-forward, true merge, octopus merges of up to four branches,
  criss-cross merges, branches that contain merges, conflicts, rebase
  (including branches with merges), reset, stash, cherry-pick, checkout,
  detached HEAD, the first commit in an empty repository and the horizontal
  layout.
- **Preview tests** predict every kind of command and assert that the
  repository's refs, index and working tree are byte-for-byte unchanged
  afterwards.
- **Golden frames** (`insta`) render each main scene at t = 0, ¼, ½, ¾ and 1
  as plain text. Fixtures use a fixed clock and ignore your git config, so
  commit ids, and therefore the snapshots, are identical on every machine.
- **Property tests** (`proptest`) generate random DAGs and random
  before/after subsets. In both orientations they check that children come
  before parents, that no two commits share a cell, and that no edge runs
  through a commit.
- Sampling is pure, and one frame (sample + draw) stays within the time budget.

## Resizing

If the terminal is resized mid-animation, graft rebuilds the scene for the new
size and carries on from the same moment. Terminals that re-wrap old lines
when they get narrower (most do) can leave a fragment of the previous frame
above the new one. graft never climbs past its own rows to clean that up, so
your earlier output is never overwritten.

## Releasing

1. Bump `version` in `Cargo.toml`, commit, then `git tag v0.1.0 && git push --tags`.
2. The `release` workflow builds static x86_64 and arm64 binaries, `.deb`
   and `.rpm` packages and checksums, and publishes them as a GitHub release,
   which `install.sh` downloads from.
3. AUR: in `packaging/aur/git-graft`, bump `pkgver`, run `updpkgsums`, then
   `makepkg --printsrcinfo > .SRCINFO` and push to `aur.archlinux.org/git-graft.git`.
