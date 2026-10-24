#!/bin/bash
# Makes /tmp/envstress/mono: a monorepo of 150 packages with hundreds of
# environment files, node_modules and .venv full of files (env files buried
# inside), build output, junk and files too big to carry. Used by the walk
# move-clone-many.
# A monorepo as people really have them, for the clone file list.
set -e
R=/tmp/envstress/mono
rm -rf /tmp/envstress; mkdir -p $R; cd $R
git init -q
cat > .gitignore <<'G'
node_modules/
.venv/
.next/
dist/
*.log
.DS_Store
.env
.env.*
!.env.example
coverage/
local.db
big.bin
G
for i in $(seq 1 150); do
  d=packages/pkg-$i; mkdir -p $d/src $d/node_modules/dep/lib $d/dist
  echo "{}" > $d/package.json; echo "x" > $d/src/index.ts
  echo "KEY_$i=1" > $d/.env; echo "LOCAL_$i=1" > $d/.env.local
  echo "EX=1" > $d/.env.example; echo "T=1" > $d/.env.test
  echo "junk" > $d/.DS_Store; echo "log" > $d/debug.log
  for j in $(seq 1 40); do echo "m" > $d/node_modules/dep/lib/f$j.js; done
  echo "DEP=1" > $d/node_modules/dep/.env
  echo "b" > $d/dist/out.js
done
mkdir -p apps/web/.next/cache node_modules .venv/lib/site-packages
for j in $(seq 1 30000); do :; done
# A big root node_modules and venv, with env files buried inside.
python3 - <<'P'
import os
for i in range(2000):
    d=f"node_modules/pkg{i}"; os.makedirs(d,exist_ok=True)
    for j in range(25): open(f"{d}/f{j}.js","w").write("x")
    if i%10==0: open(f"{d}/.env","w").write("NM=1")
for i in range(500):
    d=f".venv/lib/site-packages/m{i}"; os.makedirs(d,exist_ok=True)
    for j in range(20): open(f"{d}/f{j}.py","w").write("x")
    if i%50==0: open(f"{d}/.env.sample","w").write("V=1")
P
echo "N=1" > apps/web/.env.production.local
mkdir -p "apps/a very long folder name for testing/and another deeply nested level/config"
echo "DEEP=1" > "apps/a very long folder name for testing/and another deeply nested level/config/.env.development.local"
echo "X=1" > .envrc; echo "Y=1" > .dev.vars; echo "S=1" > .env.sample; echo "T=1" > .env.template
head -c 2500000 /dev/urandom | base64 > .env.huge
head -c 60000000 /dev/zero > big.bin
for i in $(seq 1 15); do head -c 3000000 /dev/zero > .env.blob$i; done
echo db > local.db
git add -A >/dev/null; git -c user.name=t -c user.email=t@t commit -qm init
git remote add origin git@github.com:ada/mono.git
echo "tracked files: $(git ls-files | wc -l)  all files: $(find . -type f -not -path './.git/*' | wc -l)"
