"""Search NVD for the advisories `cargo audit` can't see, and stop on one nobody has looked at.

`cargo audit` checks the crates Cargo.lock takes from a registry, against RustSec. It doesn't
see vendor/tiny_http (a path), and an advisory filed with NVD and left unreviewed on GitHub, with
no package named, reaches neither RustSec nor OSV nor a GitHub search by package: the three of
2026-07-28 for tiny_http and rouille didn't. This searches NVD's text for each name in
.cargo/advisories-by-name.toml, and lists every CVE it finds with what was done about it.

usage: python3 .github/advisories-by-name.py      (from the repository's root; a step before
       each tag, README.md's "Releasing")

Exits 0 when every CVE found is in the list; 1, naming them, when one isn't (read it, then add it
with what was done: fixed here, or why it doesn't apply); 2 when NVD can't be asked (nothing is
taken as checked then).
"""
import json
import sys
import time
import tomllib
import urllib.parse
import urllib.request

LIST = ".cargo/advisories-by-name.toml"
NVD = "https://services.nvd.nist.gov/rest/json/cves/2.0"
# NVD answers five requests in 30 s to a caller without a key
BETWEEN = 7


def search(name):
    url = NVD + "?" + urllib.parse.urlencode({"keywordSearch": name})
    request = urllib.request.Request(url, headers={"User-Agent": "advisories-by-name"})
    for attempt in range(3):
        try:
            with urllib.request.urlopen(request, timeout=60) as answer:
                found = json.load(answer)
            return {v["cve"]["id"]: next((d["value"] for d in v["cve"].get("descriptions", []) if d["lang"] == "en"), "")
                    for v in found["vulnerabilities"]}
        # an answer of another shape is one NVD didn't give: asked again, as one not given
        except (OSError, ValueError, KeyError, TypeError) as e:
            last = e
            time.sleep(BETWEEN)
    print("NVD can't be asked about %r (%s): nothing is checked" % (name, last), file=sys.stderr)
    sys.exit(2)


with open(LIST, "rb") as f:
    listed = tomllib.load(f)
done = {a["id"]: a for a in listed["advisory"]}
unlisted = []
for i, name in enumerate(listed["names"]):
    if i:
        time.sleep(BETWEEN)
    for cve, said in sorted(search(name).items()):
        if cve in done:
            print("%s  %-14s %s" % (name, cve, done[cve]["done"]))
        else:
            print("%s  %-14s NOT IN THE LIST: %s" % (name, cve, said[:160]))
            unlisted.append(cve)
if unlisted:
    print("\n%d CVE(s) not in %s: read each, then add it with what was done." % (len(set(unlisted)), LIST))
    sys.exit(1)
print("\nevery CVE found is in %s" % LIST)
