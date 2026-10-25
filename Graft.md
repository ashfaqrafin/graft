# Animated Git Visualizer CLI: Project Guideline

Working name: Graft (not final). Author: Ashfaq. Status: planning.

## 1. Vision and core idea

A blazing fast command line tool that shows an animated commit tree in the terminal, so people can see what git is doing when they merge, stash, commit, push, rebase and more.

The key design decision: **do not simulate git**. Take a snapshot of the graph, let real git do the work, take a second snapshot, then animate the difference.

```
snapshot BEFORE → run real git → snapshot AFTER → diff → animate BEFORE → AFTER
```

This keeps the tool correct for every edge case (octopus merges, conflicts, unusual configs) without reimplementing git.

For a preview mode (show what a merge would do before running it), use `git merge-tree --write-tree HEAD feature` (git 2.38+). It computes the merge result without touching the working tree or refs. It exits with code 1 on conflicts and lists the conflicted files, so the animation can flash them red.

### Modes

| Mode | Command | Behavior |
| --- | --- | --- |
| Wrap | `gv merge feature` | Runs real git, then animates before to after |
| Preview | `gv merge feature --dry-run` | Animates the predicted result and changes nothing |
| Watch | `gv watch` | Live view that animates whenever refs change (any tool, any IDE) |

## 2. Language and stack

Recommended: **Rust**.

- Startup in about 1 to 3 ms (no runtime or GC). This matters because the tool runs on every git command.
- `gix` (gitoxide) for fast in-process repo reads, with no process spawn per query.
- `crossterm` for terminal I/O, and optionally `ratatui` for buffer diffing and the inline viewport.
- `clap` for the CLI, `smallvec` for small parent lists, `notify` for watch mode.

Go with Bubble Tea is a fine alternative and quicker to prototype, but `go-git` is noticeably slower on large repos and the GC adds frame jitter.

Use `gix` for **reading** only. For **mutating** operations, spawn the real `git` binary, since `gix` does not yet cover every write operation (merge, rebase) with full parity.

```toml
[profile.release]
lto = "fat"
codegen-units = 1
panic = "abort"
opt-level = 3
```

Avoid an async runtime. One render thread plus one loader thread is enough.

## 3. Architecture

```
src/
  main.rs           # clap, dispatch
  git/
    reader.rs       # gix: refs, commits, status, stash reflog
    runner.rs       # spawn real git (inherit stdio)
    snapshot.rs     # Snapshot { nodes, refs, head, worktree, stash }
  model/
    graph.rs        # Node, Edge, visible-subgraph selection
    delta.rs        # diff(before, after) -> Vec<Change>
  layout/
    lanes.rs        # row/column assignment
  anim/
    tween.rs        # easing, tracks
    timeline.rs     # sequencing / parallel groups
    scenes/         # merge.rs, rebase.rs, stash.rs, push.rs ...
  render/
    buffer.rs       # cell grid + diff
    draw.rs         # nodes, edges, labels, panels
    term.rs         # raw mode, sync output, cleanup guard
  hooks/            # reference-transaction hook installer
```

Keep the layers one-directional: `git → model → layout → anim → render`. Everything except `render` is then testable headlessly.

## 4. Reading git fast

Never load the whole history. Build a **visible subgraph**:

1. Collect the tips involved (HEAD, the target branch, upstream, and so on).
2. Compute the merge base(s).
3. Walk from each tip back to the merge base, plus 2 to 3 commits of context beyond it.
4. Collapse long linear runs into a single `… 14 commits …` node.

This keeps the graph at roughly 10 to 40 nodes whether the repo has 100 commits or 1 million.

Speed tips:

- Make sure `commit-graph` files exist (`git commit-graph write --reachable`). `gix` uses them for fast traversal.
- Load refs in one pass (packed-refs plus loose).
- Load in a background thread and draw the first frame (even an empty tree) immediately.
- For worktree state, `git status --porcelain=v2 -z` is fine and benefits from fsmonitor and the untracked cache.

```rust
pub struct Snapshot {
    pub nodes: Vec<Node>,          // indexed by NodeId
    pub refs: Vec<RefLabel>,       // branch/tag/remote -> NodeId
    pub head: HeadState,           // Attached(branch) | Detached(NodeId)
    pub worktree: WorktreeStatus,  // staged/unstaged/untracked counts + files
    pub stash: Vec<StashEntry>,    // from refs/stash reflog
}

pub struct Node {
    pub oid: gix::ObjectId,
    pub parents: SmallVec<[NodeId; 2]>,
    pub summary: String,
    pub collapsed: Option<u32>,    // Some(n) = "n commits hidden"
}
```

## 5. Layout

Use the same idea as `git log --graph`:

1. Topologically sort (children before parents).
2. Walk the order and assign each commit a **lane** (column). A commit inherits the lane of its first child. Additional branches claim the leftmost free lane.
3. Row is the position in the topological order.
4. Convert (lane, row) to terminal cells, for example `x = lane * 4`, `y = row * 2`, leaving space for edge characters between rows.

Pick an orientation early. **Vertical** (newest on top) is simplest and fits narrow terminals. **Horizontal** (left to right, like GitHub's network view) feels more natural for branching and merging but needs more width. Make layout a trait so both can be added.

**Layout must be stable across the diff.** A node present in both snapshots must keep its position, or the animation looks like everything jumped. Lay out the *union* of before and after nodes once, then show or hide nodes by phase.

## 6. The diff that drives every animation

```rust
pub enum Change {
    Spawn(NodeId),                              // new commit
    Vanish(NodeId),                             // dropped or orphaned
    RefMoved { name: RefName, from: NodeId, to: NodeId },
    Reparent { node: NodeId, old: NodeId, new: NodeId },  // rebase
    HeadMoved { from: HeadState, to: HeadState },
    WorktreeChanged(WorktreeDelta),
    StashPushed(StashEntry),
    StashPopped(StashEntry),
    ConflictFiles(Vec<PathBuf>),
}
```

Each scene (merge, rebase, stash and so on) is a function from `Vec<Change>` to a `Timeline`.

## 7. What each operation looks like

| Operation | Animation |
| --- | --- |
| commit | The working-tree box pours its staged files into a new node that grows at HEAD. The branch label slides onto it. |
| merge (fast-forward) | No new node. The branch label and HEAD slide along existing edges to the target tip. |
| merge (true) | The target branch's edge draws toward HEAD's tip. A new merge node spawns at the junction, both edges converge into it, and the branch label hops onto it. |
| merge (conflict) | The edges stop short of the merge node, the conflicted files appear in a side panel, and the node pulses red. |
| rebase | Original commits dim (ghosted). Copies with new OIDs slide onto the new base one by one. The old ones fade out. |
| cherry-pick | A copy of the commit lifts off its branch, arcs across, and lands on the target tip as a new node. |
| reset | The label slides backward. `--soft`, `--mixed` and `--hard` differ in what happens to the staging and working-tree boxes. Orphaned commits dim. |
| checkout / switch | The HEAD marker moves and the working-tree box flashes. |
| stash push | Dirty files fly from the working-tree box into a vertical stack panel on the side. `pop` reverses it. |
| fetch | The remote-tracking lane (`origin/main`) extends with new commits sliding in from a "wire" at the edge. |
| push | Local commits travel across the wire into the remote lane and the `origin/main` label advances. Rejected pushes bounce back in red. |
| pull | The fetch scene followed by the merge or rebase scene. |

Split the screen into three zones: **graph** (center), **working tree / index / stash** (side panel), and **remote** (a second lane group, or a split pane for push and fetch).

## 8. Animation engine

Keep it simple and deterministic: everything is a function of time.

```rust
pub struct Track {
    pub target: Entity,      // node, edge, label, panel
    pub prop: Prop,          // Pos, Alpha, Color, Progress
    pub from: f32, pub to: f32,
    pub start: f32, pub dur: f32,
    pub ease: fn(f32) -> f32,
}

pub fn ease_out_cubic(t: f32) -> f32 { 1.0 - (1.0 - t).powi(3) }
pub fn ease_in_out(t: f32) -> f32 { t * t * (3.0 - 2.0 * t) }

// state at time t: a pure function, so you can scrub, skip, or test any frame
fn sample(timeline: &Timeline, t: f32) -> SceneState { ... }
```

Because `sample(t)` is pure, skipping (jump to the end on a keypress), speed control (`--speed 2`) and golden-frame tests come for free.

### Terminal-specific tricks

A terminal is a character grid, so smoothness is faked:

- **Edge drawing:** compute the cell path of an edge, then reveal the first `floor(progress * len)` cells with a bright head cell. Use box-drawing characters (`│ ─ ╭ ╮ ╰ ╯`).
- **Smooth motion:** use half-blocks or Braille characters for 2x to 4x sub-cell resolution when moving dots and commit nodes.
- **Fades:** lerp truecolor RGB toward the background color. Fall back to 256 or 16 colors when `COLORTERM` is not `truecolor`.
- **Motion trails:** render the previous 2 to 3 positions at decreasing brightness.

### Frame loop

A fixed 60 fps timer. Poll input with a timeout equal to the time until the next frame, so there is never a busy wait.

```rust
let frame = Duration::from_micros(16_667);
let mut next = Instant::now();
loop {
    let t = start.elapsed().as_secs_f32() * speed;
    let state = sample(&timeline, t);
    renderer.draw(&state)?;
    next += frame;
    if event::poll(next.saturating_duration_since(Instant::now()))? {
        if let Event::Key(_) = event::read()? { break; }   // any key skips
    }
    if t >= timeline.duration { break; }
}
```

## 9. Rendering without flicker

- Keep a **front and back cell buffer**. Each frame, write only the cells that changed (ratatui does this for you).
- Wrap each frame in **synchronized output** (`\x1b[?2026h` ... `\x1b[?2026l`) to prevent tearing in supporting terminals.
- Prefer an **inline viewport** (a fixed N-row region below the prompt) over the alternate screen for wrap mode. The user keeps their scrollback, and after the animation a compact final graph stays in place. Use the alternate screen only for `gv watch` and interactive mode.
- Use a **cleanup guard** (`Drop`) plus a panic hook and a SIGINT handler to always restore the cursor, raw mode and screen.
- Handle `SIGWINCH` by re-laying out on resize.
- Use `unicode-width` for label alignment, since CJK characters and emoji break column math.

## 10. Hooking into the user's workflow

Three integration layers, from simplest to most seamless:

1. **Explicit wrapper:** `gv merge x`, `gv push`. Easiest, and the right MVP.
2. **Shell function** so that `git merge` triggers it automatically, shipped through `gv init zsh|bash|fish` (like zoxide and starship):

```bash
git() {
  case "$1" in
    merge|rebase|stash|push|pull|commit|cherry-pick|reset|checkout|switch)
      command gv "$@" ;;
    *) command git "$@" ;;
  esac
}
```

3. **`reference-transaction` hook** (git 2.28+). It fires on every ref update with `<old> <new> <ref>` lines on stdin, from any client including IDEs and GUIs. Install it with `gv hooks install` and have it append to a small event log or notify a running `gv watch` over a Unix socket.

Critical detail for wrap mode: run git with **inherited stdio first** (so editors, credential prompts and progress output work), and only animate after it exits. Preserve git's exit code.

## 11. Performance budget

| Metric | Target |
| --- | --- |
| Process start to first frame | under 20 ms |
| Snapshot of a huge repo (visible subgraph only) | under 50 ms |
| Frame time | under 2 ms (budget is 16.6 ms) |
| Total animation | 0.6 to 1.5 s, skippable |
| Idle memory | under 20 MB |

How to hit it:

- No allocations in the per-frame path. Pre-compute cell paths and reuse buffers.
- Do not re-layout per frame. Lay out once, then interpolate.
- Measure with `hyperfine` and `cargo flamegraph`. Test on a large repo (the Linux kernel is the classic one) in CI.
- Fast path: if stdout is not a TTY, or `GV_NO_ANIM` or `CI` is set, just run git and exit.

## 12. Accessibility and robustness

- Flags and env: `--no-anim`, `--speed`, `--ascii` (plain `|`, `-`, `*`), `NO_COLOR`, and respect reduced-motion preferences.
- Edge cases to handle explicitly: detached HEAD, octopus merges, shallow clones, worktrees, submodules, empty repos, bare repos, narrow terminals (fall back to a text summary under about 40 columns), and tmux and screen quirks.

## 13. Testing

- **Fixture repos:** script them (`git init`, a sequence of commits, branches, a conflicting merge) so scenarios are reproducible.
- **Golden frames:** render `sample(t)` at t = 0, 0.25, 0.5, 0.75 and 1 to a plain-text grid and snapshot-test it with `insta`. Because sampling is pure, this works without a terminal.
- **Property tests** (`proptest`) on layout: no two nodes share a cell, and edges never cross nodes.
- **Differential tests:** after any wrapped operation, the "after" snapshot must match `git log --graph --format=%H%P` output.

## 14. Roadmap

| Phase | Deliverable |
| --- | --- |
| 1 | Snapshot reader, visible-subgraph selection, static vertical layout; `gv log` prints the tree |
| 2 | Renderer with cell diffing, tween engine, `commit` and `checkout` scenes |
| 3 | Snapshot, diff and animate pipeline, wrap mode, `merge` (fast-forward and true) |
| 4 | `merge --dry-run` via `merge-tree`, conflict scene |
| 5 | `stash`, `reset`, `cherry-pick`, then `rebase` (the hardest because of re-parenting) |
| 6 | Remote lane: `fetch`, `push`, `pull` |
| 7 | `gv init` shell integration, `reference-transaction` hook, `gv watch` |
| 8 | Themes, config file, ASCII fallback, packaging (`cargo-dist`, Homebrew, AUR, `cargo install`), completions via `clap_complete` |

## 15. Pitfalls to avoid

- Reimplementing git operations. It is tempting, but the snapshot diff approach is far more reliable.
- Animating before the real command finishes. It blocks prompts and misleads on failure.
- Letting animation block the user. Always make it skippable, and keep it short for frequent commands like `commit`.
- Assuming truecolor or Unicode support. Detect and degrade gracefully.
- Laying out the before and after graphs independently. Lay out the union once.

## 16. Naming

Candidates: Graft (top pick, since grafting joins one branch onto another and works as a verb), Arbor, Rustle, Canopy, Twig, Sproutline. Before committing, check availability on crates.io, GitHub, Homebrew and the AUR.
