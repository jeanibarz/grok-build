# grok-build (Jean fork)

SpaceXAI Grok Build harness/TUI fork used as the **local Grok binary** for
Kookr (`agentType: grok-build`). Upstream is synced periodically; this repo
carries Jean-side fixes and attach/TUI compatibility work.

## Kookr binary install path (read this before finishing any binary-affecting change)

Kookr does **not** run from this git checkout. It runs the installed binary:

| Item | Default |
|------|---------|
| Binary | `~/bin/grok` (`KOOKR_GROK_BIN` may override) |
| Rebuild script | `~/git/kookr/scripts/rebuild-grok.sh` |
| npm alias | `cd ~/git/kookr && pnpm grok:rebuild` |
| Source for rebuild | `GROK_SRC` (default **`~/git/grok-build`**, not a random worktree) |
| Version string | `grok --version` → `0.x… (git-sha)` — the SHA is the **source commit at build time** |

`pnpm prod:update` in kookr **only** updates/restarts the Kookr server worktree
(`kookr-prod`). It **never** rebuilds or reinstalls `grok`. Using it alone
after a TUI/fork fix leaves prod on the old binary.

### Mandatory: auto-rebuild after shipping binary-affecting work

Do this **without waiting for the operator to ask**, whenever any of the
following is true:

1. You merged a PR into `main` that changes runtime behavior of the pager/TUI,
   shell, hooks, CLI flags, or anything linked into `xai-grok-pager` /
   `xai-grok-pager-bin`.
2. You updated **`feat/claude-compat`** (or any branch Kookr/docs treat as the
   live fork tip) with the intent that new Kookr Grok sessions pick it up.
3. You finished a Kookr “implement issue” / self-continuation unit whose PR
   closed a fork bug that only matters once the installed binary updates
   (attach, alt-screen, hooks, headless session lifecycle, etc.).

**Steps (in order):**

```bash
# 1) Put the *install source tree* on the intended commit.
#    Default GROK_SRC is ~/git/grok-build — worktree builds do NOT install
#    themselves. Either update that checkout, or pass GROK_SRC.
cd ~/git/grok-build
git fetch origin
# Prefer the revision you just shipped (usually origin/main after merge):
git checkout main && git pull --ff-only origin main
# If you must install a feature branch instead:
#   git checkout feat/claude-compat && git pull --ff-only
# Or rebuild from a worktree without moving the primary checkout:
#   GROK_SRC=/path/to/worktree pnpm --dir ~/git/kookr grok:rebuild

# 2) Rebuild + install to ~/bin/grok
cd ~/git/kookr
pnpm grok:rebuild

# 3) Verify the installed SHA matches the source you intended
~/bin/grok --version
# Expect the new commit SHA (not a pre-merge SHA such as the old publish tip).
```

**Then report** in the task summary: installed path, `grok --version` output,
and that **already-running** Grok sessions keep the old process until they are
killed/relaunched (new launches only).

### What not to do

- Do **not** treat `pnpm prod:update` as “deploy the new Grok binary.”
- Do **not** run `pnpm grok:rebuild` while `~/git/grok-build` is still on a
  stale branch/SHA (e.g. old `feat/claude-compat` behind `origin/main`) unless
  you intentionally want that SHA — the script builds whatever is checked out
  at `GROK_SRC`.
- Do **not** stop after “PR merged” for attach/TUI/hook fixes; the merge alone
  does not change `/home/jean/bin/grok`.

### When to also restart Kookr

- **Binary-only fork fix** (this repo): `pnpm grok:rebuild` is enough for *new*
  sessions; no Kookr restart required if `KOOKR_GROK_BIN` already points at
  `~/bin/grok`.
- **Kookr server / adapter / attach pipeline change** (`~/git/kookr`): still use
  `cd ~/git/kookr && pnpm prod:update` (or `prod:restart`) as documented there —
  that path is independent of this binary rebuild.

## Worktrees

Tracked-file edits for Kookr tasks still use a **fresh worktree** off
`origin/main` (not the primary checkout). After that work merges, step (1)
above updates the primary `~/git/grok-build` (or sets `GROK_SRC`) so the
rebuild installs the merged tip — not the pre-merge primary branch.

## Compatibility note

Kookr’s Grok qualification manifest may pin an older reviewed `buildId`/SHA.
Rebuilding still installs the new binary for runtime use; updating the
manifest identity is a separate Kookr-side task when formal re-qualification
is required. Do not skip the binary rebuild because the manifest is stale.
