# Third-party notices

Parts of this UI are copied or adapted from **Meetily** (https://github.com/Zackriya-Solutions/meetily),
Copyright (c) 2024 Zackriya Solutions, released under the MIT License. The full license text is in
`MEETILY-LICENSE.md`.

Copied or adapted from Meetily:
- `tailwind.config.js`, `postcss.config.js`, `components.json`, `tsconfig.json`, `src/app/globals.css`
- `src/lib/utils.ts`, `src/components/ui/*` (shadcn/ui components as configured by Meetily)
- `src/components/RecordingStatusBar.tsx`, `src/components/VirtualizedTranscriptView.tsx`,
  `src/components/ConfidenceIndicator.tsx`, `src/hooks/useAutoScroll.ts`, `src/hooks/useTranscriptStreaming.ts`
- Layout and styling of the recording controls and transcript panel

---

The hint overlay (`src/app/overlay/`, `src/components/overlay/`) is copied or adapted from **Anarlog**
(https://github.com/fastrepl/anarlog), Copyright (c) 2023-present Fastrepl, Inc., released under the MIT License.
Only MIT-licensed paths were used (nothing from `enterprise/`). The full license text is in `ANARLOG-LICENSE.md`.

Copied or adapted from Anarlog:
- `apps/desktop/src/meeting-float/overlay/bar.tsx` (floating bar, stop control, hover drag handle, transcript bubbles, colors)
- `apps/desktop/src/meeting-float/overlay/layout.ts` (sizes and radii)
- `packages/ui/src/components/ui/dancing-sticks.tsx` and its `anarlog-dancing-stick` keyframes

---

**Google Sans** font files in `public/fonts/google-sans/` are by Google, licensed under the SIL Open Font
License 1.1 (`public/fonts/google-sans/OFL.txt`).

---

The main-window sidebar uses **shadcn/ui** components added with `npx shadcn@latest add sidebar`
(`src/components/ui/sidebar.tsx`, `sheet.tsx`, `skeleton.tsx`, `src/hooks/use-mobile.tsx`),
https://github.com/shadcn-ui/ui, released under the MIT License.

---

The live call transcript and call controls use components from **ElevenLabs UI** (https://github.com/elevenlabs/ui),
Copyright (c) 2025 Eleven Labs Inc., released under the MIT License. The full license text is in
`ELEVENLABS-UI-LICENSE.md`.

Copied or adapted from ElevenLabs UI:
- `apps/www/registry/elevenlabs-ui/ui/conversation.tsx` → `src/components/ui/conversation.tsx`
- `apps/www/registry/elevenlabs-ui/ui/message.tsx` → `src/components/ui/message.tsx`
- `apps/www/registry/elevenlabs-ui/ui/orb.tsx` → `src/components/ui/orb.tsx` (with its `perlin-noise.png`, now `public/orb/perlin-noise.png`)
- `apps/www/registry/elevenlabs-ui/ui/mic-selector.tsx` → `src/components/ui/mic-selector.tsx`
- The word rendering of `apps/www/registry/elevenlabs-ui/ui/transcript-viewer.tsx` → `src/components/ui/transcript-words.tsx`

`src/components/ui/dropdown-menu.tsx` was added with `npx shadcn@latest add dropdown-menu` (MIT).
