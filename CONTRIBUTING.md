# Contributing to Sidekick

One codebase, one `main` branch, Windows and macOS in every change, checks that run by themselves,
and one release for both platforms. This is the industry-standard way (trunk-based development
with CI/CD). Follow it every time, for every change, however small.

## The flow at a glance

```
 main ──┬──────────────────────────────────┬── CI: Windows ✓ backend ✓ ── tag v0.4.0 ──▶ Windows + macOS released
        │                                  ▲
        └─ feature-streaming-hints ─ PR ───┘   (all checks pass locally first)
```

1. Branch from `main`.
2. Build the feature for **both** Windows and macOS, with tests.
3. Run all the checks locally; they must pass.
4. Open a pull request; someone reviews it.
5. Merge. CI then builds and tests `main` on a real Windows machine plus the backend.
6. Release with a version tag when ready.

GitHub doesn't run CI on feature branches or pull requests (local checks cover them, and macOS
is tested locally), which keeps CI minutes for what only GitHub can do: Windows and releases.
If CI on `main` goes red, fixing it comes before any other merge. Never keep a long-lived branch
per platform (no `mac` or `windows` branch). Never build a release on a laptop except as the
documented fallback.

## 1. Start a branch

```bash
git switch main && git pull
git switch -c feature-short-name      # or fix-short-name
```

Keep it short-lived: days, not weeks. Small pull requests are reviewed faster and break less.

## 2. Build it for both platforms

Most code is shared and is written once. Only the parts that talk to the operating system differ.

**Where things live**

| You're changing… | Look in |
|---|---|
| What triggers a hint, the trigger words | `client/src-tauri/src/trigger.rs` |
| The coaching prompt, hint format, caching | `client/src-tauri/src/coach.rs`, `llm/` |
| Speech-to-text | `client/src-tauri/src/stt/` |
| Audio capture | `client/src-tauri/src/audio/`: `wasapi_capture.rs` (Windows), `coreaudio_capture.rs` (macOS) |
| Call detection | `client/src-tauri/src/call_detect.rs` |
| Overlay window behaviour | `client/src-tauri/src/overlay.rs`; its page is `client/ui/src/app/overlay/` |
| Starting/stopping calls, consent | `capture.rs`, `consent.rs` |
| Summary, history, CRM | `call_summary.rs`, `history.rs`, `crm.rs` |
| Settings | `settings.rs`, `config.rs`; screen in `client/ui/src/app/(main)/settings/` |
| Any screen | `client/ui/src/app/` and `client/ui/src/components/` |
| Server API | `backend/app/routers/` |
| What the coach knows about us | `client/src-tauri/playbook/sales.md` |

**A new Rust function the UI can call** follows this path:

1. Write it in the right module with `#[tauri::command]`.
2. Register it in `client/src-tauri/src/lib.rs` (`invoke_handler`).
3. Add a typed wrapper in `client/ui/src/lib/sidekick.ts`.
4. Use the wrapper from the screen.

**When Windows and macOS need different code**, keep the shared part shared and put only the OS
calls behind `cfg`, with both versions in the same pull request:

```rust
/// Shared: everyone calls this.
pub fn list_mics() -> Result<Vec<InputDevice>, String> {
    platform::list_mics()
}

#[cfg(windows)]
mod platform {
    pub fn list_mics() -> Result<Vec<super::InputDevice>, String> { /* WASAPI */ }
}

#[cfg(target_os = "macos")]
mod platform {
    pub fn list_mics() -> Result<Vec<super::InputDevice>, String> { /* Core Audio */ }
}
```

`call_detect.rs` and `capture.rs` are real examples. If something truly can't work on one
platform, give it a clear "not supported on this OS" path. Never leave it to fail silently, and
say so in the pull request.

**Rules that always apply** (from [CLAUDE.md](CLAUDE.md)):

- Hints are **event-driven** and every Claude call uses **prompt caching**. Never resend the whole
  transcript on a timer.
- Nothing is captured before the **consent** step.
- Ask before adding a new external service or dependency.
- The server must keep working with app versions already installed.

## 3. Test it

- **Unit tests** for new logic, next to the code (`#[cfg(test)] mod tests`). You run them on
  your machine; CI runs them on Windows after the merge.
- **Live tests** for hardware or paid services: mark them `#[ignore]` and run them by hand, e.g.
  `cargo test coreaudio_live -- --ignored --nocapture`.
- **Backend**: add a test to `backend/tests/` for every endpoint change.
- **Try it in the app.** Use `npm run tauri dev` from `client/`, the demo call or a real call.
  For audio work, check real sound on each OS.

Before pushing:

```bash
cd client/src-tauri && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
cd client/ui && npm run typecheck
cd backend && uv run ruff check app tests && uv run ruff format app tests && uv run pytest
```

## 4. Pull request

```bash
git push -u origin feature-short-name
gh pr create --base main
```

In the description, say:
- **what** changed and **why**;
- **how it was tested**, and **on which OS** (e.g. "demo call on macOS; real Zoom call on Windows").

Merge once the local checks pass and the change has been reviewed, then delete the branch.
CI runs on `main` (Windows and backend); if it goes red, fix it straight away in a new
pull request.

### Checklist

- [ ] Works on Windows **and** macOS (or has a clear unsupported path on one, explained)
- [ ] Tests added or updated; checks pass locally
- [ ] No hint polling, prompt caching kept, consent untouched
- [ ] No keys, `.env` files, installers or `dist-mac/` committed
- [ ] Old installed apps still work against the server
- [ ] Docs updated if behaviour or setup changed

## 5. Release

Releases come from `main`, for both platforms at once.

1. In a pull request, bump `version` in `client/src-tauri/tauri.conf.json` **and**
   `client/src-tauri/Cargo.toml`. Use semantic versioning:
   - patch (`0.3.1 → 0.3.2`) for fixes;
   - minor (`0.3.2 → 0.4.0`) for features;
   - major for breaking changes.
2. Merge it, and check CI on `main` is green.
3. Tag `main`. The tag message is what users see in the update:
   ```bash
   git switch main && git pull
   git tag -a v0.4.0 -m "Hints now stream in as they're written"
   git push origin v0.4.0
   ```
4. Watch **Actions → Release** on GitHub. It:
   - checks the version numbers match;
   - builds and signs the Windows installer and the universal macOS app;
   - publishes both to the server;
   - attaches the installers to a GitHub release.

   Installed apps show "Update ready" within 15 minutes.

The signing keys and release account live in GitHub secrets. The list, and the manual fallback
scripts, are in [backend/README.md](backend/README.md#releasing-the-desktop-app). Never commit
keys and never make a new update key: installed apps only accept the existing one.

## Server changes

Merging a change under `backend/` into `main` deploys the server automatically, after its tests
pass (and after approval, if the `production` environment requires it). See
[backend/README.md](backend/README.md#deploying). Server changes must be live **before** you tag
an app release that needs them. Keep old app versions working: add fields and endpoints, don't remove
or rename ones that installed apps use.
