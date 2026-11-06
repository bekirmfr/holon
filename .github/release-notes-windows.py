"""Rewrite the Windows install command cargo-dist puts in a release's notes.

dist writes `powershell -ExecutionPolicy Bypass -c "irm <url> | iex"`. Microsoft Defender flags
that line as a trojan pattern, and Windows PowerShell 5.1 can hand `iex` the downloaded script a
line at a time, which fails ("The terminator '#>' is missing from the multiline comment"). The
notes say instead to download the script, then run it, as the README and the manual do.

usage: python3 release-notes-windows.py < notes.md > fixed.md

Exits 1, writing nothing, when the notes have neither dist's line nor this script's own (dist's
wording changed: this script needs updating to match).
"""
import re
import sys

notes = sys.stdin.read()
DIST = re.compile(
    r'```sh\npowershell -ExecutionPolicy Bypass -c "irm (?P<url>https://\S+/holon-installer\.ps1) \| iex"\n```'
)
OURS = "-OutFile holon-installer.ps1"


def ours(m):
    return (
        "In PowerShell, download the script, then run it:\n\n"
        "```powershell\n"
        "cd $env:TEMP\n"
        f"irm {m.group('url')} -OutFile holon-installer.ps1\n"
        "powershell -ExecutionPolicy Bypass -File .\\holon-installer.ps1\n"
        "```\n\n"
        "Run straight from the download (`irm … | iex`), Windows PowerShell can fail, and Microsoft "
        "Defender flags the one-line form. If an antivirus stops the script, download "
        "`holon-x86_64-pc-windows-msvc.zip` below and put `holon.exe` on your PATH."
    )


fixed, n = DIST.subn(ours, notes)
if n == 0 and OURS not in notes:
    sys.stderr.write("release-notes-windows: no Windows install command of dist's to rewrite\n")
    sys.exit(1)
sys.stdout.write(fixed)
