'use client';

// Post-call screen. Layout follows Meetily's meeting details split view (MIT, see
// MEETILY-LICENSE.md): transcript on the left, summary on the right, button groups in each
// header. Content is Sidekick's structured call summary and call stats.

import { Copy, FileText, Loader2, Sparkles } from 'lucide-react';
import { toast } from 'sonner';
import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';
import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import { useCall, type TimedHint } from '@/contexts/CallContext';
import { SummaryContent, summaryAsText } from '@/components/CallSummaryContent';
import { SendToCrm } from '@/components/SendToCrm';
import { FollowUpEmail } from '@/components/FollowUpEmail';
import { briefTitle, type CallStats } from '@/lib/sidekick';

/** The saved call's ID, from its file name (`call-1789722575903.md`); null before it's saved. */
function callIdFromFile(file: string | null): string | null {
  return file?.match(/(call-\d+)\.md$/)?.[1] ?? null;
}

function StatsBar({ stats, hints }: { stats: CallStats; hints: TimedHint[] }) {
  const up = hints.filter((h) => h.rating === 'up').length;
  const down = hints.filter((h) => h.rating === 'down').length;
  const seconds = (ms: number | null) => (ms == null ? 'n/a' : `${(ms / 1000).toFixed(1)}s`);
  const dollars = (usd: number | null) => (usd == null ? 'n/a' : `$${usd.toFixed(2)}`);
  const items = [
    ['Duration', `${stats.duration_minutes.toFixed(1)} min`],
    ['Hints', `${stats.hints_shown} of ${stats.llm_calls}`],
    ['Hint speed p90', seconds(stats.latency_ms.keyword_turn_end_p90)],
    ['Transcript speed p90', seconds(stats.latency_ms.stt_interim_p90 ?? null)],
    ['CPU avg', stats.cpu_percent ? `${stats.cpu_percent.avg.toFixed(1)}%` : 'n/a'],
    ['Cache hit', stats.tokens.cache_ratio == null ? 'n/a' : `${Math.round(stats.tokens.cache_ratio * 100)}%`],
    ['Cost / call-hour', dollars(stats.cost_per_call_hour_usd.total)],
    ['Rated useful', up + down === 0 ? 'Not rated' : `${up} of ${up + down} (${Math.round((up / (up + down)) * 100)}%)`],
  ];
  return (
    <div className="mt-8 rounded-lg bg-gray-50 p-4">
      <h3 className="text-sm font-semibold text-gray-700 mb-3">Call stats</h3>
      <dl className="grid grid-cols-3 gap-3 text-sm">
        {items.map(([label, value]) => (
          <div key={label}>
            <dt className="text-xs text-gray-500">{label}</dt>
            <dd className="font-medium text-gray-800">{value}</dd>
          </div>
        ))}
      </dl>
      {stats.errors > 0 && <p className="mt-3 text-xs text-red-600">{stats.errors} model errors during the call</p>}
    </div>
  );
}

export default function SummaryPage() {
  const { segments, callSummary, stats, status, brief, hints } = useCall();
  const title = briefTitle(brief);
  const finals = segments.filter((s) => !s.id.startsWith('interim-'));

  const copySummary = async () => {
    if (callSummary?.status !== 'ready') return;
    await navigator.clipboard.writeText(summaryAsText(callSummary.summary));
    toast.success('Summary copied');
  };

  const copyTranscript = async () => {
    await navigator.clipboard.writeText(
      finals.map((s) => `${s.speaker === 'them' ? 'Prospect' : 'You'}: ${s.text}`).join('\n'),
    );
    toast.success('Transcript copied');
  };

  return (
    <div className="flex h-screen bg-gray-50">
      <div className="w-2/5 min-w-0 flex flex-col bg-white border-r border-gray-200">
        <div className="p-4 border-b border-gray-200 flex justify-center">
          <ButtonGroup>
            <Button variant="outline" size="sm" onClick={copyTranscript} disabled={finals.length === 0}>
              <Copy />
              <span className="hidden md:inline">Copy</span>
            </Button>
          </ButtonGroup>
        </div>
        <div className="flex-1 overflow-hidden pb-4">
          <VirtualizedTranscriptView segments={finals} disableAutoScroll showConfidence={false} />
        </div>
      </div>

      <div className="flex-1 min-w-0 flex flex-col bg-white overflow-hidden">
        <div className="p-4 border-b border-gray-200 flex justify-center">
          <ButtonGroup>
            <Button variant="outline" size="sm" onClick={copySummary} disabled={callSummary?.status !== 'ready'}>
              <Copy />
              <span className="hidden md:inline">Copy summary</span>
            </Button>
          </ButtonGroup>
        </div>

        <div className="flex-1 overflow-y-auto">
          <div className="p-6 w-full max-w-3xl mx-auto">
            {!callSummary && status !== 'finishing' ? (
              <div className="flex flex-col items-center justify-center h-full p-8 text-center">
                <FileText className="w-16 h-16 text-gray-300 mb-4" />
                <h3 className="text-lg font-semibold text-gray-900 mb-2">No call summary yet</h3>
                <p className="text-sm text-gray-500 max-w-md">
                  End a call or run the demo call, and the summary appears here.
                </p>
              </div>
            ) : callSummary == null || callSummary.status === 'pending' ? (
              <div className="flex flex-col items-center justify-center py-24">
                <Loader2 className="h-12 w-12 animate-spin text-blue-500 mb-4" />
                <p className="text-gray-600">Writing call summary...</p>
              </div>
            ) : callSummary.status === 'ready' ? (
              <>
                {title && <h2 className="mb-4 text-xl font-semibold text-gray-900">{title}</h2>}
                <SummaryContent summary={callSummary.summary} />
                <FollowUpEmail summary={callSummary.summary} brief={brief} />
                <SendToCrm
                  callId={callIdFromFile(callSummary.file)}
                  brief={brief}
                  summary={callSummary.summary}
                  durationMinutes={stats?.duration_minutes ?? 0}
                />
                {callSummary.file && (
                  <p className="mt-6 flex items-center gap-2 text-xs text-gray-400">
                    <Sparkles className="w-3.5 h-3.5" />
                    Saved to {callSummary.file}
                  </p>
                )}
              </>
            ) : (
              <div className="mt-4 p-4 rounded-lg bg-red-100 text-red-700">
                <p className="text-sm font-medium">
                  {callSummary.status === 'skipped' ? callSummary.reason : `Couldn't write the summary: ${callSummary.error}`}
                </p>
                {callSummary.file && <p className="mt-1 text-xs">Transcript saved to {callSummary.file}</p>}
              </div>
            )}

            {stats && <StatsBar stats={stats} hints={hints} />}
          </div>
        </div>
      </div>
    </div>
  );
}
