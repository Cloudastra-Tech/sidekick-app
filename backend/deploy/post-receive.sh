#!/bin/bash
# Deploys Sidekick's backend on `git push deploy main`.
#
# The server holds a bare repo (/opt/sidekick.git) that nobody clones from; pushing to it checks
# the code out into /opt/sidekick and rebuilds. No GitHub credentials live on the server, and the
# checkout always matches a commit you can name.
#
# `.env` is never in git: it stays in /opt/sidekick between deploys.
set -euo pipefail

BRANCH="main"
WORKTREE="/opt/sidekick"
GIT_DIR="/opt/sidekick.git"
# Kept stable so the database and published-build volumes stay with their stack.
PROJECT="sidekick"

while read -r _old new ref; do
  [ "$ref" = "refs/heads/$BRANCH" ] || continue

  echo "--> checking out ${new:0:8}"
  git --work-tree="$WORKTREE" --git-dir="$GIT_DIR" checkout -f "$BRANCH"
  echo "$new" > "$WORKTREE/DEPLOYED_COMMIT"

  cd "$WORKTREE/backend"
  # The stack lives in backend/, but .env sits one level up with the rest of the server's state.
  ln -sf "$WORKTREE/.env" .env

  echo "--> building"
  docker compose -p "$PROJECT" -f docker-compose.prod.yml up -d --build

  echo "--> waiting for health"
  for _ in $(seq 1 30); do
    if curl -sf -m 5 http://127.0.0.1:8020/health > /dev/null; then
      echo "--> deployed ${new:0:8}"
      exit 0
    fi
    sleep 2
  done

  echo "!!! the new build didn't answer /health within a minute; check: docker compose -p $PROJECT logs api"
  exit 1
done
