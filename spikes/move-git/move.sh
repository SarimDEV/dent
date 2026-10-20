#!/usr/bin/env bash
# Remote plan stage 0, R-D2 and R-D4 (docs/remote-tests.md): move a worktree's
# git state to another machine as snapshot commits in a bundle.
#
#   move.sh pack  WORKTREE OUT_DIR ID [HAVE_FILE]
#   move.sh apply REPO     OUT_DIR ID NEW_WORKTREE
#
# pack writes two commits on top of HEAD, under refs/dent/move/ID/:
#   snapshot: the working tree, built in a copy of the index (untracked files
#             in, ignored files out unless already in the index);
#   index:    the index as it is, so the staged/unstaged split can travel.
# The person's own index, split index and fsmonitor are never touched.
# The bundle leaves out commits listed in HAVE_FILE (tips the target already
# has), or everything on origin when no file is given.
#
# apply refuses a diverged branch, a branch checked out elsewhere, and a
# corrupt bundle; otherwise it moves the branch fast-forward only, adds a
# worktree, lays the snapshot down (deletions included) and restores the
# index from the index commit.
set -euo pipefail

die() { echo "refused: $*" >&2; exit 3; }
quiet_git() { GIT_TERMINAL_PROMPT=0 GCM_INTERACTIVE=never SSH_ASKPASS_REQUIRE=never git "$@"; }
# Never inherit a caller's git environment.
unset GIT_DIR GIT_WORK_TREE GIT_COMMON_DIR GIT_INDEX_FILE GIT_OBJECT_DIRECTORY \
      GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_CEILING_DIRECTORIES

pack() {
  local wt=$1 out=$2 id=$3 have=${4:-}
  mkdir -p "$out"
  cd "$wt"
  local tip branch
  tip=$(git rev-parse --verify HEAD)
  branch=$(git symbolic-ref -q HEAD || true)
  branch=${branch#refs/heads/}

  # Things a snapshot can't carry.
  if git status --porcelain=v1 --untracked-files=all | grep -qE '^\?\? .*/$'; then
    die "a nested git checkout is inside this worktree"
  fi
  if git diff --raw HEAD | grep -qE '^:[0-7]+ 160000|^:160000'; then
    die "a submodule has changes; commit or discard them first"
  fi

  local tmp; tmp=$(mktemp -d)
  local idx="$tmp/index"
  local real; real=$(git rev-parse --git-path index)
  if [ -f "$real" ]; then cp "$real" "$idx"; fi
  # Same-size edits made in the same second must be re-read, not trusted.
  touch -t 197001010000.01 "$idx" 2>/dev/null || true
  local cfg=(-c core.splitIndex=false -c splitIndex.sharedIndexExpire=never -c core.fsmonitor=false)
  local snap_tree idx_tree
  GIT_INDEX_FILE=$idx git "${cfg[@]}" add --all -- ':/'
  snap_tree=$(GIT_INDEX_FILE=$idx git write-tree)
  idx_tree=$(git "${cfg[@]}" write-tree)
  local who=(-c user.name=dent -c user.email=dent@localhost)
  local snap ixc
  snap=$(git "${who[@]}" commit-tree "$snap_tree" -p "$tip" -m "dent move $id: working tree")
  ixc=$(git "${who[@]}" commit-tree "$idx_tree" -p "$tip" -m "dent move $id: index")
  git update-ref "refs/dent/move/$id/snapshot" "$snap"
  git update-ref "refs/dent/move/$id/index" "$ixc"
  rm -rf "$tmp"

  local not=()
  if [ -n "$have" ] && [ -s "$have" ]; then
    while read -r h; do
      git cat-file -e "$h^{commit}" 2>/dev/null && not+=("^$h")
    done < "$have"
  else
    not+=(--not --remotes=origin)
  fi
  local refs=("refs/dent/move/$id/snapshot" "refs/dent/move/$id/index")
  [ -n "$branch" ] && refs+=("refs/heads/$branch")
  git bundle create -q "$out/$id.bundle" "${refs[@]}" "${not[@]}"
  local size; size=$(wc -c < "$out/$id.bundle" | tr -d ' ')
  if [ "$size" -gt $((200 * 1024 * 1024)) ]; then
    rm -f "$out/$id.bundle"
    die "the work is over 200 MiB; push the branch, then move"
  fi
  local sum; sum=$(shasum -a 256 "$out/$id.bundle" | cut -d' ' -f1)
  printf '{"id":"%s","tip":"%s","snapshot":"%s","index":"%s","branch":"%s","bytes":%s,"sha256":"%s"}\n' \
    "$id" "$tip" "$snap" "$ixc" "$branch" "$size" "$sum" > "$out/$id.json"
  git update-ref -d "refs/dent/move/$id/snapshot" "$snap"
  git update-ref -d "refs/dent/move/$id/index" "$ixc"
  cat "$out/$id.json"
}

field() { sed -E "s/.*\"$2\":\"?([^\",}]*)\"?.*/\1/" "$1"; }

apply() {
  local repo=$1 out=$2 id=$3 new=$4
  local man="$out/$id.json" bundle="$out/$id.bundle"
  local tip snap ixc branch sum
  tip=$(field "$man" tip); snap=$(field "$man" snapshot); ixc=$(field "$man" index)
  branch=$(field "$man" branch); sum=$(field "$man" sha256)
  [ "$(shasum -a 256 "$bundle" | cut -d' ' -f1)" = "$sum" ] || die "the bundle doesn't match its hash"
  cd "$repo"
  local heads; heads=$(git bundle list-heads "$bundle" 2>/dev/null) || die "the bundle is damaged"
  grep -q " refs/dent/move/$id/snapshot$" <<< "$heads" && grep -q " refs/dent/move/$id/index$" <<< "$heads" \
    || die "the bundle is damaged"
  if ! git bundle verify -q "$bundle" > /dev/null 2>&1; then
    quiet_git fetch -q origin || true
    git bundle verify -q "$bundle" > /dev/null 2>&1 || die "commits it builds on are missing here; push the branch where it came from"
  fi
  [ -n "$branch" ] && git check-ref-format "refs/heads/$branch" || [ -z "$branch" ] || die "bad branch name"
  git fetch -q "$bundle" "refs/dent/move/$id/snapshot:refs/dent/move/$id/snapshot" \
                         "refs/dent/move/$id/index:refs/dent/move/$id/index"
  [ "$(git rev-parse "refs/dent/move/$id/snapshot")" = "$snap" ] || die "snapshot mismatch"
  [ "$(git rev-parse "$snap^")" = "$tip" ] || die "snapshot parent mismatch"

  local undo=()
  cleanup() {
    git update-ref -d "refs/dent/move/$id/snapshot" 2>/dev/null || true
    git update-ref -d "refs/dent/move/$id/index" 2>/dev/null || true
  }
  local done=0
  rollback() {
    [ "$done" = 1 ] && return
    for u in "${undo[@]}"; do eval "$u" || true; done
    cleanup
  }
  # Any exit before the end, a refusal included, undoes what this run made.
  trap 'rollback' EXIT

  local checkout="$branch"
  if [ -n "$branch" ]; then
    local cur=""
    cur=$(git rev-parse -q --verify "refs/heads/$branch" || true)
    if git worktree list --porcelain | grep -qx "branch refs/heads/$branch"; then
      if [ "$cur" = "$tip" ]; then checkout=""; else die "$branch is checked out in another worktree here"; fi
    fi
    if [ -n "$cur" ] && [ "$cur" != "$tip" ]; then
      git merge-base --is-ancestor "$cur" "$tip" || die "$branch has commits here the move doesn't; reconcile, then move again"
      git update-ref "refs/heads/$branch" "$tip" "$cur"
      undo+=("git update-ref refs/heads/$branch $cur $tip")
    elif [ -z "$cur" ]; then
      git update-ref "refs/heads/$branch" "$tip" ""
      undo+=("git update-ref -d refs/heads/$branch $tip")
    fi
  fi
  mkdir "$new" || die "$new already exists"
  undo+=("git worktree remove --force '$new' 2>/dev/null; rm -rf '$new'; git worktree prune")
  rmdir "$new"
  if [ -n "$checkout" ]; then
    git worktree add -q "$new" "$branch"
  else
    git worktree add -q --detach "$new" "$tip"
  fi
  cd "$new"
  git checkout -q --no-overlay "$snap" -- .
  git read-tree -m "$ixc^{tree}"
  done=1
  trap - EXIT
  cleanup
  echo "applied $id into $new"
}

case "${1:-}" in
  pack) shift; pack "$@" ;;
  apply) shift; apply "$@" ;;
  *) echo "usage: move.sh pack WORKTREE OUT_DIR ID [HAVE_FILE] | apply REPO OUT_DIR ID NEW_WORKTREE" >&2; exit 2 ;;
esac
