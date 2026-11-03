'use client';

// Call state for the whole UI: consent, live transcript, hints, triggers, levels, and the
// summaries after a call. Replaces Meetily's RecordingState/Transcript/Config contexts.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { usePathname, useRouter } from 'next/navigation';
import { toast } from 'sonner';
import {
  backend,
  onBackend,
  shareOpeningTick,
  type CallBrief,
  type CallHistory,
  type DetectedCall,
  type CallStats,
  type CallSummaryStatus,
  type Channel,
  type HintEvent,
  type HintRating,
  type OpeningHint,
  type TriggerEvent,
} from '@/lib/sidekick';
import type { TranscriptSegmentData } from '@/types';

export type CallStatus = 'idle' | 'live' | 'finishing';
export type CallMode = 'live' | 'solo_test' | 'demo';

export interface TimedHint extends HintEvent {
  receivedAt: number;
  rating: HintRating | null;
}

/** An opening hint the rep can tick off once it's covered. */
export interface ChecklistHint extends OpeningHint {
  done: boolean;
}

export interface TimedTrigger extends TriggerEvent {
  id: string;
  receivedAt: number;
}

interface CallContextValue {
  status: CallStatus;
  mode: CallMode | null;
  /** When the current (or last) call started, ms since epoch. */
  startedAt: number | null;
  consented: boolean;
  /** Paused calls stay open but aren't transcribed or coached. */
  paused: boolean;
  /** Seconds of the call so far, excluding time spent paused. */
  activeSeconds: () => number;
  segments: TranscriptSegmentData[];
  hints: TimedHint[];
  /** The live hint Claude is still writing, word by word; becomes a hint when it's done. */
  draftHint: { id: string; text: string } | null;
  /** What to raise in this call, given when it starts; ticked off by the rep. */
  openingHints: ChecklistHint[];
  toggleOpeningHint: (id: string) => void;
  triggers: TimedTrigger[];
  levels: Record<Channel, number>;
  stats: CallStats | null;
  callSummary: CallSummaryStatus | null;
  soloTest: boolean;
  setSoloTest: (value: boolean) => void;
  acknowledgeConsent: (name: string, label: string) => Promise<void>;
  /** A call app using the mic that the rep hasn't started coaching or dismissed. */
  detectedCall: DetectedCall | null;
  dismissDetectedCall: () => Promise<void>;
  /** Details entered before the current (or last) call. */
  brief: CallBrief | null;
  /** `history`: the CRM record and earlier calls, still being read; capture doesn't wait. */
  startCall: (brief?: CallBrief, history?: Promise<CallHistory>) => Promise<void>;
  startDemoCall: () => Promise<void>;
  stopCall: () => Promise<void>;
  togglePause: () => Promise<void>;
  /** Rates a hint, or clears the rating with null. */
  rateHint: (hint: TimedHint, rating: HintRating | null) => Promise<void>;
}

const CallContext = createContext<CallContextValue | null>(null);

const SUMMARY_PATH = '/summary/';
const SUMMARY_POLL_MS = 500;
const SUMMARY_POLL_LIMIT = 240;

function readSoloTest(): boolean {
  try {
    return localStorage.getItem('soloTest') === 'true';
  } catch {
    return false;
  }
}

export function CallProvider({ children }: { children: React.ReactNode }) {
  const [status, setStatus] = useState<CallStatus>('idle');
  const [mode, setMode] = useState<CallMode | null>(null);
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [consented, setConsented] = useState(false);
  const [finals, setFinals] = useState<TranscriptSegmentData[]>([]);
  const [interim, setInterim] = useState<Record<Channel, string>>({ me: '', them: '' });
  const [hints, setHints] = useState<TimedHint[]>([]);
  const [draftHint, setDraftHint] = useState<{ id: string; text: string } | null>(null);
  const [openingHints, setOpeningHints] = useState<ChecklistHint[]>([]);
  const [triggers, setTriggers] = useState<TimedTrigger[]>([]);
  const [levels, setLevels] = useState<Record<Channel, number>>({ me: 0, them: 0 });
  const [stats, setStats] = useState<CallStats | null>(null);
  const [callSummary, setCallSummary] = useState<CallSummaryStatus | null>(null);
  const [soloTest, setSoloTestState] = useState(false);
  const [paused, setPaused] = useState(false);
  const [brief, setBrief] = useState<CallBrief | null>(null);
  // Read inside the backend push, which runs after the call is over.
  const briefRef = useRef<CallBrief | null>(null);
  briefRef.current = brief;
  const [detectedCall, setDetectedCall] = useState<DetectedCall | null>(null);
  const pausedTotalMs = useRef(0);
  const pausedSince = useRef<number | null>(null);

  const startedAtRef = useRef<number | null>(null);
  // Backend ID of the current (or last) call; null while a new call is starting. Late
  // results from an earlier call are ignored once a newer one has begun.
  const sessionIdRef = useRef<number | null>(null);
  const pollingIdRef = useRef<number | null>(null);
  const idRef = useRef(0);
  const nextId = () => String(++idRef.current);

  useEffect(() => setSoloTestState(readSoloTest()), []);

  const setSoloTest = useCallback((value: boolean) => {
    setSoloTestState(value);
    try {
      localStorage.setItem('soloTest', String(value));
    } catch {}
  }, []);

  const secondsIntoCall = () =>
    startedAtRef.current == null ? 0 : (Date.now() - startedAtRef.current) / 1000;

  const applyPaused = useCallback((next: boolean) => {
    const now = Date.now();
    if (next && pausedSince.current == null) pausedSince.current = now;
    if (!next && pausedSince.current != null) {
      pausedTotalMs.current += now - pausedSince.current;
      pausedSince.current = null;
    }
    setPaused(next);
  }, []);

  const activeSeconds = useCallback(() => {
    if (startedAtRef.current == null) return 0;
    const now = Date.now();
    const pausedNow = pausedSince.current == null ? 0 : now - pausedSince.current;
    return Math.max(0, (now - startedAtRef.current - pausedTotalMs.current - pausedNow) / 1000);
  }, []);

  const resetForNewCall = useCallback((newMode: CallMode) => {
    const now = Date.now();
    pausedTotalMs.current = 0;
    pausedSince.current = null;
    setPaused(false);
    sessionIdRef.current = null;
    startedAtRef.current = now;
    setStartedAt(now);
    setMode(newMode);
    setFinals([]);
    setInterim({ me: '', them: '' });
    setHints([]);
    setDraftHint(null);
    setOpeningHints([]);
    setTriggers([]);
    setStats(null);
    setCallSummary(null);
    setStatus('live');
  }, []);

  // The backend is optional: a failed push is logged, never shown mid-call, and the call stays
  // saved on this computer either way.
  const pushToBackend = useCallback((session: CallStats, call: CallSummaryStatus) => {
    const file = 'file' in call ? call.file : null;
    const callId = file?.match(/(call-\d+)\.md$/)?.[1];
    if (!callId) return;
    backend
      .backendStatus()
      .then((status) => {
        if (!status.signed_in) return;
        return backend.pushCallToBackend({
          call_id: callId,
          prospect: briefRef.current?.prospect ?? '',
          company: briefRef.current?.company ?? '',
          mode: session.mode,
          duration_minutes: session.duration_minutes,
          cost_usd: session.cost_usd.total,
          crm_record_url: null,
          crm_person_id: briefRef.current?.crm_person_id ?? null,
          crm_company_id: briefRef.current?.crm_company_id ?? null,
          summary: call.status === 'ready' ? call.summary : null,
          metrics: session,
        });
      })
      .catch((error) => console.error('Could not send the call to the backend:', error));
  }, []);

  const router = useRouter();
  const pathname = usePathname();
  // Read inside callbacks without making them depend on the current page.
  const pathnameRef = useRef(pathname);
  pathnameRef.current = pathname;

  // After a call ends, stats arrive within ~5 s and the written summary shortly after.
  // Stops quietly if another call starts meanwhile, so it can't touch the new call's state.
  const awaitSummaries = useCallback(async (sessionId: number) => {
    if (pollingIdRef.current === sessionId) return;
    if (sessionIdRef.current !== null && sessionIdRef.current > sessionId) return;
    sessionIdRef.current = sessionId;
    pollingIdRef.current = sessionId;
    const superseded = () => sessionIdRef.current !== sessionId;
    setStatus('finishing');
    // The call is over, so show the summary being written instead of a frozen live screen.
    if (pathnameRef.current !== SUMMARY_PATH) router.push(SUMMARY_PATH);
    try {
      for (let attempt = 0; attempt < SUMMARY_POLL_LIMIT; attempt += 1) {
        const { session_id, session, call } = await backend.lastSummaries();
        if (superseded() || session_id !== sessionId) return;
        if (session) setStats(session);
        if (call) setCallSummary(call);
        if (session) setStatus('idle');
        if (session && call && call.status !== 'pending') {
          if (call.status === 'ready') toast.success('Call summary ready');
          pushToBackend(session, call);
          return;
        }
        await new Promise((resolve) => setTimeout(resolve, SUMMARY_POLL_MS));
      }
      toast.error("The call summary didn't arrive. The transcript is saved in the calls folder.");
    } finally {
      if (!superseded()) setStatus('idle');
      if (pollingIdRef.current === sessionId) pollingIdRef.current = null;
    }
  }, [router]);

  useEffect(() => {
    backend.consentStatus().then(setConsented).catch(() => {});
    backend.detectedCall().then(setDetectedCall).catch(() => {});
    backend
      .captureStatus()
      .then(async (running) => {
        if (!running) return;
        setStatus('live');
        // The window reloaded mid-call: bring back the clock and what was said so far.
        const live = await backend.liveTranscript();
        if (!live) return;
        const started = live.started_at_unix_ms;
        startedAtRef.current = started;
        setStartedAt(started);
        const restored = live.lines.map((line) => ({
          id: nextId(),
          timestamp: Math.max(0, (line.at_unix_ms - started) / 1000),
          text: line.text,
          speaker: line.channel,
        }));
        const key = (speaker: string | undefined, text: string) => `${speaker}\u0000${text}`;
        const known = new Set(restored.map((r) => key(r.speaker, r.text)));
        // Lines that arrived while this was loading are in both: keep one copy.
        setFinals((prev) => [...restored, ...prev.filter((p) => !known.has(key(p.speaker, p.text)))]);
      })
      .catch(() => {});

    const subscriptions = [
      onBackend('transcript', (t) => {
        if (t.is_final) {
          setFinals((prev) => [
            ...prev,
            { id: nextId(), timestamp: secondsIntoCall(), text: t.text, speaker: t.channel },
          ]);
          setInterim((prev) => ({ ...prev, [t.channel]: '' }));
        } else {
          setInterim((prev) => ({ ...prev, [t.channel]: t.text }));
        }
      }),
      onBackend('hint-delta', (draft) => setDraftHint(draft)),
      onBackend('hint-discarded', ({ id }) => setDraftHint((d) => (d?.id === id ? null : d))),
      onBackend('hint', (h) => {
        setDraftHint((d) => (d?.id === h.id ? null : d));
        setHints((prev) => [{ ...h, receivedAt: Date.now(), rating: null }, ...prev]);
      }),
      onBackend('opening-hints', (list) => setOpeningHints(list.map((h) => ({ ...h, done: false })))),
      // Covered in the conversation (or ticked in another window): off the list.
      onBackend('opening-covered', (ids) =>
        setOpeningHints((prev) => prev.map((h) => (ids.includes(h.id) ? { ...h, done: true } : h))),
      ),
      onBackend('opening-reopened', (ids) =>
        setOpeningHints((prev) => prev.map((h) => (ids.includes(h.id) ? { ...h, done: false } : h))),
      ),
      // Ratings can come from the overlay too.
      onBackend('hint-rated', ({ hint_id, rating }) =>
        setHints((prev) => prev.map((h) => (h.id === hint_id ? { ...h, rating } : h))),
      ),
      onBackend('trigger', (t) =>
        setTriggers((prev) => [{ ...t, id: nextId(), receivedAt: Date.now() }, ...prev]),
      ),
      onBackend('audio-level', (l) => setLevels((prev) => ({ ...prev, [l.channel]: l.rms }))),
      onBackend('capture-error', (e) =>
        toast.error(`${e.channel === 'me' ? 'Microphone' : 'System audio'}: ${e.message}`),
      ),
      onBackend('coach-error', (message) => toast.error(`Coach: ${message}`)),
      onBackend('call-paused', ({ paused: next }) => applyPaused(next)),
      onBackend('call-detected', setDetectedCall),
      onBackend('call-detection-cleared', ({ key }) =>
        setDetectedCall((current) => (current?.key === key ? null : current)),
      ),
      // One toast per channel that updates in place, instead of stacking during an outage.
      onBackend('stt-status', ({ channel, state, retry_in_secs }) => {
        const source = channel === 'me' ? 'Your mic' : 'Prospect audio';
        const id = `stt-${channel}`;
        if (state === 'reconnecting') {
          const when = retry_in_secs ? ` (retry in ${retry_in_secs}s)` : '';
          toast.loading(`${source}: transcription reconnecting${when}...`, { id });
        } else {
          toast.success(`${source}: transcription reconnected`, { id, duration: 3000 });
        }
      }),
      onBackend('call-ending-soon', ({ message }) => {
        if (message) toast.warning(message, { id: 'call-ending-soon', duration: Infinity });
        else toast.dismiss('call-ending-soon');
      }),
      onBackend('prospect-not-heard', ({ message }) => {
        if (message) toast.warning(message, { id: 'prospect-not-heard', duration: Infinity });
        else toast.dismiss('prospect-not-heard');
      }),
      // Stopped somewhere else, e.g. the overlay's Stop button.
      onBackend('call-stopped', ({ session_id, reason }) => {
        toast.dismiss('call-ending-soon');
        toast.dismiss('prospect-not-heard');
        if (reason) toast.info(reason, { duration: 15000 });
        applyPaused(false);
        setConsented(false);
        setLevels({ me: 0, them: 0 });
        awaitSummaries(session_id);
      }),
      // A demo call ends on its own when its script finishes.
      onBackend('session-ended', ({ session_id }) => {
        setLevels({ me: 0, them: 0 });
        awaitSummaries(session_id);
      }),
    ];
    return () => {
      subscriptions.forEach((s) => s.then((unlisten) => unlisten()));
    };
  }, [awaitSummaries, applyPaused]);

  const acknowledgeConsent = useCallback(async (name: string, label: string) => {
    await backend.acknowledgeConsent(name, label);
    setConsented(true);
  }, []);

  const startCall = useCallback(async (callBrief?: CallBrief, history?: Promise<CallHistory>) => {
    const newMode: CallMode = soloTest ? 'solo_test' : 'live';
    resetForNewCall(newMode);
    setBrief(callBrief ?? null);
    const callApp = detectedCall;
    setDetectedCall(null);
    try {
      const sessionId = await backend.startCapture(soloTest, callBrief ?? null, callApp, history != null);
      sessionIdRef.current = sessionId;
      // Recording has started; the history follows as soon as it's read.
      history
        ?.then((found) => {
          setBrief((current) => (current ? { ...current, ...found } : current));
          return backend.provideCallHistory(sessionId, found);
        })
        .catch(() => {});
    } catch (error) {
      setStatus('idle');
      toast.error(String(error));
    }
    // Consent is used up by a started call and expires if none starts.
    backend.consentStatus().then(setConsented).catch(() => {});
  }, [soloTest, resetForNewCall, detectedCall]);

  const dismissDetectedCall = useCallback(async () => {
    if (!detectedCall) return;
    setDetectedCall(null);
    await backend.dismissDetectedCall(detectedCall.key).catch(() => {});
  }, [detectedCall]);

  const startDemoCall = useCallback(async () => {
    resetForNewCall('demo');
    setBrief(null);
    try {
      sessionIdRef.current = await backend.startDemoCall();
    } catch (error) {
      setStatus('idle');
      toast.error(String(error));
    }
  }, [resetForNewCall]);

  const rateHint = useCallback(async (hint: TimedHint, rating: HintRating | null) => {
    try {
      await backend.rateHint(hint.id, hint.text, rating);
    } catch (error) {
      toast.error(String(error));
    }
  }, []);

  const toggleOpeningHint = useCallback((id: string) => {
    setOpeningHints((prev) =>
      prev.map((h) => {
        if (h.id !== id) return h;
        shareOpeningTick(id, !h.done);
        return { ...h, done: !h.done };
      }),
    );
  }, []);

  const togglePause = useCallback(async () => {
    try {
      await backend.setCallPaused(!paused);
    } catch (error) {
      toast.error(String(error));
    }
  }, [paused]);

  const stopCall = useCallback(async () => {
    applyPaused(false);
    const sessionId = await backend.stopCapture();
    setConsented(false);
    setLevels({ me: 0, them: 0 });
    // Nothing was running (e.g. a demo that had just finished): no summary to wait for.
    if (sessionId === null) {
      if (pollingIdRef.current === null) setStatus('idle');
      return;
    }
    awaitSummaries(sessionId);
  }, [awaitSummaries, applyPaused]);

  const segments = useMemo(() => {
    const live: TranscriptSegmentData[] = (['me', 'them'] as Channel[])
      .filter((channel) => interim[channel])
      .map((channel) => ({
        id: `interim-${channel}`,
        timestamp: secondsIntoCall(),
        text: interim[channel],
        speaker: channel,
      }));
    return [...finals, ...live];
  }, [finals, interim]);

  const value: CallContextValue = {
    status,
    mode,
    startedAt,
    consented,
    paused,
    activeSeconds,
    segments,
    hints,
    draftHint,
    openingHints,
    toggleOpeningHint,
    triggers,
    levels,
    stats,
    callSummary,
    soloTest,
    setSoloTest,
    acknowledgeConsent,
    brief,
    detectedCall,
    dismissDetectedCall,
    startCall,
    startDemoCall,
    stopCall,
    togglePause,
    rateHint,
  };

  return <CallContext.Provider value={value}>{children}</CallContext.Provider>;
}

export function useCall(): CallContextValue {
  const context = useContext(CallContext);
  if (!context) throw new Error('useCall must be used inside CallProvider');
  return context;
}
