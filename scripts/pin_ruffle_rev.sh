#!/usr/bin/env bash
set -euo pipefail

LOCK="rust/bridge/Cargo.lock"
TOML="rust/bridge/Cargo.toml"

if [[ ! -f "$LOCK" ]]; then
  echo "ERROR: $LOCK not found. Run 'make' once (or 'cargo generate-lockfile' in rust/bridge) first." >&2
  exit 1
fi

# Extract the first Ruffle git rev from Cargo.lock.
# Cargo encodes it like: git+https://github.com/ruffle-rs/ruffle?...#<sha>
REV=$(grep -oE 'git\+https://github\.com/ruffle-rs/ruffle[^#]*#[0-9a-f]{7,40}' "$LOCK" | head -n1 | sed 's/.*#//')

if [[ -z "${REV:-}" ]]; then
  echo "ERROR: Could not find a ruffle-rs/ruffle git source in $LOCK." >&2
  echo "       Are you using a different git URL?" >&2
  exit 1
fi

echo "Detected Ruffle rev: $REV"

# Refuse to double-insert.
if grep -q 'git = "https://github.com/ruffle-rs/ruffle".*rev =' "$TOML"; then
  echo "Cargo.toml already contains a pinned rev. Nothing to do." >&2
  exit 0
fi

# Insert `rev = "..."` next to each `git = "https://github.com/ruffle-rs/ruffle"`.
# Works for { git = "...", ... } dependency tables.
cp "$TOML" "$TOML.bak"

# GNU sed on MSYS supports -i.
sed -i "s|git = \"https://github.com/ruffle-rs/ruffle\"|git = \"https://github.com/ruffle-rs/ruffle\", rev = \"$REV\"|g" "$TOML"

echo "Pinned Ruffle git deps in $TOML (backup: $TOML.bak)"
echo "Tip: commit Cargo.lock + Cargo.toml together for stability."
