#!/usr/bin/env bash
# What a change touches, decided by the files it changes, for the jobs of the workflows that
# skip work a change can't affect (ci.yml, heavy.yml). The rules are .github/file-rules.txt;
# this file only applies them. It prints each changed file and the decisions, and writes the
# decisions to $GITHUB_OUTPUT when that is set:
#
#   code=true|false     a file that isn't a document ([inert], added or edited, not deleted)
#   rust=true|false     a file that is neither a document nor the editor's ([editor]): the Rust
#                       tests run
#   grids=true|false    the heavy grids run (a file under [grids], or the `heavy` label)
#   records=true|false  the heavy records sweep runs (a file under [records], or the label)
#
# What it compares: on a pull request, the merge commit the checkout made with its first parent
# (the base). code and rust are decided only there: on a push (to main) both are true. The heavy
# groups are decided on a push too, the commit before the push ($BEFORE) with HEAD. Anything
# else (the nightly run, a run by hand), or a comparison that can't be made, turns everything on.
# A path no section names is code.
#
# By hand: `bash .github/what-changed.sh --files < list`, each line of the list as
# `git diff --name-status --no-renames` writes it (a status, a tab, a path), decided as a pull
# request's change would be.
set -u
here=$(cd "$(dirname "$0")" && pwd)
rules="$here/file-rules.txt"
[ -f "$rules" ] || { echo "no $rules" >&2; exit 1; }

all=0
list=""
pull=1
if [ "${1:-}" = "--files" ]; then
  list=$(cat)
else
  case "${GITHUB_EVENT_NAME:-}" in
    pull_request)
      if git rev-parse -q --verify "HEAD^2" >/dev/null; then
        list=$(git diff --name-status --no-renames "HEAD^1" HEAD) || all=1
      else
        echo "HEAD is not the merge commit of a pull request: running everything"
        all=1
      fi
      ;;
    push)
      pull=0
      before="${BEFORE:-}"
      if [ -n "$before" ] && [ -n "${before//0/}" ] && git cat-file -e "$before^{commit}" 2>/dev/null; then
        list=$(git diff --name-status --no-renames "$before" HEAD) || all=1
      else
        echo "no commit before this push to compare with: running everything"
        all=1
      fi
      ;;
    *)
      all=1
      ;;
  esac
fi
# An empty change is a comparison that went wrong more likely than a change of nothing.
[ -z "$list" ] && all=1

if [ "$all" = 1 ]; then
  decided="code=true
rust=true
grids=true
records=true"
else
  decided=$(printf '%s\n' "$list" | awk -F '\t' -v rules="$rules" '
    BEGIN {
      sec = ""
      while ((getline line < rules) > 0) {
        sub(/\r$/, "", line)
        sub(/#.*/, "", line)
        gsub(/^[ \t]+|[ \t]+$/, "", line)
        if (line == "") continue
        if (line ~ /^\[.*\]$/) { sec = substr(line, 2, length(line) - 2); continue }
        n[sec]++
        entry[sec, n[sec]] = line
      }
      code = 0; rust = 0; grids = 0; records = 0
    }
    # The length of the longest entry of section s that holds path p (0 if none): an entry
    # ending in / holds everything under it, any other entry the one file it names.
    function best(s, p,    i, e, b) {
      b = 0
      for (i = 1; i <= n[s]; i++) {
        e = entry[s, i]
        if ((substr(e, length(e)) == "/" && index(p, e) == 1) || p == e) {
          if (length(e) > b) b = length(e)
        }
      }
      return b
    }
    # In a group: its longest entry is longer than the longest of the group'"'"'s -not section.
    function within(g, p) { return best(g, p) > best(g "-not", p) }
    NF >= 2 {
      path = $NF
      said = ""
      # A document is inert only added or edited: deleted, a page something cites may be gone.
      if ($1 ~ /^[AM]/ && within("inert", path)) {
        said = " a document"
      } else if (within("editor", path)) {
        code = 1; said = " the editor"
      } else {
        code = 1; rust = 1; said = " code"
      }
      if (within("grids", path)) { grids = 1; said = said " grids" }
      if (within("records", path)) { records = 1; said = said " records" }
      printf "  %s %s:%s\n", $1, path, said > "/dev/stderr"
    }
    END {
      printf "code=%s\nrust=%s\n", (code ? "true" : "false"), (rust ? "true" : "false")
      printf "grids=%s\nrecords=%s\n", (grids ? "true" : "false"), (records ? "true" : "false")
    }
  ') || { echo "the file rules could not be applied" >&2; exit 1; }
fi

# The rules are a pull request's: a push runs every job whole.
if [ "$pull" = 0 ]; then
  decided=$(printf '%s\n' "$decided" | sed -e 's/^code=.*/code=true/' -e 's/^rust=.*/rust=true/')
fi
if [ "${HEAVY_LABEL:-false}" = "true" ]; then
  echo "labelled heavy: both heavy groups run"
  decided=$(printf '%s\n' "$decided" | sed -e 's/^grids=.*/grids=true/' -e 's/^records=.*/records=true/')
fi

printf '%s\n' "$decided"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  printf '%s\n' "$decided" >> "$GITHUB_OUTPUT"
fi
exit 0
