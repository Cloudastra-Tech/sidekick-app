'use client';

// Rep performance, for admins: each rep's calls over a period. The conversation measures are the
// ones call-intelligence tools report (talk ratio, longest monologue, questions, back-and-forth),
// with the published benchmarks beside them; the coaching measures say whether Sidekick could
// help (prospect heard, hints shown, how fast). Every value is printed; meters only add shape.

import { useEffect, useMemo, useState } from 'react';
import { AlertTriangle, CheckCircle2, CircleDashed, Loader2, TrendingUp } from 'lucide-react';
import { toast } from 'sonner';
import { backend, type RepStats } from '@/lib/sidekick';

const PERIODS = [7, 30, 90] as const;

/**
 * Benchmarks from published call-intelligence research on won B2B discovery calls: top reps talk
 * about 43% of the time, ask 11–14 questions, and keep a monologue under ~2.5 minutes (about 375
 * words at a typical 150 words a minute).
 */
const TALK_RATIO_TARGET = 0.43;
const TALK_RATIO_OK = [0.35, 0.55] as const;
const QUESTIONS_TARGET = [11, 14] as const;
const MONOLOGUE_MAX_WORDS = 375;

type Status = 'good' | 'warning' | 'none';

const STATUS_ICON = {
  good: <CheckCircle2 className="h-3.5 w-3.5 text-green-600" aria-hidden="true" />,
  warning: <AlertTriangle className="h-3.5 w-3.5 text-amber-600" aria-hidden="true" />,
  none: <CircleDashed className="h-3.5 w-3.5 text-gray-300" aria-hidden="true" />,
};

const pct = (value: number | null) => (value == null ? '–' : `${Math.round(value * 100)}%`);
const name = (email: string) => email.split('@')[0] || email;

function when(iso: string): string {
  const date = new Date(iso.endsWith('Z') ? iso : `${iso}Z`);
  return date.toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
}

/** A value with a thin meter behind it and, where there is one, the benchmark marked. */
function Meter({
  value,
  max,
  label,
  target,
  status,
  hint,
}: {
  value: number | null;
  max: number;
  label: string;
  target?: number;
  status: Status;
  hint: string;
}) {
  const width = value == null ? 0 : Math.min(100, (value / max) * 100);
  return (
    <div className="min-w-[96px]" title={hint}>
      <div className="flex items-center gap-1 text-sm tabular-nums text-gray-900">
        {STATUS_ICON[status]}
        {label}
      </div>
      <div className="relative mt-1 h-1.5 rounded-full bg-gray-100">
        <div className="h-full rounded-full bg-blue-600" style={{ width: `${width}%` }} />
        {target != null && (
          <div
            className="absolute -top-0.5 h-2.5 w-0.5 rounded bg-gray-500"
            style={{ left: `${Math.min(100, (target / max) * 100)}%` }}
            aria-hidden="true"
          />
        )}
      </div>
    </div>
  );
}

function Tile({ label, value, note }: { label: string; value: string; note?: string }) {
  return (
    <div className="rounded-lg border border-gray-200 bg-white p-4">
      <p className="text-xs text-gray-500">{label}</p>
      <p className="mt-1 text-2xl font-semibold tabular-nums text-gray-900">{value}</p>
      {note && <p className="mt-0.5 text-xs text-gray-400">{note}</p>}
    </div>
  );
}

function talkStatus(ratio: number | null): Status {
  if (ratio == null) return 'none';
  return ratio >= TALK_RATIO_OK[0] && ratio <= TALK_RATIO_OK[1] ? 'good' : 'warning';
}

export default function PerformancePage() {
  const [days, setDays] = useState<number>(30);
  const [reps, setReps] = useState<RepStats[] | null>(null);

  useEffect(() => {
    setReps(null);
    backend
      .repStats(days)
      .then((found) => setReps(found.reps))
      .catch((error) => {
        toast.error(String(error));
        setReps([]);
      });
  }, [days]);

  const team = useMemo(() => {
    if (!reps || reps.length === 0) return null;
    const calls = reps.reduce((sum, r) => sum + r.calls, 0);
    const weighted = (pick: (r: RepStats) => number | null, weight: (r: RepStats) => number) => {
      const usable = reps.filter((r) => pick(r) != null && weight(r) > 0);
      const total = usable.reduce((sum, r) => sum + weight(r), 0);
      return total ? usable.reduce((sum, r) => sum + pick(r)! * weight(r), 0) / total : null;
    };
    return {
      calls,
      hours: reps.reduce((sum, r) => sum + r.minutes, 0) / 60,
      heard: weighted((r) => r.prospect_heard_rate, (r) => r.calls),
      hints: weighted((r) => r.hints_per_call, (r) => r.calls),
      talk: weighted((r) => r.talk_ratio, (r) => r.calls_with_transcript),
      cost: reps.reduce((sum, r) => sum + r.cost_usd, 0),
    };
  }, [reps]);

  return (
    <div className="h-screen overflow-y-auto bg-gray-50">
      <div className="mx-auto max-w-6xl space-y-6 p-8">
        <div className="flex flex-wrap items-end justify-between gap-4">
          <div>
            <h1 className="flex items-center gap-2 text-2xl font-semibold text-gray-900">
              <TrendingUp className="h-6 w-6 text-gray-400" aria-hidden="true" />
              Rep performance
            </h1>
            <p className="mt-1 text-sm text-gray-500">
              Live calls of 18 seconds or more. Conversation measures come from transcripts; benchmarks are from
              published research on won sales calls.
            </p>
          </div>
          <div className="flex rounded-md border border-gray-200 bg-white p-0.5" role="group" aria-label="Period">
            {PERIODS.map((period) => (
              <button
                key={period}
                type="button"
                onClick={() => setDays(period)}
                aria-pressed={days === period}
                className={`rounded px-3 py-1 text-sm transition-colors ${
                  days === period ? 'bg-blue-600 text-white' : 'text-gray-600 hover:bg-gray-100'
                }`}
              >
                {period} days
              </button>
            ))}
          </div>
        </div>

        {reps === null ? (
          <div className="flex justify-center py-24">
            <Loader2 className="h-8 w-8 animate-spin text-gray-400" />
          </div>
        ) : !team ? (
          <p className="rounded-lg border border-gray-200 bg-white p-8 text-center text-sm text-gray-500">
            No live calls in the last {days} days.
          </p>
        ) : (
          <>
            <div className="grid grid-cols-2 gap-3 md:grid-cols-3 lg:grid-cols-6">
              <Tile label="Calls" value={String(team.calls)} note={`${reps.length} reps`} />
              <Tile label="Hours on calls" value={team.hours.toFixed(1)} />
              <Tile label="Prospect heard" value={pct(team.heard)} note="of calls" />
              <Tile label="Hints per call" value={team.hints == null ? '–' : team.hints.toFixed(1)} />
              <Tile label="Rep talk ratio" value={pct(team.talk)} note="target 43%" />
              <Tile label="Cost" value={`$${team.cost.toFixed(2)}`} />
            </div>

            <div className="overflow-x-auto rounded-lg border border-gray-200 bg-white">
              <table className="w-full text-left text-sm">
                <thead className="border-b border-gray-200 text-xs text-gray-500">
                  <tr>
                    <th className="px-4 py-3 font-medium">Rep</th>
                    <th className="px-4 py-3 font-medium">Calls</th>
                    <th className="px-4 py-3 font-medium" title="Share of the rep's words in the call">
                      Talk ratio
                    </th>
                    <th className="px-4 py-3 font-medium">Questions / call</th>
                    <th className="px-4 py-3 font-medium" title="Most words the rep said without the prospect speaking">
                      Longest monologue
                    </th>
                    <th className="px-4 py-3 font-medium" title="Times the speaker changed, per call">
                      Back-and-forth
                    </th>
                    <th className="px-4 py-3 font-medium">Prospect heard</th>
                    <th className="px-4 py-3 font-medium">Hints / call</th>
                    <th className="px-4 py-3 font-medium">Hint speed p90</th>
                    <th className="px-4 py-3 font-medium">Cost / hour</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-gray-100">
                  {reps.map((rep) => (
                    <tr key={rep.email} className="align-top">
                      <td className="px-4 py-3">
                        <p className="font-medium text-gray-900">{name(rep.email)}</p>
                        <p className="text-xs text-gray-500">last call {when(rep.last_call_at)}</p>
                      </td>
                      <td className="px-4 py-3 tabular-nums text-gray-900">
                        {rep.calls}
                        <p className="text-xs text-gray-500">{rep.avg_minutes} min avg</p>
                      </td>
                      <td className="px-4 py-3">
                        <Meter
                          value={rep.talk_ratio}
                          max={1}
                          target={TALK_RATIO_TARGET}
                          label={pct(rep.talk_ratio)}
                          status={talkStatus(rep.talk_ratio)}
                          hint={`Target about 43% (35–55% is fine). From ${rep.calls_with_transcript} calls with a transcript.`}
                        />
                      </td>
                      <td className="px-4 py-3">
                        <Meter
                          value={rep.questions_per_call}
                          max={20}
                          target={QUESTIONS_TARGET[0]}
                          label={rep.questions_per_call == null ? '–' : rep.questions_per_call.toFixed(1)}
                          status={
                            rep.questions_per_call == null
                              ? 'none'
                              : rep.questions_per_call >= QUESTIONS_TARGET[0]
                                ? 'good'
                                : 'warning'
                          }
                          hint="Top reps ask 11–14 questions on a discovery call."
                        />
                      </td>
                      <td className="px-4 py-3">
                        <Meter
                          value={rep.longest_monologue_words}
                          max={MONOLOGUE_MAX_WORDS * 2}
                          target={MONOLOGUE_MAX_WORDS}
                          label={
                            rep.longest_monologue_words == null
                              ? '–'
                              : `${Math.round(rep.longest_monologue_words)} words`
                          }
                          status={
                            rep.longest_monologue_words == null
                              ? 'none'
                              : rep.longest_monologue_words <= MONOLOGUE_MAX_WORDS
                                ? 'good'
                                : 'warning'
                          }
                          hint="Keep it under about 375 words (2.5 minutes), then hand the call back."
                        />
                      </td>
                      <td className="px-4 py-3 tabular-nums text-gray-900">
                        {rep.switches_per_call == null ? '–' : Math.round(rep.switches_per_call)}
                      </td>
                      <td className="px-4 py-3">
                        <Meter
                          value={rep.prospect_heard_rate}
                          max={1}
                          label={pct(rep.prospect_heard_rate)}
                          status={rep.prospect_heard_rate >= 0.9 ? 'good' : 'warning'}
                          hint="Calls where audio from the prospect's side reached Sidekick. Low means a capture problem."
                        />
                      </td>
                      <td className="px-4 py-3 tabular-nums text-gray-900">
                        {rep.hints_per_call.toFixed(1)}
                        <p className="text-xs text-gray-500">{pct(rep.calls_with_hints_rate)} of calls</p>
                      </td>
                      <td className="px-4 py-3 tabular-nums text-gray-900">
                        {rep.hint_p90_ms == null ? '–' : `${(rep.hint_p90_ms / 1000).toFixed(1)} s`}
                        {rep.hint_p90_source === 'slowest call' && (
                          <p className="text-xs text-gray-500" title="These calls have no hint log yet">
                            slowest call
                          </p>
                        )}
                      </td>
                      <td className="px-4 py-3 tabular-nums text-gray-900">
                        {rep.cost_per_hour_usd == null ? '–' : `$${rep.cost_per_hour_usd.toFixed(2)}`}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <p className="text-xs text-gray-400">
              ✓ within the benchmark · ⚠ outside it · ◌ not enough data. Transcripts are counted from this version of the
              app on, so conversation measures cover newer calls only.
            </p>
          </>
        )}
      </div>
    </div>
  );
}
