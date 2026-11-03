'use client';

// Team calls: everyone's calls in the workspace, as the server keeps them (summary, numbers and
// transcript; admins only). Each call says whether the coach could work: whether
// the prospect was heard, triggers fired and hints were shown. Split view follows Call history.

import { useEffect, useMemo, useState } from 'react';
import { AlertTriangle, CheckCircle2, FileText, Loader2, RefreshCw, Users } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { SummaryContent } from '@/components/CallSummaryContent';
import { backend, type TeamCall } from '@/lib/sidekick';

const MODE_LABEL: Record<string, string | undefined> = { solo_test: 'Solo test', demo: 'Demo' };

type Health = { level: 'bad' | 'warn' | 'ok'; label: string; detail: string };

/**
 * What went wrong, if anything, from the call's numbers. Only live calls long enough to judge:
 * a solo test has no prospect, and a few seconds says nothing.
 */
function health(call: TeamCall): Health | null {
  const m = call.metrics;
  if (!m || call.mode !== 'live' || call.duration_minutes < 0.3) return null;
  const themMinutes = m.stt_minutes?.them ?? null;
  const themLines = m.final_transcripts?.them ?? 0;
  if (themMinutes === 0) {
    const device = m.audio?.output_device;
    return {
      level: 'bad',
      label: 'Prospect not heard',
      detail:
        "No audio from the prospect's side reached the app, so nothing could trigger a hint. Usually the call's " +
        'sound plays on a different speaker or headset than the one Sidekick captures' +
        (device ? ` (it captured "${device}").` : '.'),
    };
  }
  if (themLines === 0) {
    return {
      level: 'bad',
      label: 'Prospect not transcribed',
      detail: "Audio from the prospect's side came in, but no words were recognised from it.",
    };
  }
  if ((m.stream_errors?.length ?? 0) > 0 || (m.errors ?? 0) > 0) {
    return { level: 'warn', label: 'Errors', detail: 'Speech-to-text or Claude failed during the call; see below.' };
  }
  if ((m.hints_shown ?? 0) === 0) {
    return {
      level: 'warn',
      label: 'No hints',
      detail: 'The prospect was heard, but no hint was shown.',
    };
  }
  return { level: 'ok', label: 'Coached', detail: 'The prospect was heard and hints were shown.' };
}

const HEALTH_STYLE = {
  bad: 'bg-red-50 text-red-700',
  warn: 'bg-amber-50 text-amber-800',
  ok: 'bg-green-50 text-green-700',
} as const;

function rep(email: string): string {
  return email.split('@')[0] || email;
}

function when(iso: string): string {
  // The server stores UTC without a zone.
  const date = new Date(iso.endsWith('Z') ? iso : `${iso}Z`);
  return date.toLocaleString(undefined, { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });
}

function title(call: TeamCall): string {
  const who = [call.prospect, call.company].filter((s) => s?.trim()).join(', ');
  return who ? `Call with ${who}` : 'Call (nobody named)';
}

function seconds(ms: number | null | undefined): string {
  return ms == null ? '–' : `${(ms / 1000).toFixed(1)} s`;
}

/** Average level while something played; none means the captured device played nothing. */
function decibels(db: number | null | undefined): string {
  return db == null ? 'Silent' : `${Math.round(db)} dB`;
}

function Stat({ label, value, alert }: { label: string; value: React.ReactNode; alert?: boolean }) {
  return (
    <div className="rounded-md bg-gray-50 px-3 py-2">
      <p className="text-[11px] uppercase tracking-wide text-gray-500">{label}</p>
      <p className={`text-sm font-semibold ${alert ? 'text-red-600' : 'text-gray-900'}`}>{value}</p>
    </div>
  );
}

function Transcript({ callId }: { callId: string }) {
  const [lines, setLines] = useState<{ channel: 'me' | 'them'; text: string }[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    backend
      .callTranscript(callId)
      .then((found) => live && setLines(found.transcript))
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
  }, [callId]);

  return (
    <section className="mt-8">
      <h3 className="mb-3 font-semibold text-gray-800">Transcript</h3>
      {error ? (
        <p className="text-sm text-gray-500">{error}</p>
      ) : lines === null ? (
        <Loader2 className="h-5 w-5 animate-spin text-gray-400" />
      ) : lines.length === 0 ? (
        <p className="text-sm text-gray-500">
          No transcript on the server for this call. Calls are saved with one from this version of the app on.
        </p>
      ) : (
        <div className="space-y-3">
          {lines.map((line, index) => (
            <div key={index} className="text-sm">
              <span
                className={`block text-xs font-semibold ${line.channel === 'them' ? 'text-blue-600' : 'text-gray-500'}`}
              >
                {line.channel === 'them' ? 'Prospect' : 'Rep'}
              </span>
              <p className="text-gray-800">{line.text}</p>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

function CallDetails({ call }: { call: TeamCall }) {
  const m = call.metrics;
  const h = health(call);
  const latency = m?.latency_ms ?? {};
  const cost = call.cost_usd ?? m?.cost_usd?.total ?? null;
  return (
    <>
      <h2 className="mb-1 text-xl font-semibold text-gray-900">{title(call)}</h2>
      <p className="mb-6 text-sm text-gray-500">
        {rep(call.user_email)} · {when(call.created_at)} · {call.duration_minutes.toFixed(1)} min
        {cost != null && ` · $${cost.toFixed(3)}`}
        {MODE_LABEL[call.mode] && ` · ${MODE_LABEL[call.mode]}`}
      </p>

      {h && (
        <div className={`mb-6 flex gap-3 rounded-lg p-4 text-sm ${HEALTH_STYLE[h.level]}`}>
          {h.level === 'ok' ? (
            <CheckCircle2 className="h-5 w-5 shrink-0" aria-hidden="true" />
          ) : (
            <AlertTriangle className="h-5 w-5 shrink-0" aria-hidden="true" />
          )}
          <div>
            <p className="font-semibold">{h.label}</p>
            <p>{h.detail}</p>
          </div>
        </div>
      )}

      {m && (
        <section className="mb-8">
          <h3 className="mb-3 font-semibold text-gray-800">What the coach got</h3>
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
            <Stat label="Rep lines" value={m.final_transcripts?.me ?? '–'} />
            <Stat
              label="Prospect lines"
              value={m.final_transcripts?.them ?? '–'}
              alert={call.mode === 'live' && (m.final_transcripts?.them ?? 0) === 0}
            />
            <Stat
              label="Prospect audio"
              value={m.stt_minutes?.them == null ? '–' : `${m.stt_minutes.them.toFixed(1)} min`}
              alert={call.mode === 'live' && m.stt_minutes?.them === 0}
            />
            <Stat label="Triggers" value={m.triggers ?? '–'} />
            <Stat label="Claude requests" value={m.claude_requests_sent ?? '–'} />
            <Stat label="Hints shown" value={m.hints_shown ?? '–'} />
            <Stat label="Opening hints" value={m.opening_hints ?? '–'} />
            <Stat label="Speech → hint p90" value={seconds(latency.speech_to_screen_p90 ?? latency.speech_to_hint_p90)} />
          </div>
          {m.audio && (
            <div className="mt-2 grid grid-cols-2 gap-2 sm:grid-cols-4">
              <Stat label="Speaker / headset" value={m.audio.output_device ?? '–'} />
              <Stat
                label="Prospect speech"
                value={m.audio.them ? `${Math.round(m.audio.them.speech_seconds)} s` : '–'}
                alert={call.mode === 'live' && m.audio.them?.speech_seconds === 0}
              />
              <Stat label="Prospect level" value={decibels(m.audio.them?.avg_db)} />
              <Stat
                label="Rep warned"
                value={m.audio.prospect_warnings ?? '–'}
                alert={(m.audio.prospect_warnings ?? 0) > 0}
              />
            </div>
          )}
          {(m.stream_errors?.length ?? 0) > 0 && (
            <ul className="mt-3 space-y-1 text-xs text-amber-800">
              {m.stream_errors!.map((error, index) => (
                <li key={index}>• {error}</li>
              ))}
            </ul>
          )}
        </section>
      )}

      {call.summary ? (
        <SummaryContent summary={call.summary} />
      ) : (
        <p className="rounded-lg bg-gray-50 p-4 text-sm text-gray-600">
          No summary for this call (too short, or it wasn&apos;t written).
        </p>
      )}
      <Transcript callId={call.call_id} />

    </>
  );
}

export default function TeamCallsPage() {
  const [calls, setCalls] = useState<TeamCall[] | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [repFilter, setRepFilter] = useState<string | null>(null);
  const [problemsOnly, setProblemsOnly] = useState(false);
  const [loading, setLoading] = useState(false);

  const load = async () => {
    setLoading(true);
    try {
      const list = await backend.teamCalls();
      setCalls(list);
      setSelectedId((current) => current ?? list[0]?.call_id ?? null);
    } catch (error) {
      toast.error(String(error));
      setCalls([]);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    load();
  }, []);

  const reps = useMemo(() => [...new Set((calls ?? []).map((c) => c.user_email))].sort(), [calls]);
  const shown = useMemo(
    () =>
      (calls ?? []).filter(
        (call) =>
          (!repFilter || call.user_email === repFilter) &&
          (!problemsOnly || ['bad', 'warn'].includes(health(call)?.level ?? '')),
      ),
    [calls, repFilter, problemsOnly],
  );
  const selected = calls?.find((c) => c.call_id === selectedId) ?? null;

  return (
    <div className="flex h-screen bg-gray-50">
      <div className="flex w-80 shrink-0 flex-col border-r border-gray-200 bg-white">
        <div className="flex items-center justify-between border-b border-gray-200 p-4">
          <span className="flex items-center gap-2 text-sm font-medium text-gray-700">
            <Users className="h-4 w-4" />
            Team calls
          </span>
          <Button variant="ghost" size="sm" onClick={load} disabled={loading} aria-label="Refresh">
            <RefreshCw className={loading ? 'animate-spin' : ''} />
          </Button>
        </div>
        <div className="flex flex-wrap gap-1 border-b border-gray-200 p-3">
          {[null, ...reps].map((email) => (
            <button
              key={email ?? 'all'}
              type="button"
              onClick={() => setRepFilter(email)}
              className={`rounded-full px-2.5 py-1 text-xs transition-colors ${
                repFilter === email ? 'bg-blue-600 text-white' : 'bg-gray-100 text-gray-600 hover:bg-gray-200'
              }`}
            >
              {email ? rep(email) : 'Everyone'}
            </button>
          ))}
          <label className="ml-auto flex items-center gap-1.5 text-xs text-gray-600">
            <input type="checkbox" checked={problemsOnly} onChange={(e) => setProblemsOnly(e.target.checked)} />
            Problems only
          </label>
        </div>
        <div className="flex-1 overflow-y-auto">
          {calls == null ? (
            <div className="flex justify-center py-12">
              <Loader2 className="h-6 w-6 animate-spin text-gray-400" />
            </div>
          ) : shown.length === 0 ? (
            <p className="px-6 py-12 text-center text-sm text-gray-500">
              No calls here yet. A call shows up when it ends on a laptop signed in to the workspace.
            </p>
          ) : (
            <ul>
              {shown.map((call) => {
                const h = health(call);
                const mode = MODE_LABEL[call.mode];
                return (
                  <li key={call.call_id}>
                    <button
                      type="button"
                      onClick={() => setSelectedId(call.call_id)}
                      className={`w-full border-b border-gray-100 px-4 py-3 text-left transition-colors ${
                        call.call_id === selectedId ? 'bg-blue-50' : 'hover:bg-gray-50'
                      }`}
                    >
                      <p className="truncate text-sm font-medium text-gray-900">{title(call)}</p>
                      <p className="mt-0.5 text-xs text-gray-500">
                        {rep(call.user_email)} · {when(call.created_at)} · {call.duration_minutes.toFixed(1)} min
                      </p>
                      {(h || mode) && (
                        <div className="mt-1.5 flex flex-wrap gap-1">
                          {mode && (
                            <span className="rounded-full bg-gray-100 px-2 py-0.5 text-[11px] text-gray-600">{mode}</span>
                          )}
                          {h && (
                            <span className={`rounded-full px-2 py-0.5 text-[11px] ${HEALTH_STYLE[h.level]}`}>
                              {h.label}
                            </span>
                          )}
                        </div>
                      )}
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </div>

      <div className="flex min-w-0 flex-1 flex-col overflow-hidden bg-white">
        <div className="flex-1 overflow-y-auto">
          <div className="mx-auto w-full max-w-3xl p-6">
            {selected ? (
              <CallDetails key={selected.call_id} call={selected} />
            ) : (
              <div className="flex flex-col items-center justify-center py-24 text-center">
                <FileText className="mb-4 h-16 w-16 text-gray-300" />
                <p className="text-sm text-gray-500">Select a call to see how it went.</p>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
