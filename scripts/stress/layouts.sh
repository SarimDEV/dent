#!/bin/bash
# Makes /tmp/structs/{rails,next,python,rust,workers,compose}: the ignored
# files of common project layouts, to see what a clone is offered.
set -e
B=/tmp/structs; rm -rf $B; mkdir -p $B
mk(){ d=$B/$1; mkdir -p $d; cd $d; git init -q; printf "$2" > .gitignore; shift 2; for f in "$@"; do mkdir -p "$(dirname "$f")"; echo "v" > "$f"; done; git add .gitignore >/dev/null; git -c user.name=t -c user.email=t@t commit -qm i; }
mk rails "/log/*\n/tmp/*\n/config/master.key\n/config/credentials/*.key\n.env*\n/storage/*\n/node_modules\n" config/master.key config/credentials/production.key .env.development log/development.log tmp/cache/x storage/db.sqlite3 node_modules/a/.env
mk next ".env*.local\n.next/\nnode_modules/\n.vercel\nnext-env.d.ts\n" .env.local .env.production.local .next/cache/x .vercel/project.json next-env.d.ts node_modules/react/.env
mk python ".venv/\n__pycache__/\n.env\n*.pyc\ninstance/\n.pytest_cache/\n" .env .venv/lib/python3.12/site-packages/x/.env instance/app.db app/__pycache__/m.pyc .pytest_cache/v
mk rust "/target\n.env\n" .env target/debug/build/x/out/.env target/debug/app
mk workers "node_modules/\n.dev.vars\n.wrangler/\n" .dev.vars .wrangler/state/v3/x node_modules/w/.dev.vars
mk compose ".env\ndocker-compose.override.yml\ndata/\n" .env docker-compose.override.yml data/pg/base
