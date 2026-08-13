# Why Kookr-managed Grok sessions do not run the operator's Claude hooks

**Date:** 2026-08-13
**Verdict:** Working as designed in the fork; the drop is an **intentional Kookr
decision**. The owning layer is `kookr-ai/kookr`, not `jeanibarz/grok-build`.

## TL;DR

The Grok Build fork (`feat/claude-compat`) **does** implement Claude hook
compatibility and **would** load the operator's `~/.claude/settings.json` hooks
under Kookr's isolated `GROK_HOME` + real `HOME`. It does not, because Kookr's
adapter **deliberately sets `GROK_CLAUDE_HOOKS_ENABLED=0`** in the child
environment. The fork faithfully honors that toggle by resolving
`compat.claude.hooks = false`, which makes hook discovery skip the
`~/.claude/settings.json` global source entirely.

No fork bug. The product gap ("operator Claude hooks never reach a managed Grok
session") lives in Kookr and is filed as an issue for forwarding.

## What Grok is supposed to load

`discover_hook_source_paths` assembles global + project hook sources. When Claude
hook compat is on it appends the operator's global Claude settings:

- `crates/codegen/xai-grok-shell/src/util/hooks.rs:80-84` — pushes
  `~/.claude/settings.json` and `~/.claude/settings.local.json` onto the global
  source list (home from `dirs::home_dir()` at `hooks.rs:52`, i.e. the real
  `HOME`, independent of `GROK_HOME`).
- `crates/codegen/xai-grok-shell/src/util/hooks.rs:108-116` — `discover_hooks`,
  the single load entry point used by every session-start / reload site
  (`session/acp_session_impl/spawn.rs:1142`,
  `session/acp_session_impl/hooks_plugins.rs:640`,
  `agent/mvp_agent/agent_ops.rs:3628`).
- Matcher aliasing is real: a Claude `"matcher": "Bash"` matches Grok's
  `run_terminal_command` (`crates/codegen/xai-grok-hooks/src/matcher.rs:187-189`;
  `crates/codegen/xai-grok-hooks/src/discovery.rs:961`).
- The Claude-compat cell defaults ON
  (`crates/codegen/xai-grok-tools/src/types/compat.rs:319-330`).

So under Kookr's launch shape (real `HOME`, isolated empty `GROK_HOME`, no
`[claude_compat] imported` marker in `$GROK_HOME/config.toml`), the fork's
discovery would include and load `~/.claude/settings.json`. Verified live: the
operator's `~/.grok/config.toml` has **no** import marker, and
`~/.claude/settings.json` registers `clear-writing-nudge.sh` on `SessionStart`,
`UserPromptSubmit`, and `PreToolUse(Bash)` — all events/matchers the fork loads.

## The gate that turns it off

`include_claude_hooks` is the single gate:

- `crates/codegen/xai-grok-shell/src/util/hooks.rs:36-39`
  ```rust
  fn include_claude_hooks(compat) -> bool {
      compat.claude.hooks
          && !is_claude_import_marked_with_log("discover_hook_source_paths")
  }
  ```
- `compat.claude.hooks` is resolved with precedence **env → TOML → remote →
  default-ON**:
  - `crates/codegen/xai-grok-tools/src/types/compat.rs:187-192` registers the
    `claude/hooks` cell against the env var **`GROK_CLAUDE_HOOKS_ENABLED`**.
  - `crates/codegen/xai-grok-shell/src/agent/config.rs:718-741`
    (`resolve_compat_cell` / `resolve_compat_cell_with_env`) — env override wins.
  - `crates/codegen/xai-grok-config/src/lib.rs:74-82` (`env_bool`) — `"0"`,
    `"false"`, `"no"`, `"off"`, `"disabled"` → `Some(false)`.

So `GROK_CLAUDE_HOOKS_ENABLED=0` ⇒ `compat.claude.hooks = false` ⇒
`include_claude_hooks` false ⇒ `hooks.rs:80-84` never appends the
`~/.claude/settings.json` source ⇒ operator Claude hooks are never loaded or
matched.

## Where Kookr drops it (the guilty line)

- **`src/adapters/grok-launch-args.ts:141`**
  ```js
  env.GROK_CLAUDE_HOOKS_ENABLED = '0';
  ```
  with the rationale at `grok-launch-args.ts:135-140`: Kookr's monitoring hook is
  its own native instrumentation under the isolated `GROK_HOME`, and the operator's
  global Claude hooks "can run serially before Kookr's UserPromptSubmit
  acknowledgement, stranding launch confirmation behind the adapter's timeout."
- The env reaches the child because the launch uses `envMode: 'replace'`
  (`src/adapters/grok-build-adapter.ts:386`) over an allowlist that keeps real
  `HOME` (`src/adapters/grok-launch-args.ts:69`) but then sets the toggle to `0`.
- The composer keeps real `HOME` and writes no `config.toml`
  (`src/adapters/grok-home-composer.ts:22-24,114-139`) — so `HOME` and the
  missing import-marker are **not** the cause; the explicit toggle is.

This is intentional, not accidental: the value is hard-coded with a comment
explaining the launch-timeout tradeoff.

## The product gap

Operator Claude hooks (writing nudges, KB-scout, gates, `~/.claude/hooks/*.sh`)
**never** run in a managed Grok session, because the toggle is unconditional and
global. The tradeoff Kookr documents (avoid *blocking* hooks stalling the
UserPromptSubmit ack) is real, but the current fix is a blunt all-or-nothing
disable rather than forwarding non-blocking operator hooks.

## Reproduction

Rust characterization tests added on branch `fix/claude-user-hooks` in
`crates/codegen/xai-grok-shell/src/util/hooks.rs` (module `claude_user_hook_tests`):

```
cd <grok-build worktree on fix/claude-user-hooks>
cargo test -p xai-grok-shell --lib claude_user_hook_tests -- --nocapture
```

- `claude_compat_on_loads_and_matches_user_hooks` — proves the fork discovers,
  loads, and Bash→`run_terminal_command`-aliases `~/.claude/settings.json` under
  the Kookr-shaped env when compat is ON (the fork is correct).
- `claude_compat_off_drops_user_hooks` — proves `GROK_CLAUDE_HOOKS_ENABLED=0`
  resolves `claude.hooks=false` and drops the `~/.claude/settings.json` source
  (the exact Kookr behavior).

Dependency-free cross-check:

```
grep -n "GROK_CLAUDE_HOOKS_ENABLED" ~/git/kookr/src/adapters/grok-launch-args.ts
grep -n "GROK_CLAUDE_HOOKS_ENABLED" ~/git/grok-build/crates/codegen/xai-grok-tools/src/types/compat.rs
grep -n "include_claude" ~/git/grok-build/crates/codegen/xai-grok-shell/src/util/hooks.rs
```

## Recommendation (for the Kookr issue)

Forward the operator's user-scoped Claude hooks into managed Grok sessions
without reintroducing the launch-ack stall — e.g. compose the operator's
`~/.claude/settings.json` hooks into the isolated `$GROK_HOME` (or stop forcing
`GROK_CLAUDE_HOOKS_ENABLED=0` and instead gate only the events that block the
UserPromptSubmit ack). Do **not** copy hook `.sh` scripts per-session; keep it a
source-forwarding change. No fork change required.
