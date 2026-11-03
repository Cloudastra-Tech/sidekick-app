// How old the hint on screen is. A hint stays until the next one, so when the coach has nothing
// new the same words sit there; dimming them with their age shows the rep they're from earlier,
// not what to say right now.

import { useEffect, useState } from 'react';

/** Older than this, a hint is from an earlier moment of the call. */
export const STALE_HINT_MS = 45_000;

/** "1 min ago" once a hint is stale, else null. */
export function hintAgeLabel(shownAt: number | undefined, now: number): string | null {
  if (shownAt == null) return null;
  const age = now - shownAt;
  if (age < STALE_HINT_MS) return null;
  return `${Math.max(1, Math.round(age / 60_000))} min ago`;
}

/** The current time, updated every few seconds, for age labels. */
export function useNow(intervalMs = 5_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(timer);
  }, [intervalMs]);
  return now;
}
