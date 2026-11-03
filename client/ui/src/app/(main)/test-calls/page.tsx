'use client';

// Test calls: how to run a real test call, and the Phase 2 go/no-go numbers pooled across
// every live call. Card and table styling follows Meetily's settings and summary screens
// (MIT, see MEETILY-LICENSE.md).

import { useEffect, useState } from 'react';
import { CheckCircle2, CircleDashed, Loader2, XCircle } from 'lucide-react';
import { toast } from 'sonner';
import { useCall } from '@/contexts/CallContext';
import { backend, type TestCallReport } from '@/lib/sidekick';

const STEPS = [
  'Call a colleague or a friendly prospect for at least 10 minutes. Real conversation, not a script.',
  'Use headphones, turn Solo test off, and set the language in Settings (Indian English or Hindi-English mixed if that is how the call will go).',
  'Tell them the call is transcribed and get their OK, then start from the banner or Start call and confirm consent.',
  'During the call, rate hints with 👍 or 👎 when you glance at them. Rate the useless ones too.',
  'After the call, open Call history and read two minutes of the transcript against what you remember. Note wrong or missing words.',
];

const STATUS_ICON = {
  pass: <CheckCircle2 className="h-5 w-5 text-green-600" />,
  fail: <XCircle className="h-5 w-5 text-red-600" />,
  no_data: <CircleDashed className="h-5 w-5 text-gray-400" />,
} as const;

const STATUS_TEXT = { pass: 'Meets target', fail: 'Misses target', no_data: 'No data yet' } as const;

const percent = (value: number | null) => (value == null ? 'n/a' : `${Math.round(value)}%`);

export default function TestCallsPage() {
  const { status } = useCall();
  const [report, setReport] = useState<TestCallReport | null>(null);

  // Reload after each call so a finished test call shows up.
  useEffect(() => {
    if (status !== 'idle') return;
    backend
      .testCallReport()
      .then(setReport)
      .catch((error) => toast.error(String(error)));
  }, [status]);

  return (
    <div className="h-screen overflow-y-auto bg-gray-50">
      <div className="mx-auto max-w-4xl space-y-6 p-8">
        <div>
          <h1 className="text-2xl font-semibold text-gray-900">Test calls</h1>
          <p className="mt-1 text-sm text-gray-500">
            The go/no-go numbers for this phase, measured across your live calls. Demo calls and solo tests aren&apos;t
            counted.
          </p>
        </div>

        <section className="rounded-lg border border-gray-200 bg-white p-6">
          <h2 className="text-base font-semibold text-gray-900">Running a test call</h2>
          <ol className="mt-4 list-decimal space-y-2 pl-5 text-sm text-gray-700">
            {STEPS.map((step) => (
              <li key={step}>{step}</li>
            ))}
          </ol>
        </section>

        {!report ? (
          <div className="flex justify-center py-12">
            <Loader2 className="h-8 w-8 animate-spin text-gray-400" />
          </div>
        ) : (
          <>
            <section className="rounded-lg border border-gray-200 bg-white p-6">
              <h2 className="text-base font-semibold text-gray-900">Targets</h2>
              <p className="mt-1 text-sm text-gray-500">
                {report.real_calls} of {report.calls_needed} real calls (10+ minutes) done. Only those count toward
                the targets.
              </p>
              <ul className="mt-4 divide-y divide-gray-100">
                {report.checks.map((check) => (
                  <li key={check.label} className="flex items-center gap-3 py-3">
                    {STATUS_ICON[check.status]}
                    <div className="min-w-0 flex-1">
                      <p className="text-sm font-medium text-gray-900">{check.label}</p>
                      <p className="text-xs text-gray-500">
                        Target {check.target} · {STATUS_TEXT[check.status]}
                      </p>
                    </div>
                    <span className="text-lg font-semibold text-gray-900">{check.value ?? '–'}</span>
                  </li>
                ))}
              </ul>
              <dl className="mt-4 grid grid-cols-2 gap-4 rounded-lg bg-gray-50 p-4 text-sm sm:grid-cols-4">
                <div>
                  <dt className="text-xs text-gray-500">Hints rated useful</dt>
                  <dd className="font-medium text-gray-800">
                    {percent(report.rated_useful_percent)}{' '}
                    <span className="text-xs font-normal text-gray-500">
                      ({report.hints_rated} of {report.hints_shown} rated)
                    </span>
                  </dd>
                </div>
                <div>
                  <dt className="text-xs text-gray-500">Prompt cache hit</dt>
                  <dd className="font-medium text-gray-800">{percent(report.cache_hit_percent)}</dd>
                </div>
                <div>
                  <dt className="text-xs text-gray-500">Audio sent to transcription</dt>
                  <dd className="font-medium text-gray-800">{percent(report.stt_sent_percent)}</dd>
                </div>
                <div>
                  <dt className="text-xs text-gray-500">Summaries</dt>
                  <dd className="font-medium text-gray-800">${report.summary_cost_usd.toFixed(2)}</dd>
                </div>
              </dl>
            </section>

            <section className="rounded-lg border border-gray-200 bg-white p-6">
              <h2 className="text-base font-semibold text-gray-900">Live calls</h2>
              {report.calls.length === 0 ? (
                <p className="mt-3 text-sm text-gray-500">No live calls yet. Run a test call using the steps above.</p>
              ) : (
                <div className="mt-4 overflow-x-auto">
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="border-b border-gray-200 text-left text-xs text-gray-500">
                        <th className="py-2 pr-3 font-medium">Call</th>
                        <th className="py-2 pr-3 font-medium">Minutes</th>
                        <th className="py-2 pr-3 font-medium">Hint p90</th>
                        <th className="py-2 pr-3 font-medium">Cost / hour</th>
                        <th className="py-2 pr-3 font-medium">CPU</th>
                        <th className="py-2 pr-3 font-medium">Audio sent</th>
                        <th className="py-2 pr-3 font-medium">Hints (👍/👎)</th>
                        <th className="py-2 font-medium">Errors</th>
                      </tr>
                    </thead>
                    <tbody>
                      {report.calls.map((call) => (
                        <tr key={call.id} className="border-b border-gray-100 text-gray-800">
                          <td className="py-2 pr-3">
                            {call.title}
                            {call.short && <span className="ml-2 text-xs text-gray-400">under 10 min</span>}
                          </td>
                          <td className="py-2 pr-3">{call.minutes.toFixed(1)}</td>
                          <td className="py-2 pr-3">
                            {call.latency_p90_ms == null ? 'n/a' : `${(call.latency_p90_ms / 1000).toFixed(1)} s`}
                          </td>
                          <td className="py-2 pr-3">
                            {call.cost_per_hour_usd == null ? 'n/a' : `$${call.cost_per_hour_usd.toFixed(2)}`}
                          </td>
                          <td className="py-2 pr-3">
                            {call.cpu_avg_percent == null ? 'n/a' : `${call.cpu_avg_percent.toFixed(1)}%`}
                          </td>
                          <td className="py-2 pr-3">{call.stt_sent_ratio == null ? 'n/a' : percent(call.stt_sent_ratio * 100)}</td>
                          <td className="py-2 pr-3">
                            {call.hints_shown} ({call.rated_up}/{call.rated_down})
                          </td>
                          <td className="py-2">{call.stream_errors}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </section>
          </>
        )}
      </div>
    </div>
  );
}
