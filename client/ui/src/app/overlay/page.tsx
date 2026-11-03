'use client';

// The hint overlay window: Anarlog's floating bar (MIT, see ANARLOG-LICENSE.md) driven by
// Sidekick's backend events.

import { useEffect, useMemo, useRef, useState } from 'react';
import { backend, onBackend, reportPainted, type Channel, type DetectedCall } from '@/lib/sidekick';
import { useTheme } from '@/lib/theme';
import {
  FloatingHintsOverlay,
  type ColorScheme,
  type OverlayHint,
  type TranscriptBubbleData,
} from '@/components/overlay/FloatingHintsOverlay';

const TITLES = { live: 'Live call', solo_test: 'Solo test', demo: 'Demo call' } as const;
const MAX_BUBBLES = 40;
const EXPANDED_KEY = 'overlayExpanded';

// RMS to a 0–1 level on a -60..0 dBFS scale, so normal speech moves the sticks.
function levelFromRms(rms: number): number {
  const db = 20 * Math.log10(Math.max(rms, 1e-6));
  return Math.min(1, Math.max(0, (db + 60) / 60));
}

function withInterim(bubbles: TranscriptBubbleData[], interim: Record<Channel, string>): TranscriptBubbleData[] {
  const live = (['them', 'me'] as const)
    .filter((channel) => interim[channel].trim())
    .map((channel) => ({
      id: `interim-${channel}`,
      text: interim[channel],
      isSelf: channel === 'me',
      speakerLabel: channel === 'me' ? 'You' : 'Prospect',
      interim: true,
    }));
  return live.length ? [...bubbles, ...live] : bubbles;
}

function readExpanded(): boolean {
  try {
    return localStorage.getItem(EXPANDED_KEY) !== 'false';
  } catch {
    return true;
  }
}

export default function OverlayPage() {
  const [ready, setReady] = useState(false);
  const [title, setTitle] = useState<string>(TITLES.live);
  const [expanded, setExpanded] = useState(true);
  // Follows the app theme (System / Light / Dark), shared with the main window.
  const colorScheme: ColorScheme = useTheme().resolved;
  const [hint, setHint] = useState<OverlayHint | null>(null);
  const lastHintRef = useRef<OverlayHint | null>(null);
  const [opening, setOpening] = useState<{ id: string; text: string; covered: boolean }[]>([]);
  const [bubbles, setBubbles] = useState<TranscriptBubbleData[]>([]);
  // Words still being recognised, per side; shown as the last bubble and replaced when the line
  // settles, so text appears while people speak instead of after they pause.
  const [interim, setInterim] = useState<Record<Channel, string>>({ me: '', them: '' });
  const [amplitude, setAmplitude] = useState(0);
  const [paused, setPaused] = useState(false);
  const [detected, setDetected] = useState<DetectedCall | null>(null);
  const [callRunning, setCallRunning] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  // Can't hear the prospect: shown over the end-of-call notice, since it stops hints now.
  const [hearing, setHearing] = useState<string | null>(null);
  const levels = useRef<Record<Channel, number>>({ me: 0, them: 0 });
  const idRef = useRef(0);

  useEffect(() => {
    // The window is transparent; only the floating surface should paint. Dark in every theme
    // (see THEME_BOOT in the root layout), or a light scheme paints the window white.
    document.documentElement.style.colorScheme = 'dark';
    document.documentElement.style.background = 'transparent';
    document.body.style.background = 'transparent';

    const initialExpanded = readExpanded();
    setExpanded(initialExpanded);
    backend.overlaySetExpanded(initialExpanded).catch(() => {});
    setReady(true);

    backend.detectedCall().then(setDetected).catch(() => {});
    backend.captureStatus().then(setCallRunning).catch(() => {});

    const nextId = () => String(++idRef.current);
    const subscriptions = [
      onBackend('call-detected', setDetected),
      onBackend('call-detection-cleared', ({ key }) => setDetected((current) => (current?.key === key ? null : current))),
      // A finished call's hint, checklist and transcript don't stay behind to look like a live
      // call the next time the overlay opens.
      onBackend('session-ended', () => {
        setCallRunning(false);
        setHint(null);
        setOpening([]);
        setBubbles([]);
        setInterim({ me: '', them: '' });
      }),
      onBackend('session-started', ({ mode }) => {
        setCallRunning(true);
        setDetected(null);
        setTitle(TITLES[mode] ?? TITLES.live);
        setPaused(false);
        setNotice(null);
        setHearing(null);
        setHint(null);
        lastHintRef.current = null;
        setOpening([]);
        setBubbles([]);
        setInterim({ me: '', them: '' });
      }),
      onBackend('opening-hints', (list) => {
        setOpening(list.map((h) => ({ id: h.id, text: h.text, covered: false })));
        if (list[0]) reportPainted(list[0].id);
      }),
      // Covered items leave the overlay; reopened ones come back.
      onBackend('opening-covered', (ids) =>
        setOpening((prev) => prev.map((h) => (ids.includes(h.id) ? { ...h, covered: true } : h))),
      ),
      onBackend('opening-reopened', (ids) =>
        setOpening((prev) => prev.map((h) => (ids.includes(h.id) ? { ...h, covered: false } : h))),
      ),
      // The words appear as Claude writes them; the finished hint (same id) replaces them. Words
      // for a hint that won't come give the screen back to the previous hint.
      onBackend('hint-delta', ({ id, text }) =>
        setHint((current) => ({
          id,
          kind: current?.id === id ? current.kind : 'fact',
          urgency: current?.id === id ? current.urgency : 'medium',
          text,
          rating: null,
          shownAt: Date.now(),
        })),
      ),
      onBackend('hint-discarded', ({ id }) =>
        setHint((current) => (current?.id === id ? lastHintRef.current : current)),
      ),
      onBackend('hint', (h) => {
        const finished = { id: h.id, kind: h.kind, urgency: h.urgency, text: h.text, rating: null, shownAt: Date.now() };
        lastHintRef.current = finished;
        setHint(finished);
        reportPainted(h.id);
      }),
      onBackend('hint-rated', ({ hint_id, rating }) =>
        setHint((current) => (current?.id === hint_id ? { ...current, rating } : current)),
      ),
      onBackend('transcript', (t) => {
        if (!t.is_final) {
          setInterim((prev) => (prev[t.channel] === t.text ? prev : { ...prev, [t.channel]: t.text }));
          return;
        }
        setInterim((prev) => (prev[t.channel] === '' ? prev : { ...prev, [t.channel]: '' }));
        if (!t.text.trim()) return;
        setBubbles((prev) =>
          [
            ...prev,
            {
              id: nextId(),
              text: t.text,
              isSelf: t.channel === 'me',
              speakerLabel: t.channel === 'me' ? 'You' : 'Prospect',
            },
          ].slice(-MAX_BUBBLES),
        );
      }),
      onBackend('call-paused', ({ paused: next }) => setPaused(next)),
      onBackend('call-ending-soon', ({ message }) => setNotice(message)),
      onBackend('prospect-not-heard', ({ message }) => setHearing(message)),
      onBackend('call-stopped', () => {
        setNotice(null);
        setHearing(null);
        setPaused(false);
        setCallRunning(false);
        setHint(null);
        setOpening([]);
        setBubbles([]);
        setInterim({ me: '', them: '' });
      }),
      onBackend('audio-level', (l) => {
        levels.current[l.channel] = levelFromRms(l.rms);
      }),
    ];

    // Sticks follow the louder channel, decaying smoothly between audio chunks.
    const decay = setInterval(() => {
      const target = Math.max(levels.current.me, levels.current.them);
      setAmplitude((current) => (target > current ? target : Math.max(0, current - 0.08)));
      levels.current = { me: levels.current.me * 0.6, them: levels.current.them * 0.6 };
    }, 100);

    return () => {
      clearInterval(decay);
      subscriptions.forEach((s) => s.then((unlisten) => unlisten()));
    };
  }, []);

  const shownBubbles = useMemo(() => withInterim(bubbles, interim), [bubbles, interim]);
  const openingLeft = useMemo(() => opening.filter((h) => !h.covered).map((h) => h.text), [opening]);

  if (!ready) {
    return <div className="h-screen w-screen bg-transparent" />;
  }

  return (
    <div className="h-screen w-screen bg-transparent">
      <FloatingHintsOverlay
        state={{
          title: detected && !callRunning ? 'Call detected' : paused ? `${title} · Paused` : title,
          detectedApp: callRunning ? null : (detected?.app ?? null),
          amplitude: paused ? 0 : amplitude,
          expanded,
          opacity: 0.95,
          colorScheme,
          hint,
          opening: openingLeft,
          bubbles: shownBubbles,
          notice: callRunning ? (hearing ?? notice) : null,
        }}
        onStartDetected={() => {
          backend.requestStartCall().catch(() => {});
        }}
        onDismissDetected={() => {
          if (detected) backend.dismissDetectedCall(detected.key).catch(() => {});
          setDetected(null);
        }}
        onRateHint={(rated, rating) => {
          backend.rateHint(rated.id, rated.text, rating).catch(() => {});
        }}
        onStop={() => {
          backend.stopCapture().catch(() => {});
        }}
        onAsk={() => {
          backend.askCoach(null).catch(() => {});
        }}
        onToggleExpanded={(next) => {
          setExpanded(next);
          try {
            localStorage.setItem(EXPANDED_KEY, String(next));
          } catch {}
          backend.overlaySetExpanded(next).catch(() => {});
        }}
      />
    </div>
  );
}
