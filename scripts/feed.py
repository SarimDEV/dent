#!/usr/bin/env python3
"""Write latest.json for the parts in a folder (decision 0077).

    feed.py PARTS BUILD COMMIT BASE_URL > latest.json

Each part `<kind>-<os>-<arch>.tar.gz` is published as
`<kind>-<os>-<arch>-<build>.tar.gz` under BASE_URL, so a download never
mixes two builds.
"""
import hashlib, json, os, sys

parts, build, commit, base = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4].rstrip("/")
feed = {"build": build, "commit": commit, "parts": {}}
for name in sorted(os.listdir(parts)):
    if not name.endswith(".tar.gz"):
        continue
    path = os.path.join(parts, name)
    kind = name[: -len(".tar.gz")]
    with open(path, "rb") as f:
        sha = hashlib.sha256(f.read()).hexdigest()
    feed["parts"][kind] = {
        "url": f"{base}/{kind}-{build}.tar.gz",
        "sha256": sha,
        "size": os.path.getsize(path),
    }
if not feed["parts"]:
    sys.exit("no parts in " + parts)
json.dump(feed, sys.stdout, indent=2)
print()
