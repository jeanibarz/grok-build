#!/usr/bin/env bash
# =============================================================================
# Issue #1 end-to-end evidence: headless `-p` must emit an end-of-life event.
# =============================================================================
# Proves the fix and its cross-CLI parity in one run:
#
#   1. BEFORE/AFTER control — the released grok 0.2.101 (unpatched) vs the
#      patched build, same isolated-home hook-capture, same trivial headless
#      turn. The released binary must MISS `session_end` for the parent;
#      the patched build must EMIT it. This is the direct fix proof.
#
#   2. THREE-WAY parity — patched grok vs Claude Code (`claude -p`) vs Codex
#      (`codex exec`), each with an equivalent end-of-life capture hook. All
#      three must fire their end-of-life event (grok/codex `session_end`,
#      claude `SessionEnd`) so the patched headless run matches the documented
#      lifecycle contract the other two CLIs already honor.
#
# Isolation: every CLI runs against a synthetic HOME with only a capture hook
# installed; auth is seeded read-only from the operator's real home when
# present. No secret is printed or written into the evidence dir. Auto-update
# and workspace data collection are disabled for grok.
#
# Usage:
#   ./issue-1-session-end-e2e.sh --patched /abs/path/to/grok [--out DIR]
#
# Exit 0 = harness ran; verdicts live in $OUT/summary.json and stdout.
# Nonzero = the harness itself failed to run (not a gate failure).
set -uo pipefail

PATCHED_GROK=""
RELEASED_GROK="${RELEASED_GROK:-$HOME/.grok/bin/grok}"
CLAUDE_BIN="${CLAUDE_BIN:-claude}"
CODEX_BIN="${CODEX_BIN:-$HOME/bin/codex}"
MODEL="${KOOKR_GROK_MODEL:-grok-build}"
TURN_TIMEOUT="${TURN_TIMEOUT:-180}"
OUT=""
PROMPT='Reply with exactly the single word: PONG'

while [ $# -gt 0 ]; do
  case "$1" in
    --patched) PATCHED_GROK="$2"; shift 2;;
    --released) RELEASED_GROK="$2"; shift 2;;
    --out) OUT="$2"; shift 2;;
    --model) MODEL="$2"; shift 2;;
    *) echo "unknown arg: $1" >&2; exit 2;;
  esac
done
[ -n "$PATCHED_GROK" ] || { echo "ERROR: --patched /path/to/grok is required" >&2; exit 2; }
[ -x "$PATCHED_GROK" ] || { echo "ERROR: patched grok not executable: $PATCHED_GROK" >&2; exit 2; }
[ -n "$OUT" ] || OUT="$(mktemp -d "${TMPDIR:-/tmp}/issue1-e2e.XXXXXX")"
mkdir -p "$OUT"; chmod 700 "$OUT"

ts() { date -u +%FT%TZ; }
log() { printf '[%s] %s\n' "$(ts)" "$*"; }

# capture.sh <cli> <event>: append one ordered JSONL record. The event name is
# baked into each hook command so we never depend on per-CLI stdin/env shapes.
CAP="$OUT/capture.sh"
cat > "$CAP" <<'EOS'
#!/usr/bin/env bash
# args: <cli> <event>. CAPFILE + a nanosecond clock give a total order.
printf '{"cli":"%s","event":"%s","ns":%s}\n' "$1" "$2" "$(date +%s%N)" >> "$CAPFILE"
# drain stdin so the child never blocks on a full pipe
cat >/dev/null 2>&1 || true
EOS
chmod +x "$CAP"

EVENTS=(SessionStart UserPromptSubmit PreToolUse PostToolUse Stop Notification SubagentStart SubagentStop SessionEnd)

# ---- grok run (isolated GROK_HOME hook config) -----------------------------
run_grok() {  # run_grok <label> <binary>
  local label="$1" bin="$2"
  local home="$OUT/$label-home" gh cap
  gh="$home/.grok"; cap="$OUT/$label.jsonl"; : > "$cap"
  mkdir -p "$gh/hooks"; chmod -R 700 "$home"
  python3 - "$gh/hooks" "$CAP" "$label" <<'PY'
import json,sys,os
hd,cap,label=sys.argv[1:4]
events=["SessionStart","UserPromptSubmit","PreToolUse","PostToolUse","Stop",
        "Notification","SubagentStart","SubagentStop","SessionEnd"]
cfg={"hooks":{e:[{"hooks":[{"type":"command","command":f"{cap} {label} {e}","timeout":10}]}] for e in events}}
open(os.path.join(hd,"capture.json"),"w").write(json.dumps(cfg,indent=2))
PY
  # seed auth read-only if the operator has it
  [ -f "$HOME/.grok/auth.json" ] && { cp "$HOME/.grok/auth.json" "$gh/auth.json"; chmod 600 "$gh/auth.json"; }
  [ -f "$HOME/.grok/auth.json.lock" ] && cp "$HOME/.grok/auth.json.lock" "$gh/auth.json.lock" 2>/dev/null
  local rc
  ( cd "$OUT" && timeout "$TURN_TIMEOUT" env -u HOME HOME="$home" GROK_HOME="$gh" \
      CAPFILE="$cap" GROK_DISABLE_AUTOUPDATER=1 GROK_AUTO_UPDATE=0 \
      GROK_WORKSPACE_DATA_COLLECTION_DISABLED=1 GROK_FOLDER_TRUST=1 TERM=xterm-256color \
      "$bin" --model "$MODEL" -p "$PROMPT" ) > "$OUT/$label.out" 2>&1
  rc=$?
  echo "$rc" > "$OUT/$label.rc"
  log "grok[$label] rc=$rc events=[$(seq_of "$cap")]"
}

# ---- claude run (isolated HOME + --settings hook config) -------------------
run_claude() {
  local home="$OUT/claude-home" cap="$OUT/claude.jsonl" set="$OUT/claude-settings.json"
  : > "$cap"; mkdir -p "$home/.claude"; chmod -R 700 "$home"
  # Seed the operator's OAuth credentials read-only so the turn actually runs;
  # a minimal ~/.claude.json skips onboarding. Neither is printed or committed.
  [ -f "$HOME/.claude/.credentials.json" ] && { cp "$HOME/.claude/.credentials.json" "$home/.claude/.credentials.json"; chmod 600 "$home/.claude/.credentials.json"; }
  printf '{"hasCompletedOnboarding":true,"bypassPermissionsModeAccepted":true}\n' > "$home/.claude.json"
  python3 - "$set" "$CAP" <<'PY'
import json,sys
setf,cap=sys.argv[1:3]
events=["SessionStart","UserPromptSubmit","PreToolUse","PostToolUse","Stop",
        "Notification","SubagentStop","SessionEnd"]
cfg={"hooks":{e:[{"hooks":[{"type":"command","command":f"{cap} claude {e}"}]}] for e in events}}
open(setf,"w").write(json.dumps(cfg,indent=2))
PY
  local rc
  ( cd "$OUT" && timeout "$TURN_TIMEOUT" env -u HOME HOME="$home" CAPFILE="$cap" \
      "$CLAUDE_BIN" -p "$PROMPT" --settings "$set" ) > "$OUT/claude.out" 2>&1
  rc=$?; echo "$rc" > "$OUT/claude.rc"
  log "claude rc=$rc events=[$(seq_of "$cap")]"
}

# ---- codex run (isolated HOME + --hooks config) ----------------------------
run_codex() {
  local home="$OUT/codex-home" ch cap="$OUT/codex.jsonl" hf="$OUT/codex-settings.json"
  ch="$home/.codex"
  : > "$cap"; mkdir -p "$ch"; chmod -R 700 "$home"
  [ -f "$HOME/.codex/auth.json" ] && { cp "$HOME/.codex/auth.json" "$ch/auth.json"; chmod 600 "$ch/auth.json"; }
  python3 - "$hf" "$CAP" <<'PY'
import json,sys
hf,cap=sys.argv[1:3]
events=["SessionStart","UserPromptSubmit","PreToolUse","PostToolUse",
        "Notification","SubagentStart","SubagentStop","SessionEnd"]
cfg={"hooks":{e:[{"hooks":[{"type":"command","command":f"{cap} codex {e}"}]}] for e in events}}
open(hf,"w").write(json.dumps(cfg,indent=2))
PY
  # codex exec: --settings injects hooks; a scratch git repo satisfies exec's
  # repo check; read-only sandbox — the PONG prompt needs no tools.
  local wd="$OUT/codex-cwd"; mkdir -p "$wd"; ( cd "$wd" && git init -q 2>/dev/null )
  local rc
  ( cd "$wd" && timeout "$TURN_TIMEOUT" env -u HOME HOME="$home" CODEX_HOME="$ch" CAPFILE="$cap" \
      "$CODEX_BIN" exec --dangerously-bypass-hook-trust --settings "$hf" \
      -s read-only "$PROMPT" ) > "$OUT/codex.out" 2>&1
  rc=$?; echo "$rc" > "$OUT/codex.rc"
  log "codex rc=$rc events=[$(seq_of "$cap")]"
}

seq_of() {  # ordered, comma-joined event names from a capture file
  python3 -c "
import json,sys
try: rows=[json.loads(l) for l in open(sys.argv[1]) if l.strip()]
except FileNotFoundError: rows=[]
rows.sort(key=lambda r: r['ns'])
print(','.join(r['event'] for r in rows))" "$1" 2>/dev/null
}
has_end() {  # 1 if the CLI's end-of-life event is present, else 0
  python3 -c "
import json,sys
try: rows=[json.loads(l) for l in open(sys.argv[1]) if l.strip()]
except FileNotFoundError: rows=[]
print(1 if any(r['event']=='SessionEnd' for r in rows) else 0)" "$1" 2>/dev/null
}

# =============================================================================
log "issue-1 e2e start; OUT=$OUT"
log "patched=$PATCHED_GROK released=$RELEASED_GROK model=$MODEL"

[ -x "$RELEASED_GROK" ] && run_grok released "$RELEASED_GROK" || log "released grok not available; skipping before/after control"
run_grok patched "$PATCHED_GROK"
command -v "$CLAUDE_BIN" >/dev/null 2>&1 && run_claude || log "claude not on PATH; skipping"
[ -x "$CODEX_BIN" ] && run_codex || log "codex not available; skipping"

# ---- summary ---------------------------------------------------------------
python3 - "$OUT" <<'PY'
import json,os,sys
out=sys.argv[1]
def load(l):
    p=os.path.join(out,f"{l}.jsonl")
    try: rows=[json.loads(x) for x in open(p) if x.strip()]
    except FileNotFoundError: return None
    rows.sort(key=lambda r:r['ns'])
    return rows
def rc(l):
    try: return int(open(os.path.join(out,f"{l}.rc")).read().strip())
    except Exception: return None
res={}
for label in ("released","patched","claude","codex"):
    rows=load(label)
    if rows is None: continue
    seq=[r['event'] for r in rows]
    res[label]={"rc":rc(label),"sequence":seq,
                "session_end":("SessionEnd" in seq),"event_count":len(seq)}
json.dump(res,open(os.path.join(out,"summary.json"),"w"),indent=2)

print("\n================ issue #1 end-of-life evidence ================")
for label in ("released","patched","claude","codex"):
    if label not in res:
        print(f"{label:9} : (skipped)"); continue
    r=res[label]
    mark="EMITS session_end" if r["session_end"] else "MISSING session_end"
    print(f"{label:9} : rc={r['rc']!s:>4}  {mark:20}  seq={','.join(r['sequence']) or '(none)'}")

print("\n---- verdicts ----")
rel=res.get("released"); pat=res.get("patched")
if rel is not None:
    ok = (not rel["session_end"]) and (pat and pat["session_end"])
    print(f"before/after   : {'PASS' if ok else 'CHECK'} "
          f"(released emits={rel['session_end']}, patched emits={pat and pat['session_end']})")
else:
    print("before/after   : (no released control)")
if pat is not None:
    print(f"patched fix    : {'PASS' if pat['session_end'] else 'FAIL'} (patched grok emits session_end headless)")
parity=[l for l in ('patched','claude','codex') if l in res]
allok=all(res[l]['session_end'] for l in parity)
detail=', '.join('%s=%s' % (l, res[l]['session_end']) for l in parity)
print('three-way parity: %s (%s)' % ('PASS' if allok else 'CHECK', detail))
PY

log "issue-1 e2e done. Evidence in $OUT"
echo "OUT=$OUT"
