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
# An absolute path, so a relative argument still works after a cd.
abs() { case $1 in /*) printf '%s' "$1" ;; *) printf '%s/%s' "$PWD" "$1" ;; esac; }
# shasum ships with macOS; sha256sum with coreutils.
sha256() {
  if command -v sha256sum > /dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}
quiet_git() { GIT_TERMINAL_PROMPT=0 GCM_INTERACTIVE=never SSH_ASKPASS_REQUIRE=never git "$@"; }
# Never inherit a caller's git environment.
unset GIT_DIR GIT_WORK_TREE GIT_COMMON_DIR GIT_INDEX_FILE GIT_OBJECT_DIRECTORY \
      GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_CEILING_DIRECTORIES

pack() {
  local wt out id=$3 have=${4:-}
  wt=$(abs "$1"); out=$(abs "$2")
  [ -n "$have" ] && have=$(abs "$have")
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
  # Any exit from here on, a refusal included, leaves no temporary refs or
  # files behind in the person's repository.
  trap "rm -rf $(printf %q "$tmp"); git update-ref -d refs/dent/move/$id/snapshot 2>/dev/null; git update-ref -d refs/dent/move/$id/index 2>/dev/null" EXIT
  local idx="$tmp/index"
  local real; real=$(git rev-parse --git-path index)
  if [ -f "$real" ]; then
    cp "$real" "$idx"
    # An old mtime marks every entry racily clean, so same-size edits made in
    # the same second as the last index write are re-read, not trusted.
    touch -t 197001010000.01 "$idx"
  fi
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

  # Commits to leave out. It can end up empty, and macOS's bash 3.2 treats
  # "${a[@]}" of an empty array as unbound under set -u, hence ${a[@]+…} below.
  local not=()
  if [ -n "$have" ] && [ -s "$have" ]; then
    while read -r h; do
      if git cat-file -e "$h^{commit}" 2>/dev/null; then not+=("^$h"); fi
    done < "$have"
  else
    not+=(--not --remotes=origin)
  fi
  local refs=("refs/dent/move/$id/snapshot" "refs/dent/move/$id/index")
  if [ -n "$branch" ]; then refs+=("refs/heads/$branch"); fi
  git bundle create -q "$out/$id.bundle" "${refs[@]}" ${not[@]+"${not[@]}"}
  local size; size=$(wc -c < "$out/$id.bundle" | tr -d ' ')
  if [ "$size" -gt $((200 * 1024 * 1024)) ]; then
    rm -f "$out/$id.bundle"
    die "the work is over 200 MiB; push the branch, then move"
  fi
  local sum; sum=$(sha256 "$out/$id.bundle")
  printf '{"id":"%s","tip":"%s","snapshot":"%s","index":"%s","branch":"%s","bytes":%s,"sha256":"%s"}\n' \
    "$id" "$tip" "$snap" "$ixc" "$branch" "$size" "$sum" > "$out/$id.json"
  git update-ref -d "refs/dent/move/$id/snapshot" "$snap"
  git update-ref -d "refs/dent/move/$id/index" "$ixc"
  rm -rf "$tmp"
  trap - EXIT
  cat "$out/$id.json"
}

field() { sed -E "s/.*\"$2\":\"?([^\",}]*)\"?.*/\1/" "$1"; }

apply() {
  local repo out id=$3 new
  repo=$(abs "$1"); out=$(abs "$2"); new=$(abs "$4")
  local man="$out/$id.json" bundle="$out/$id.bundle"
  local tip snap ixc branch sum
  tip=$(field "$man" tip); snap=$(field "$man" snapshot); ixc=$(field "$man" index)
  branch=$(field "$man" branch); sum=$(field "$man" sha256)
  [ "$(sha256 "$bundle")" = "$sum" ] || die "the bundle doesn't match its hash"
  cd "$repo"
  local heads; heads=$(git bundle list-heads "$bundle" 2>/dev/null) || die "the bundle is damaged"
  grep -q " refs/dent/move/$id/snapshot$" <<< "$heads" && grep -q " refs/dent/move/$id/index$" <<< "$heads" \
    || die "the bundle is damaged"
  if ! git bundle verify -q "$bundle" > /dev/null 2>&1; then
    quiet_git fetch -q origin || true
    git bundle verify -q "$bundle" > /dev/null 2>&1 || die "commits it builds on are missing here; push the branch where it came from"
  fi
  if [ -n "$branch" ]; then
    git check-ref-format "refs/heads/$branch" || die "bad branch name"
  fi

  # Undo steps, run newest first. Starts with a harmless entry because
  # macOS's bash 3.2 treats "${a[@]}" of an empty array as unbound.
  local undo=(:)
  cleanup() {
    git update-ref -d "refs/dent/move/$id/snapshot" 2>/dev/null || true
    git update-ref -d "refs/dent/move/$id/index" 2>/dev/null || true
  }
  rollback() {
    # The new worktree may be the current folder, and may be removed below.
    cd "$repo" || true
    local i
    for ((i = ${#undo[@]} - 1; i >= 0; i--)); do eval "${undo[i]}" || true; done
    cleanup
  }
  # Any exit before the end, a refusal included, undoes what this run made.
  trap 'rollback' EXIT

  git fetch -q "$bundle" "+refs/dent/move/$id/snapshot:refs/dent/move/$id/snapshot" \
                         "+refs/dent/move/$id/index:refs/dent/move/$id/index"
  [ "$(git rev-parse "refs/dent/move/$id/snapshot")" = "$snap" ] || die "snapshot mismatch"
  [ "$(git rev-parse "refs/dent/move/$id/index")" = "$ixc" ] || die "index mismatch"
  [ "$(git rev-parse "$snap^")" = "$tip" ] || die "snapshot parent mismatch"

  local checkout="$branch"
  if [ -n "$branch" ]; then
    local cur=""
    cur=$(git rev-parse -q --verify "refs/heads/$branch" || true)
    if git worktree list --porcelain | grep -qxF "branch refs/heads/$branch"; then
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
  local q; q=$(printf %q "$new")
  undo+=("git worktree remove --force $q 2>/dev/null; rm -rf $q; git worktree prune")
  rmdir "$new"
  if [ -n "$checkout" ]; then
    git worktree add -q "$new" "$branch"
  else
    git worktree add -q --detach "$new" "$tip"
  fi
  cd "$new"
  git checkout -q --no-overlay "$snap" -- .
  git read-tree -m "$ixc^{tree}"
  trap - EXIT
  cleanup
  echo "applied $id into $new"
}

case "${1:-}" in
  pack) shift; pack "$@" ;;
  apply) shift; apply "$@" ;;
  *) echo "usage: move.sh pack WORKTREE OUT_DIR ID [HAVE_FILE] | apply REPO OUT_DIR ID NEW_WORKTREE" >&2; exit 2 ;;
esac
