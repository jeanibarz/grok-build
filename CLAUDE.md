# grok-build (Jean fork)

SpaceXAI Grok Build harness/TUI fork used as the **local Grok binary** for
Kookr (`agentType: grok-build`). Upstream is `xai-org/grok-build` (`upstream`).
This fork carries Jean-side fixes and attach/TUI compatibility work.

## Branch policy — `feat/claude-compat` is the fork “main”

| Branch | Role |
|--------|------|
| **`feat/claude-compat`** | **Primary integration branch of this fork.** All day-to-day work, PRs, and Kookr binary installs target this branch. Upstream reintegration lands here so Jean’s line does not diverge into a parallel `main`. |
| `main` | GitHub default / mirror tip. Prefer **not** to develop on it. If something lands on `main` by mistake, reintegrate into `feat/claude-compat` promptly (merge or cherry-pick) so the integration branch stays the single source of truth. |
| `upstream/main` | SpaceXAI upstream. Periodically merge/rebase **into `feat/claude-compat`**, never treat it as the live install tip by itself. |

### What agents must do

1. **Base all worktrees and PRs on `feat/claude-compat`**, not `origin/main`:
   ```bash
   git fetch origin
   git worktree add ../grok-build-<short> -b <feature-branch> origin/feat/claude-compat
   # PR base:
   gh pr create --base feat/claude-compat ...
   ```
2. **Primary checkout** for this repo is expected to stay on `feat/claude-compat`
   (dirty or mid-reintegration work lives there or in a dedicated worktree).
3. **Do not open PRs against `main`** unless the operator explicitly asks.
4. After merging to `feat/claude-compat`, run the binary rebuild steps below
   (auto, no ask). Do **not** auto-restart Kookr.

### Upstream reintegration

When pulling SpaceXAI changes:

```bash
cd ~/git/grok-build   # on feat/claude-compat
git fetch upstream
git merge upstream/main   # or rebase if the operator prefers; ask if unclear
# resolve, test, push feat/claude-compat
```

Do not create a long-lived parallel history on `main` that skips
`feat/claude-compat`.

## Kookr binary install path (read this before finishing any binary-affecting change)

Kookr does **not** run from this git checkout. It runs the installed binary:

| Item | Default |
|------|---------|
| Binary | `~/bin/grok` (`KOOKR_GROK_BIN` may override) |
| Rebuild script | `~/git/kookr/scripts/rebuild-grok.sh` |
| npm alias | `cd ~/git/kookr && pnpm grok:rebuild` |
| Source for rebuild | `GROK_SRC` (default **`~/git/grok-build`**) |
| Expected source tip | **`feat/claude-compat`** (not `main`) |
| Version string | `grok --version` → `0.x… (git-sha)` — SHA is the source commit at build time |

`pnpm prod:update` in kookr **only** updates/restarts the Kookr server worktree
(`kookr-prod`). It **never** rebuilds or reinstalls `grok`.

### Mandatory: auto-rebuild after shipping binary-affecting work

Do this **without waiting for the operator to ask**, whenever any of the
following is true:

1. You merged a PR into **`feat/claude-compat`** that changes runtime behavior
   of the pager/TUI, shell, hooks, CLI flags, or anything linked into
   `xai-grok-pager` / `xai-grok-pager-bin`.
2. You reintegrated upstream (or `main`) into **`feat/claude-compat`** and the
   tip advanced with runtime-affecting commits.
3. You finished a Kookr “implement issue” / self-continuation unit whose PR
   closed a fork bug that only matters once the installed binary updates.

**Steps (in order):**

```bash
# 1) Put the *install source tree* on feat/claude-compat tip.
#    Default GROK_SRC is ~/git/grok-build — worktree builds do NOT install
#    themselves. Either update that checkout, or pass GROK_SRC.
cd ~/git/grok-build
git fetch origin
git checkout feat/claude-compat
git pull --ff-only origin feat/claude-compat
# Or rebuild from a worktree without moving the primary checkout:
#   GROK_SRC=/path/to/worktree pnpm --dir ~/git/kookr grok:rebuild

# 2) Rebuild + install to ~/bin/grok
cd ~/git/kookr
pnpm grok:rebuild

# 3) Verify the installed SHA matches feat/claude-compat tip
~/bin/grok --version
git -C ~/git/grok-build rev-parse --short HEAD
# Expect the SHAs to match (or GROK_SRC’s HEAD if that was used).
```

**Then report** in the task summary: installed path, `grok --version` output,
and that **already-running** Grok sessions keep the old process until they are
killed/relaunched (new launches only).

### What not to do

- Do **not** treat `origin/main` as the live fork tip or install source.
- Do **not** treat `pnpm prod:update` as “deploy the new Grok binary.”
- Do **not** run `pnpm prod:update` / `prod:restart` (or otherwise restart
  Kookr) without an explicit operator OK — even if a rebuild just finished.
- Do **not** run `pnpm grok:rebuild` while `GROK_SRC` is on a stale branch
  (e.g. old `main` tip, or a finished feature worktree) unless intentional.
- Do **not** stop after “PR merged” for attach/TUI/hook fixes; the merge alone
  does not change `/home/jean/bin/grok`.

### When to also restart Kookr — **ask first, never auto-restart**

- **Binary-only fork fix** (this repo): `pnpm grok:rebuild` is enough for *new*
  sessions; no Kookr restart required if `KOOKR_GROK_BIN` already points at
  `~/bin/grok`. Do the rebuild automatically; **do not** restart Kookr unless
  the operator asked for it.
- **Any Kookr process restart** (`pnpm prod:update`, `pnpm prod:restart`,
  `scripts/prod-restart.sh`, killing port 4800, systemd restart of kookr,
  or any other action that restarts the live Kookr server): **always ask the
  operator and wait for explicit yes** before running it. Auto-rebuild of
  `~/bin/grok` is authorized; auto-restart of Kookr is **not**.
- If a Kookr server / adapter / attach-pipeline change *would* benefit from a
  restart, say so in the task summary and offer the exact command — then stop
  until confirmed.

## Worktrees

Tracked-file edits for Kookr tasks use a **fresh worktree** off
**`origin/feat/claude-compat`** (not the primary checkout, not `origin/main`):

```bash
git fetch origin
git worktree add ../grok-build-<short-name> -b <feature-branch> origin/feat/claude-compat
```

After the PR merges into `feat/claude-compat`, rebuild from that tip (see above).

## Compatibility note

Kookr’s Grok qualification manifest may pin an older reviewed `buildId`/SHA.
Rebuilding still installs the new binary for runtime use; updating the
manifest identity is a separate Kookr-side task when formal re-qualification
is required. Do not skip the binary rebuild because the manifest is stale.
