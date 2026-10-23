#!/usr/bin/env bash
# holon's installers under the name releases up to 0.1.0-rc.3 used: holon-installer.{sh,ps1} in
# $1 become holon-server-installer.{sh,ps1} in $2. A `holon-server update` there runs the newest
# release's holon-server-installer with that install's settings under the old names
# (HOLON_SERVER_NO_MODIFY_PATH=1 when it was installed without touching PATH, …): each copy
# reads those as holon's own, unless holon's are set. Run by legacy-installers.yml.
set -euo pipefail
in=$1
out=$2
mkdir -p "$out"
vars="INSTALL_DIR NO_MODIFY_PATH UNMANAGED_INSTALL INSTALLER_GITHUB_BASE_URL INSTALLER_GHE_BASE_URL"

# sh: right after the shebang.
{
  head -n 1 "$in/holon-installer.sh"
  echo "# Installed before as holon-server: its settings, under the old names, count as holon's."
  for v in $vars; do
    echo "if [ -n \"\${HOLON_SERVER_$v:-}\" ] && [ -z \"\${HOLON_$v:-}\" ]; then HOLON_$v=\"\$HOLON_SERVER_$v\"; export HOLON_$v; fi"
  done
  tail -n +2 "$in/holon-installer.sh"
} > "$out/holon-server-installer.sh"

# ps1: after the param block (a script's first statement), before anything reads them.
mapping=$(
  echo "# Installed before as holon-server: its settings, under the old names, count as holon's."
  for v in $vars; do
    echo "if (\$env:HOLON_SERVER_$v -and -not \$env:HOLON_$v) { \$env:HOLON_$v = \$env:HOLON_SERVER_$v }"
  done
)
anchor='if ($NoModifyPath) {'
grep -qF "$anchor" "$in/holon-installer.ps1" || { echo "no place for the old names in holon-installer.ps1" >&2; exit 1; }
awk -v mapping="$mapping" -v anchor="$anchor" '!done && index($0, anchor) == 1 { print mapping; done = 1 } { print }' "$in/holon-installer.ps1" > "$out/holon-server-installer.ps1"
grep -q 'HOLON_SERVER_NO_MODIFY_PATH' "$out/holon-server-installer.ps1"
