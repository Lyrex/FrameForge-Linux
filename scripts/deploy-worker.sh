#!/usr/bin/env bash
#
# A wizard — walks a human through a manual procedure step by step.
#
# Everything above the "STAGES" marker is the wizard library: do not hand-edit
# it. Author the per-step stages below the marker.

set -euo pipefail

# ──────────────────────────────────────────────────────────────────────────
# Wizard library — delightful, consistent UX. Identical across every wizard.
# ──────────────────────────────────────────────────────────────────────────

if [[ -t 1 ]] && command -v tput >/dev/null 2>&1 && [[ "$(tput colors 2>/dev/null || echo 0)" -ge 8 ]]; then
  BOLD=$(tput bold); DIM=$(tput dim); RESET=$(tput sgr0)
  BLUE=$(tput setaf 4); GREEN=$(tput setaf 2); YELLOW=$(tput setaf 3); RED=$(tput setaf 1)
else
  BOLD=""; DIM=""; RESET=""; BLUE=""; GREEN=""; YELLOW=""; RED=""
fi

# Author sets this at the top of the stages section.
TOTAL_STAGES=0

_STAGE_INDEX=0
ENV_FILE="${ENV_FILE:-.env}"
WRITTEN_ENV=()    # KEYs written to ENV_FILE this run
WRITTEN_SECRET=() # secret NAMEs set this run
SKIPPED=()        # things we couldn't do (e.g. gh missing)

# _clear — wipe the terminal so only the current step is on screen. No-op when
# output isn't a terminal, so piped logs stay readable.
_clear() {
  [[ -t 1 ]] || return 0
  if command -v tput >/dev/null 2>&1; then tput clear; else printf '\033[2J\033[3J\033[H'; fi
}

# banner "Title" — opening frame: what this wizard does.
banner() {
  _clear
  printf '\n%s%s  %s%s\n' "$BOLD" "$BLUE" "$1" "$RESET"
  printf '%s  %s stages%s\n\n' "$DIM" "$TOTAL_STAGES" "$RESET"
  printf '%s  You drive the browser; this wizard tells you exactly what to do and\n' "$DIM"
  printf '  captures the values you copy back. Stop any time with Ctrl-C and re-run\n'
  printf '  later — it remembers values already saved.%s\n' "$RESET"
  pause "Ready to start?"
}

# stage "Name" — clear the screen, then announce a stage and show progress.
# Clearing keeps only the current step on screen.
stage() {
  _clear
  _STAGE_INDEX=$((_STAGE_INDEX + 1))
  printf '\n%s%s▸ Stage %s/%s · %s%s\n' \
    "$BOLD" "$BLUE" "$_STAGE_INDEX" "$TOTAL_STAGES" "$1" "$RESET"
}

# say "..." — a plain instruction line.
say()  { printf '  %s\n' "$1"; }
# step "..." — a numbered-feeling action the human takes in the browser.
step() { printf '  %s•%s %s\n' "$BLUE" "$RESET" "$1"; }
note() { printf '  %s%s%s\n' "$DIM" "$1" "$RESET"; }
warn() { printf '  %s⚠ %s%s\n' "$YELLOW" "$1" "$RESET"; }

# open_url URL — open in the human's browser, cross-platform incl. WSL.
open_url() {
  local url="$1"
  printf '  %s↗ opening%s %s\n' "$GREEN" "$RESET" "$url"
  { if   command -v wslview     >/dev/null 2>&1; then wslview "$url"
    elif command -v explorer.exe >/dev/null 2>&1; then explorer.exe "$url"
    elif command -v xdg-open    >/dev/null 2>&1; then xdg-open "$url"
    elif command -v open        >/dev/null 2>&1; then open "$url"
    else warn "couldn't open a browser — visit it manually: $url"; fi
  } >/dev/null 2>&1 || warn "couldn't open a browser — visit it manually: $url"
}

# pause "msg" — wait for the human to confirm they've done the manual part.
pause() {
  printf '  %s%s%s ' "$DIM" "${1:-Press Enter to continue}" "$RESET"
  read -r _ || true
}

# confirm "question" — y/N gate; returns success on yes.
confirm() {
  local reply=""
  printf '  %s? %s [y/N] ' "$YELLOW" "$1"
  read -r reply || true
  [[ "$reply" =~ ^[Yy] ]]
}

# _existing KEY — current value of KEY in ENV_FILE, if any.
_existing() {
  [[ -f "$ENV_FILE" ]] || return 1
  local line; line=$(grep -E "^${1}=" "$ENV_FILE" | tail -n1) || return 1
  printf '%s' "${line#*=}"
}

# ask KEY "Prompt" — read a value into $KEY. Offers the existing .env value as
# a default on re-runs (Enter keeps it). Visible input (non-secret).
ask() {
  local key="$1" prompt="$2" current input
  current=$(_existing "$key" || true)
  if [[ -n "$current" ]]; then
    printf '  %s%s%s %s[Enter keeps current]%s ' "$BOLD" "$prompt" "$RESET" "$DIM" "$RESET"
  else
    printf '  %s%s%s ' "$BOLD" "$prompt" "$RESET"
  fi
  read -r input || true
  [[ -z "$input" && -n "$current" ]] && input="$current"
  printf -v "$key" '%s' "$input"
}

# ask_secret KEY "Prompt" — like ask, but input is hidden.
ask_secret() {
  local key="$1" prompt="$2" current input
  current=$(_existing "$key" || true)
  if [[ -n "$current" ]]; then
    printf '  %s%s%s %s[Enter keeps current]%s ' "$BOLD" "$prompt" "$RESET" "$DIM" "$RESET"
  else
    printf '  %s%s%s ' "$BOLD" "$prompt" "$RESET"
  fi
  read -rs input || true
  printf '\n'
  [[ -z "$input" && -n "$current" ]] && input="$current"
  printf -v "$key" '%s' "$input"
}

# write_env KEY VALUE — upsert KEY=VALUE into ENV_FILE (creates it; replaces
# any existing line). Idempotent.
write_env() {
  local key="$1" value="$2" tmp
  touch "$ENV_FILE"
  tmp=$(mktemp)
  grep -vE "^${key}=" "$ENV_FILE" > "$tmp" || true
  printf '%s=%s\n' "$key" "$value" >> "$tmp"
  mv "$tmp" "$ENV_FILE"
  WRITTEN_ENV+=("$key")
  printf '  %s✓ wrote%s %s → %s\n' "$GREEN" "$RESET" "$key" "$ENV_FILE"
}

# set_secret NAME VALUE — set a GitHub Actions repo secret via gh. Falls back
# to a warning (and records it) if gh is unavailable or unauthenticated.
set_secret() {
  local name="$1" value="$2"
  if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    if printf '%s' "$value" | gh secret set "$name" >/dev/null 2>&1; then
      WRITTEN_SECRET+=("$name")
      printf '  %s✓ set%s GitHub secret %s\n' "$GREEN" "$RESET" "$name"
      return
    fi
  fi
  SKIPPED+=("GitHub secret $name (set it manually: gh secret set $name)")
  warn "skipped GitHub secret $name — gh not ready; set it later"
}

# set_var NAME VALUE — set a GitHub Actions repo variable (non-secret).
set_var() {
  local name="$1" value="$2"
  if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    if gh variable set "$name" --body "$value" >/dev/null 2>&1; then
      printf '  %s✓ set%s GitHub variable %s\n' "$GREEN" "$RESET" "$name"
      return
    fi
  fi
  SKIPPED+=("GitHub variable $name")
  warn "skipped GitHub variable $name — gh not ready; set it later"
}

# finish — clear, then a closing summary of everything configured.
finish() {
  _clear
  printf '\n%s%s  ✓ Setup complete%s\n' "$BOLD" "$GREEN" "$RESET"
  (( ${#WRITTEN_ENV[@]} ))    && note "wrote ${#WRITTEN_ENV[@]} value(s) to $ENV_FILE: ${WRITTEN_ENV[*]}"
  (( ${#WRITTEN_SECRET[@]} )) && note "set ${#WRITTEN_SECRET[@]} GitHub secret(s): ${WRITTEN_SECRET[*]}"
  if (( ${#SKIPPED[@]} )); then
    printf '\n'; warn "still to do by hand:"
    for s in "${SKIPPED[@]}"; do note "  - $s"; done
  fi
  printf '\n'
}

# ──────────────────────────────────────────────────────────────────────────
# STAGES
# ──────────────────────────────────────────────────────────────────────────

TOTAL_STAGES=6

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKER_DIR="$REPO_ROOT/worker"
CONFIG="$WORKER_DIR/wrangler.jsonc"
CLIENT="$REPO_ROOT/src-tauri/src/worker.rs"

# The id `wrangler.jsonc` ships with. Anything else is a real namespace, so the
# create stage knows it has already run.
PLACEHOLDER_KV_ID="00000000000000000000000000000000"

die() {
  printf '\n  %s✗ %s%s\n\n' "$RED" "$1" "$RESET" >&2
  exit 1
}

# Every command that touches the Cloudflare account is printed before it runs:
# the human is accountable for what happens to their account, so nothing
# happens to it that they did not see first.
run() {
  printf '  %s$ %s%s\n' "$DIM" "$*" "$RESET"
  "$@"
}

wrangler() { (cd "$WORKER_DIR" && run npx --no-install wrangler "$@"); }

banner "Deploy the FrameForge cache worker"

# ── 1 ─────────────────────────────────────────────────────────────────────
stage "Prerequisites and Cloudflare login"
say "Checking the tools this wizard drives."

command -v npm >/dev/null 2>&1 || die "npm not found — install Node.js first."
command -v curl >/dev/null 2>&1 || die "curl not found — the health check needs it."
[[ -f "$CONFIG" ]] || die "no $CONFIG — run this from a FrameForge checkout."
[[ -d "$WORKER_DIR/node_modules" ]] || {
  say "The worker's dependencies are not installed yet."
  (cd "$WORKER_DIR" && run npm install) || die "npm install failed in $WORKER_DIR."
}
note "npm, curl and the worker's dependencies are present."

if wrangler whoami >/dev/null 2>&1; then
  note "wrangler is already authenticated."
else
  say "wrangler is not logged in. It will open Cloudflare in your browser."
  step "Approve the access request on the page that opens, then come back here."
  wrangler login || die "wrangler login failed — nothing has been changed."
fi
wrangler whoami || die "still not authenticated; re-run once login succeeds."
pause "Account above is the one to deploy to?"

# ── 2 ─────────────────────────────────────────────────────────────────────
stage "KV namespace for the price snapshot"
say "The snapshot document and the prewarm cursor live in a KV namespace."

current_kv=$(grep -o '"id": "[0-9a-f]\{32\}"' "$CONFIG" | head -n1 | grep -o '[0-9a-f]\{32\}')
if [[ "$current_kv" != "$PLACEHOLDER_KV_ID" ]]; then
  note "already created: $current_kv"
else
  create_log=$(mktemp)
  wrangler kv namespace create SNAPSHOT | tee "$create_log" || die "namespace creation failed."
  kv_id=$(grep -o '[0-9a-f]\{32\}' "$create_log" | head -n1)
  rm -f "$create_log"
  [[ -n "$kv_id" ]] || die "could not read the new namespace id out of wrangler's output."
  # In place of the placeholder, so a re-run of this wizard sees it and a
  # deploy binds the real namespace.
  sed -i "s/$PLACEHOLDER_KV_ID/$kv_id/" "$CONFIG" || die "could not write the id into $CONFIG."
  printf '  %s✓ wrote%s namespace %s into wrangler.jsonc\n' "$GREEN" "$RESET" "$kv_id"
  warn "commit that change — a fresh checkout still carries the placeholder."
fi
pause

# ── 3 ─────────────────────────────────────────────────────────────────────
stage "Daily request budget"
say "Past this many requests in a UTC day the worker stands down and clients"
say "go back to fetching the upstreams directly, until the next UTC midnight."

current_budget=$(grep -o '"DAILY_REQUEST_BUDGET": [0-9]*' "$CONFIG" | grep -o '[0-9]*')
note "currently $current_budget requests/day"
printf '  %sNew value, or Enter to keep it:%s ' "$BOLD" "$RESET"
read -r new_budget || true
if [[ -n "$new_budget" ]]; then
  [[ "$new_budget" =~ ^[0-9]+$ ]] || die "'$new_budget' is not a number."
  sed -i "s/\"DAILY_REQUEST_BUDGET\": [0-9]*/\"DAILY_REQUEST_BUDGET\": $new_budget/" "$CONFIG" ||
    die "could not write the budget into $CONFIG."
  printf '  %s✓ set%s DAILY_REQUEST_BUDGET to %s\n' "$GREEN" "$RESET" "$new_budget"
fi
note "later changes need no redeploy: Workers → frameforge-cache → Settings →"
note "Variables and Secrets, edit DAILY_REQUEST_BUDGET. That rolls a new version"
note "of the same code. Keep this file in step, or the next deploy reverts it."
pause

# ── 4 ─────────────────────────────────────────────────────────────────────
stage "Deploy"
say "This uploads the worker, applies the Durable Object migration that creates"
say "the daily budget counter, and starts the five-minute prewarm cron."
grep -q '"new_sqlite_classes"' "$CONFIG" || die "the DailyBudget migration is missing from $CONFIG."

confirm "Deploy to Cloudflare now?" || die "nothing deployed."
deploy_log=$(mktemp)
wrangler deploy | tee "$deploy_log" || { rm -f "$deploy_log"; die "deploy failed — see the output above."; }
host=$(grep -o 'https://[a-zA-Z0-9.-]*workers\.dev' "$deploy_log" | head -n1)
rm -f "$deploy_log"
[[ -n "$host" ]] || {
  warn "could not read the deployed hostname out of wrangler's output."
  ask host "Paste the worker's URL (https://…):"
}
pause

# ── 5 ─────────────────────────────────────────────────────────────────────
stage "Health check"
say "Asking the deployed worker whether it is alive."
printf '  %s$ curl %s/v1/health%s\n' "$DIM" "$host" "$RESET"
health=$(curl --silent --show-error --fail --max-time 15 "$host/v1/health") ||
  die "$host/v1/health did not answer. DNS can take a minute on a first deploy; re-run."
[[ "$health" == *'"ok"'* ]] || die "unexpected answer from /v1/health: $health"
printf '  %s✓ live%s %s\n' "$GREEN" "$RESET" "$host"
pause

# ── 6 ─────────────────────────────────────────────────────────────────────
stage "Point the app at it"
say "The app ships one default worker URL. It has to name the host above."
shipped=$(grep -o 'https://[a-zA-Z0-9.-]*workers\.dev' "$CLIENT" | head -n1)
if [[ "$shipped" == "$host" ]]; then
  note "DEFAULT_BASE_URL already matches: $shipped"
else
  warn "they differ — this wizard does not edit application code:"
  note "  shipped:  $shipped"
  note "  deployed: $host"
  step "Set DEFAULT_BASE_URL in src-tauri/src/worker.rs to $host and commit it."
  SKIPPED+=("point DEFAULT_BASE_URL in src-tauri/src/worker.rs at $host")
fi

finish
