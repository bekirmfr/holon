#!/usr/bin/env bash
# The launch repository's check: no file meant only for the development phase goes out.
#
# Design notes and plans may be committed with the dev-phase-only marker (below) in their text
# (the owner's decision, 2026-11-15); the launch repository is a fresh one cut without them. This
# lists every file in a tree that holds the marker, and fails when there is any, so a tree meant
# for the launch repository is checked before it is pushed.
#
#   .github/launch-check.sh [TREE]         fail (exit 1) if any file in TREE holds the marker
#   .github/launch-check.sh --list [TREE]  list them and exit 0: what the cut leaves out
#
# TREE defaults to the current folder. In a git checkout only tracked files are read (git grep),
# else every file under TREE. Exit 2 when TREE isn't a folder. It runs nowhere yet: it is for the
# day the launch repository is cut.
set -euo pipefail

list=0
if [ "${1:-}" = "--list" ]; then list=1; shift; fi
tree="${1:-.}"
if [ ! -d "$tree" ]; then echo "launch-check: no folder $tree" >&2; exit 2; fi

# Built from two halves, so this script doesn't hold the marker it looks for.
marker="holon:dev-phase""-only"

if git -C "$tree" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  found=$(git -C "$tree" grep -l -F -e "$marker" -- . || true)
else
  found=$(cd "$tree" && grep -r -l -F -e "$marker" . || true)
fi

if [ "$list" = 1 ]; then
  if [ -n "$found" ]; then printf '%s\n' "$found"; fi
  exit 0
fi
if [ -n "$found" ]; then
  n=$(printf '%s\n' "$found" | wc -l | tr -d ' ')
  echo "launch-check: $n file(s) marked $marker, not for the launch repository:" >&2
  printf '%s\n' "$found" | sed 's/^/  /' >&2
  exit 1
fi
echo "launch-check: no file in $tree is marked $marker"
