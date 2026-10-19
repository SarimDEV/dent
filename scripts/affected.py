"""Which workspace crates a change touches, as cargo `-p` flags.

    python3 -I scripts/affected.py [BASE]   (default BASE: origin/main)

Counts what differs from where this branch left BASE, committed or not,
plus untracked files. A crate is in if one of its files changed or it
depends on one that did, so a change to a shared crate checks everything
that uses it. A change outside any crate that can affect the build
(Cargo.toml, Cargo.lock, .cargo, rust-toolchain, the justfile, this
script) means all of `crates/*`. Spikes count only when one is touched.

Prints nothing when no crate is affected (docs only, say).
"""

import json
import subprocess
import sys
from pathlib import Path

base = sys.argv[1] if len(sys.argv) > 1 else "origin/main"


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True).stdout


root = Path(git("rev-parse", "--show-toplevel").strip())
fork = git("merge-base", "HEAD", base).strip() or base
changed = set(git("diff", "--name-only", fork).split())
changed |= set(git("ls-files", "--others", "--exclude-standard").split())

meta = json.loads(
    subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        capture_output=True,
        text=True,
        check=True,
        cwd=root,
    ).stdout
)
crates = {}  # name -> (directory relative to the root, workspace deps)
names = {p["name"] for p in meta["packages"]}
for p in meta["packages"]:
    d = Path(p["manifest_path"]).parent.relative_to(root).as_posix()
    crates[p["name"]] = (d, {x["name"] for x in p["dependencies"]} & names)
default = sorted(n for n, (d, _) in crates.items() if d.startswith("crates/"))

build_wide = {"Cargo.toml", "Cargo.lock", "justfile", "rust-toolchain", "rust-toolchain.toml", "scripts/affected.py"}
if any(f in build_wide or f.startswith(".cargo/") for f in changed):
    hit = set(default)
else:
    hit = {n for n, (d, _) in crates.items() if any(f.startswith(d + "/") for f in changed)}

# Everything that depends on a hit crate, however indirectly.
grew = True
while grew:
    more = {n for n, (_, deps) in crates.items() if deps & hit} - hit
    hit |= more
    grew = bool(more)

print(" ".join(f"-p {n}" for n in sorted(hit)))
