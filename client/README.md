# Sidekick desktop app

The Windows and macOS app: `src-tauri/` is the Rust engine (Tauri 2), `ui/` the Next.js screens.

- Overview, setup and running it: [../README.md](../README.md)
- Adding a feature for both platforms, testing, releasing: [../CONTRIBUTING.md](../CONTRIBUTING.md)

```bash
npm install && (cd ui && npm install)
cp src-tauri/.env.example src-tauri/.env   # then add keys, or set SIDEKICK_BACKEND_URL
npm run tauri dev                          # run the app
npm run tauri:ui-dev                       # with hot reload for UI work
```
