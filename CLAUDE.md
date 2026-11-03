# Project: Live Call Coach

## Every task, every time (read first)
1. **Follow the process in [CONTRIBUTING.md](CONTRIBUTING.md)** and the "How we ship" rules below: a short branch from `main` → build it for **Windows and macOS** → tests → `cargo fmt` / `cargo clippy --all-targets -- -D warnings` / `cargo test` (and `ruff` / `pytest` for the backend) → all passing locally → pull request → merge → CI (Windows + backend) green on `main` → release only by version tag. Use the industry-standard way; don't invent a one-off process, and say so if asked to deviate.
2. **Adding a feature or function:** find where it lives with the table in CONTRIBUTING.md §2. Write shared logic once; put OS-specific code behind `#[cfg(windows)]` / `#[cfg(target_os = "macos")]` with **both** versions in the same change. A new UI-callable function = `#[tauri::command]` → register in `lib.rs` → wrapper in `client/ui/src/lib/sidekick.ts`.
3. **Keep the non-negotiables below:** event-driven hints with prompt caching, consent before capture, ask before new dependencies, server stays compatible with installed apps.
4. **Report honestly in the pull request** what was tested and on which OS. Code that wasn't run on Windows is called out as "needs a Windows check".
5. **Never commit** keys, `.env`, installers or `dist-mac/`. Never create a new update-signing key.

## What this is
A desktop agent for Windows and macOS that detects when a call starts, captures both sides of audio natively (no meeting bot), transcribes it live, and streams the transcript to Claude for real-time coaching hints during the call. After the call, it pushes a summary to a self-hosted backend.


## Stack (do not deviate without discussion)
- **Client app:** Rust + Tauri. This is the only client framework — do not suggest Electron.
- **Windows audio capture:** WASAPI loopback (system audio) + separate mic stream.
- **macOS audio capture:** Core Audio process tap (macOS 14.2+) + separate mic stream, via `objc2-core-audio` (`audio/coreaudio_capture.rs`).
- **Streaming STT:** Deepgram or AssemblyAI (vendor-abstracted — don't hardcode one).
- **Live coaching model:** Claude Haiku 4.5, via direct API calls.
- **Post-call summary model:** Claude Sonnet 5.
- **Backend/CRM:** Python + FastAPI + PostgreSQL, self-hosted, Docker-deployed. This is NOT Salesforce/HubSpot — build our own REST API.

## The one rule that matters most
The live coaching loop must be **event-driven, not fixed-interval polling**. Triggers: silence > 2s, keyword hits, turn-end detection — not a 30-second timer. Every call to the Haiku model must use **prompt caching**: static system prompt + playbook + running summary go in the cached prefix; only the new transcript increment is sent fresh each time.

A naive implementation (resend full growing transcript every 30s, no caching) costs 5–10x more per call-hour than the optimized version. If you're about to write code that resends the whole transcript on a timer, stop and re-read this section.

## Current phase
We are in **Phase 1–2**: capture pipeline (Windows and macOS) + trigger engine + LLM loop. Do not build the overlay UI polish or backend/CRM features yet unless explicitly asked — those are Phase 3+.

## Success criteria for this phase (don't consider it "done" without measuring these)
- Pointer latency ≤ 3s p90 (audio → hint on screen)
- Measured cost per call-hour (log actual token usage, don't estimate)
- CPU usage < 15% during an active Zoom call
- Transcript accuracy on real test calls (especially Indian-English/Hindi-English mixed audio if that's the target market)

## Conventions
- Rust: standard `rustfmt` + `clippy` clean, no warnings.
- Python backend: FastAPI + Pydantic models, SQLAlchemy/SQLModel for DB, type hints everywhere.
- Commit small, testable increments — this project is still in feasibility/PoC mode, not production polish mode.

## How we ship (industry standard — follow every time, no exceptions)
One codebase, one `main` branch, both platforms in every change, automatic checks, one release for both. Do not invent another process.

1. **Only `main` lives long.** Every change is a short branch from `main` (`feature-…`, `fix-…`) and a pull request back into it. Never create long-lived per-platform branches (no `mac`/`windows` branches): they drift apart and cause confusion.
2. **Every change covers Windows and macOS.** Shared logic is written once. Platform-specific code sits behind `#[cfg(windows)]` / `#[cfg(target_os = "macos")]`, with the other platform's version in the same pull request (or a clear "not supported on X" path). A feature isn't done until it works on both.
3. **Tests come with the change.** New logic gets unit tests; hardware or vendor paths get an `#[ignore]` live test. Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` (and `ruff`/`pytest` for the backend) before pushing.
4. **Test locally before the pull request; CI checks `main`.** Run every check locally (they must pass) before opening a pull request; GitHub doesn't run CI on feature branches or pull requests. After a merge, `.github/workflows/ci.yml` builds and tests the app on a real Windows machine plus the backend. If it goes red, fixing it is the next thing done, before any other merge. Say in the pull request what was tested and on which OS.
5. **Releases come only from a version tag on `main`.** Bump `version` in `client/src-tauri/tauri.conf.json` and `Cargo.toml` (in a pull request), then `git tag -a vX.Y.Z -m "notes for users"` and push the tag. `.github/workflows/release.yml` builds, signs and publishes Windows and macOS together; installed apps then offer the update. Same version number on both platforms. Use semantic versioning: patch = fixes, minor = features, major = breaking.
6. **No releases from laptops** except as a documented fallback (`scripts/release.ps1`, `scripts/release-mac.sh`). Never commit build outputs (`dist-mac/`, installers) or keys; signing keys live only in GitHub secrets and the release owner's backup.
7. **Server changes stay backward compatible** with app versions already installed (old apps keep working until they update).

## Consent (do not skip)
Every call must show a consent banner before capture starts, and log the consent event locally. Do not build a capture path that starts recording before this banner is acknowledged, even in test/dev builds.

## Ask before doing
- Ask before adding any new external dependency/service not listed above.
- Ask before implementing screen-share invisibility — this needs a deliberate decision on the per-window isolation approach (macOS 15+), not the simple exclusion flag.
