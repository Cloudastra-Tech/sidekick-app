# Sidekick: live call coach

Sidekick is a desktop app for sales reps. It listens to a call (Zoom, Teams, Google Meet, a phone
app, anything), transcribes both sides live, and shows short coaching hints on screen while the
call is happening: what to say to an objection, a buying signal to follow up, a question to ask.
After the call it writes a summary and sends it to the CRM.

It runs on **Windows** and **macOS** (Intel and Apple Silicon, macOS 14.2+).

## What it does

| | |
|---|---|
| **Hears both sides** | Captures your mic and the call audio separately, on the laptop itself. No meeting bot joins the call. |
| **Live transcript** | Deepgram streaming speech-to-text: English, Indian English, Hindi, and Hindi-English mixed. |
| **Hints that come to you** | Claude Haiku suggests the words to say when the prospect objects, shows a buying signal, finishes speaking or goes quiet. No button to press. |
| **Floating overlay** | A small always-on-top panel shows the latest hint, an opening checklist that ticks itself, and the transcript. `Ctrl+Shift+H` shows or hides it. |
| **Notices calls** | Offers to start coaching when a call app starts using the mic. It never starts on its own. |
| **Consent first** | Every call starts with a consent step, and it's logged. Nothing is captured before it. |
| **After the call** | A structured summary (needs, pains, objections, budget, timeline, next step), saved locally and sent to the CRM (Twenty) and the team's server. |
| **Team workspace** | Reps sign in to the company server: shared playbook, settings, spend limits, admin and members. AI and speech keys stay on the server, not on laptops. |
| **Cost control** | Spend cap per call, monthly limit per person, auto-stop after long silence. Cost per call-hour is measured and shown. |
| **Auto-update** | Installed apps offer new versions themselves; updates are signed and never install during a call. |

## How it fits together

```
 Laptop (client/)                                          Server (backend/)
 ┌──────────────────────────────────────────────┐         ┌──────────────────────────┐
 │  Mic ─┐                                       │         │ FastAPI + PostgreSQL     │
 │       ├─ audio capture ─ Deepgram (live text) │         │  sign-in, members, roles │
 │ Call ─┘   WASAPI (Win)        │               │         │  playbook and settings   │
 │  audio    Core Audio (Mac)    ▼               │  HTTPS  │  call summaries, consent │
 │                    trigger engine ── Claude ──┼────────▶│  Claude + Deepgram keys  │
 │                    (objection, pause, …)      │         │  CRM proxy (Twenty)      │
 │                           │                   │         │  app updates             │
 │                           ▼                   │         └──────────────────────────┘
 │             overlay + main window (Next.js)   │
 └──────────────────────────────────────────────┘
```

- **`client/src-tauri/`**: the app's engine, in Rust (Tauri 2). Audio capture, speech-to-text,
  triggers, Claude calls, call detection, consent, summaries, CRM, updates.
- **`client/ui/`**: the screens, in Next.js and React: live call, overlay, summary, history,
  settings.
- **`backend/`**: the team server, in Python (FastAPI) and PostgreSQL, deployed with Docker.
  See [backend/README.md](backend/README.md).
- **`client/src-tauri/playbook/sales.md`**: what the coach knows about your company and offer.
  The one shipped is a sample for a made-up company: replace it with your own.

## Run it on your machine

You need [Rust](https://rustup.rs), [Node.js](https://nodejs.org) 22+ and, for the server,
[uv](https://docs.astral.sh/uv/) and Docker. On Windows also the
[Tauri prerequisites](https://tauri.app/start/prerequisites/) (Visual Studio C++ build tools,
WebView2).

```bash
# Desktop app
cd client
npm install && (cd ui && npm install)
cp src-tauri/.env.example src-tauri/.env    # add DEEPGRAM_API_KEY and ANTHROPIC_API_KEY,
                                            # or set SIDEKICK_BACKEND_URL and sign in instead
npm run tauri dev                           # runs the app
npm run tauri:ui-dev                        # same, with hot reload for UI work
```

No mic or call handy? **Run demo call** on the Live call screen plays a scripted call through
the real hint and summary pipeline.

```bash
# Server (optional for app work)
cd backend
cp .env.example .env && docker compose up -d db && uv sync
uv run python -m app.seed --workspace Acme --email you@example.com --password ...
uv run uvicorn app.main:app --reload --port 8000
```

## Checks

```bash
cd client/src-tauri && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
cd backend && uv run ruff check app tests && uv run ruff format app tests && uv run pytest
```

Run them before every pull request. After a merge, GitHub runs them on Windows (plus the
backend) for `main`.

## Working on it

- **[CONTRIBUTING.md](CONTRIBUTING.md)**: how to add a feature for both platforms, test it,
  get it merged and released. Read this before your first change.
- **[CLAUDE.md](CLAUDE.md)**: the project rules (stack, the cost rule, consent, how we ship).
  Claude Code follows it automatically; people should too.
- **[backend/README.md](backend/README.md)**: the server, its API, deploying and releasing.

## Releasing

Bump the version, then push a tag on `main`:

```bash
git tag -a v0.4.0 -m "What changed, for users"
git push origin v0.4.0
```

GitHub builds, signs and publishes Windows and macOS together; installed apps offer the update
within 15 minutes. Details: [CONTRIBUTING.md](CONTRIBUTING.md#5-release).

## License

Sidekick is free software under the [GNU Affero General Public License v3.0](LICENSE)
(AGPL-3.0). You can use, change and share it; if you run a changed version for other people,
including as a hosted service, you must share your changes under the same license.

Copyright (C) 2026 Cloudastra Technologies.
