# Sidekick backend

What the desktop app logs into. It holds the things that should follow a person rather than a
laptop: the workspace's settings and playbook, every finished call, and the consent log.

**Transcripts are not sent here.** The prospect's words stay on the laptop until the consent flow
and data handling have had a legal review (roadmap Phase 4). The server receives the summary, the
numbers and the consent events.

**Secrets live in the server's environment, never in a table:** the database URL, the token
signing secret, and later the vendor keys. Everything a workspace can change lives in the
database, so a fresh install is configured by logging in.

## Running it

```bash
cp .env.example .env                 # then change JWT_SECRET
docker compose up -d db              # Postgres on port 55432 (5432 is often taken on Windows)
uv sync
uv run python -m app.seed --workspace Acme --email you@example.com --password ...
uv run uvicorn app.main:app --reload --port 8000
```

`http://localhost:8000/docs` is the API browser. The whole stack (API in Docker too) is
`docker compose up --build`.

Running the seed command again with the same email resets that user's password.

## Endpoints

| Method | Path | What it does |
|---|---|---|
| GET | `/health` | Is the server up (no login) |
| POST | `/auth/login` | Email and password, returns the token the app keeps |
| GET | `/me` | Who the token belongs to |
| GET | `/settings` | What a fresh install pulls: language, limits, playbook |
| PUT | `/settings` | Change them (admins only); the playbook is versioned |
| POST | `/calls` | A finished call. Sending the same `call_id` again updates it |
| POST | `/stt/key` | A short-lived Deepgram key for this person's calls |
| POST | `/llm/messages` | Claude, with the server's key and the user's spend limit |
| GET | `/updates/latest` | The newest published build, for the app's updater |
| POST | `/updates` | Publishes a build (admin; see Releasing below) |
| GET | `/calls` | Recent calls in this workspace |
| GET | `/calls/{call_id}` | One call |

Calls and settings are per workspace: a token from one workspace never sees another's.

## Tests

```bash
uv run pytest
```

They run against a throwaway SQLite database, so no Docker is needed: login, settings and
playbook versions, a call sent twice staying one call, and one workspace not seeing another's.

## Releasing the desktop app

Installed copies check this server for a newer build and install it themselves. Nothing is
published publicly: the feed and the installer both need a sign-in, and each build carries a
signature the app checks against the release key, so even a server that has been taken over can't
get a foreign build installed.

To release (Windows and macOS together, from GitHub; nothing is built on a laptop):

1. In a pull request, bump `version` in `client/src-tauri/tauri.conf.json` and
   `client/src-tauri/Cargo.toml`; merge it once CI is green.
2. On `main`: `git tag -a v0.4.0 -m "What changed, for users"` then `git push origin v0.4.0`.

`.github/workflows/release.yml` then checks the versions match, builds the Windows installer and
one universal macOS app (Intel and Apple Silicon), signs both, publishes them here
(`windows-x86_64`, `darwin-aarch64`, `darwin-x86_64`) and attaches the installers to a GitHub
release. Installed copies offer the update within 15 minutes; users press Install and restart, and
it never interrupts a call in progress. Each OS is only offered its own newest build.

One-time setup, done once by the release owner:

1. **Update signing key.** It already exists (`~/.sidekick-release/sidekick.key` + its password on
   the machine that made the first releases; `npx @tauri-apps/cli signer generate` made it). Never
   make a new one: installed apps refuse updates signed with any other key, and its public half is
   in `client/src-tauri/tauri.conf.json`. Losing it means no installed copy can be updated again.
2. **macOS signing certificate.** On a Mac, `scripts/release-mac.sh --setup-only` creates
   `~/.sidekick-release/mac-signing.p12`, a self-made code-signing certificate. It keeps the app's
   identity the same from build to build, so macOS remembers the mic and system-audio permissions
   across updates. It is not an Apple Developer ID: a fresh install still needs right-click > Open
   once; updates don't.
3. **GitHub settings** (repo > Settings > Secrets and variables > Actions):

   | Name | Kind | Value |
   | --- | --- | --- |
   | `SIDEKICK_BACKEND_URL` | Variable | `https://sidekick.example.com` |
   | `SIDEKICK_RELEASE_EMAIL` | Secret | an admin account used only for releases |
   | `SIDEKICK_RELEASE_PASSWORD` | Secret | its password |
   | `TAURI_SIGNING_PRIVATE_KEY` | Secret | contents of `sidekick.key` |
   | `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Secret | its password |
   | `MAC_SIGNING_P12_BASE64` | Secret | `base64 -i ~/.sidekick-release/mac-signing.p12` |
   | `MAC_SIGNING_P12_PASSWORD` | Secret | contents of `mac-signing-password.txt` |

   Back up `~/.sidekick-release/` (password manager) as well.

Fallback when GitHub is unavailable: `scripts/release.ps1` on Windows and `scripts/release-mac.sh`
on a Mac do the same per platform.

`PUBLIC_URL` must be the address the app reaches this server on: the update file hands out a
download link built from it.

## Deploying

**Automatic:** when a change under `backend/` is merged into `main`, the **Deploy backend**
workflow (`.github/workflows/deploy-backend.yml`) runs the backend tests, then pushes that commit
to the VPS. If a required reviewer is set on the `production` environment (repo Settings >
Environments), it waits for their approval first. "Run workflow" on the Actions tab deploys `main`
by hand. App-only changes never deploy the server, and server deploys never release the app.

It uses a deploy key that can only push to `/opt/sidekick.git` (GitHub secrets `VPS_DEPLOY_KEY`
and `VPS_KNOWN_HOSTS`, variable `VPS_DEPLOY_TARGET`). Its public half sits in the VPS's
`/root/.ssh/authorized_keys`, restricted to that one command.

**By hand** (fallback): the server holds a bare repo and deploys what you push to it. Nothing on
the server has GitHub credentials, and `DEPLOYED_COMMIT` always names the commit that is running.

```bash
git remote add deploy root@your-server:/opt/sidekick.git   # once
git push deploy main                                            # every deploy
```

The hook (`backend/deploy/post-receive.sh`, installed at `/opt/sidekick.git/hooks/post-receive`)
checks the code out into `/opt/sidekick`, rebuilds with `docker-compose.prod.yml`, and waits for
`/health` before reporting success. `.env` lives at `/opt/sidekick/.env` and is never in git.

On the server: `docker compose -p sidekick -f /opt/sidekick/backend/docker-compose.prod.yml logs -f api`
for the running container, or `journalctl -t sidekick-api --since "2026-10-22 10:00"` for any time,
including before a deploy replaced the container. Each call's own log, as the laptop recorded it,
is at `GET /calls/{call_id}/logs`.
Always pass `-p sidekick`, so the database and published-build volumes stay with the stack.

nginx config for the site is in `deploy/nginx-sidekick.conf`.

## Not built yet

- **Migrations.** Tables are created at startup while the schema is young; Alembic arrives with
  the first change that has to keep real data.
- **Usage table**: spend limits currently add up the cost of a user's calls, which is close
  enough while calls are the only spend, but token-level usage belongs in its own table.
