---
name: audio-capture-conventions
description: Use whenever writing or debugging audio capture code (WASAPI loopback on Windows, Core Audio Process Tap on macOS). Encodes known edge cases so they aren't rediscovered by trial and error.
---

# Audio Capture Conventions

## Windows — WASAPI Loopback

- **Loopback only works in shared mode.** If a device is running in exclusive mode (rare on typical laptops, but possible with pro-audio/DAW software), loopback capture will fail — detect this and surface a clear error rather than failing silently.
- **Loopback captures the default render device's output**, not necessarily the device the call app is actually using. If a user has multiple output devices (e.g., call audio to a headset, music to speakers), the capture target must track the call app's actual output device, not just "whatever is default."
- **Bluetooth output devices work fine with loopback** — no special-casing needed there.
- **Always keep the mic stream and the loopback (system audio) stream as two separate channels** — do not mix them before sending to STT. This is what makes "me" vs "them" diarization free.

## macOS — Core Audio Process Tap (macOS 14.2+)

- Requires the `com.apple.security.device.audio-input` entitlement and an `NSAudioCaptureUsageDescription` string in `Info.plist`.
- Uses a distinct TCC permission ("System Audio Recording Only") — this is **NOT** the Screen Recording permission. Do not request Screen Recording for audio-only capture; it's unnecessary and more alarming to the user.
- **There is no supported API to query permission status in advance** — you must attempt to create the tap and handle the failure case gracefully (clear user-facing error, not a crash).
- **Watch for aggregate-device cleanup errors on teardown** — always explicitly tear down the aggregate device and process tap when a call ends or the app closes; don't rely on process exit to clean it up.
- **Handle sleep/wake and audio-route changes explicitly** — do not assume the tap survives these events; detect disconnection and re-establish it.
- **For macOS versions before 14.2**, there is no process-tap fallback other than a virtual audio driver (e.g., BlackHole-style). This requires a separate install step and should be treated as a distinct, lower-priority code path — not silently assumed to work the same way.

## General

- **Call detection** (is a call currently happening?) via audio-activity heuristics and foreground app/window-title matching is inherently approximate. Log false positives/negatives during testing rather than assuming the heuristic is reliable — it will need tuning against real usage data.
- **For 3+ remote participants** merged into a single system-audio channel, do not assume diarization quality matches the clean 2-channel (mic vs. system) case — this needs direct testing against real multi-participant recordings before being trusted.
