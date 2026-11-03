'use client';

// Light, dark, or follow Windows. The choice is saved in localStorage, which both windows
// share, so the main window and the hint overlay switch together. The root layout applies
// it before first paint.

import { useCallback, useEffect, useState } from 'react';

export type ThemeChoice = 'system' | 'light' | 'dark';
export type ResolvedTheme = 'light' | 'dark';

const KEY = 'sidekick:theme';

function readChoice(): ThemeChoice {
  try {
    const saved = localStorage.getItem(KEY);
    return saved === 'light' || saved === 'dark' ? saved : 'system';
  } catch {
    return 'system';
  }
}

function resolve(choice: ThemeChoice): ResolvedTheme {
  if (choice !== 'system') return choice;
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

export function useTheme() {
  const [choice, setChoiceState] = useState<ThemeChoice>('system');
  const [resolved, setResolved] = useState<ResolvedTheme>('light');

  useEffect(() => {
    const apply = () => {
      const current = readChoice();
      const next = resolve(current);
      document.documentElement.classList.toggle('dark', next === 'dark');
      setChoiceState(current);
      setResolved(next);
    };
    apply();
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    // Windows switching theme, or the other window changing the choice.
    const onStorage = (event: StorageEvent) => event.key === KEY && apply();
    media.addEventListener('change', apply);
    window.addEventListener('storage', onStorage);
    window.addEventListener('sidekick:theme', apply);
    return () => {
      media.removeEventListener('change', apply);
      window.removeEventListener('storage', onStorage);
      window.removeEventListener('sidekick:theme', apply);
    };
  }, []);

  const setChoice = useCallback((next: ThemeChoice) => {
    try {
      if (next === 'system') localStorage.removeItem(KEY);
      else localStorage.setItem(KEY, next);
    } catch {}
    // `storage` only fires in other windows; tell this one too.
    window.dispatchEvent(new Event('sidekick:theme'));
  }, []);

  return { choice, resolved, setChoice };
}
