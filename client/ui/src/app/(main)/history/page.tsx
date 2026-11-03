'use client';

// Call history: every saved call, newest first, with its summary and transcript. Split view and
// empty states follow Meetily's meeting list and details screens (MIT, see MEETILY-LICENSE.md).

import { useCallback, useEffect, useMemo, useState } from 'react';
import { Copy, FileText, History, Loader2, Search, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { SummaryContent, summaryAsText } from '@/components/CallSummaryContent';
import { FollowUpEmail } from '@/components/FollowUpEmail';
import { Input } from '@/components/ui/input';
import { TranscriptWords, normalizeWord, searchTerms } from '@/components/ui/transcript-words';
import { useCall } from '@/contexts/CallContext';
import { backend, type CallListItem, type SavedCall } from '@/lib/sidekick';

const MODE_LABEL = { live: null, solo_test: 'Solo test', demo: 'Demo' } as const;

const STATUS_LABEL: Record<string, string | undefined> = {
  failed: 'Summary failed',
  skipped: 'Too short to summarize',
  transcript_only: 'Transcript only',
};

function formatDate(ms: number | null): string {
  if (ms == null) return '';
  return new Date(ms).toLocaleString(undefined, {
    day: 'numeric',
    month: 'short',
    year: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function Badge({ children }: { children: React.ReactNode }) {
  return <span className="rounded-full bg-gray-100 px-2 py-0.5 text-[11px] text-gray-600">{children}</span>;
}

function CallDetails({ saved, item }: { saved: SavedCall; item: CallListItem }) {
  const record = saved.record;
  if (!record) {
    return (
      <>
        <h2 className="mb-1 text-xl font-semibold text-gray-900">{item.title}</h2>
        <p className="mb-6 text-sm text-gray-500">
          {formatDate(item.ended_at_unix_ms)}. Saved before call history existed, so it's shown as plain text.
        </p>
        <pre className="whitespace-pre-wrap font-sans text-sm leading-relaxed text-gray-800">{saved.markdown}</pre>
      </>
    );
  }

  const { call, stats, brief, transcript } = record;
  return (
    <CallRecordDetails record={record} item={item} call={call} stats={stats} brief={brief} transcript={transcript} />
  );
}

type CallRecord = NonNullable<SavedCall['record']>;

function CallRecordDetails({
  record,
  item,
  call,
  stats,
  brief,
  transcript,
}: {
  record: CallRecord;
  item: CallListItem;
  call: CallRecord['call'];
  stats: CallRecord['stats'];
  brief: CallRecord['brief'];
  transcript: CallRecord['transcript'];
}) {
  const [query, setQuery] = useState('');
  const terms = useMemo(() => searchTerms(query), [query]);
  const matches = useMemo(() => {
    if (terms.length === 0) return 0;
    return transcript.reduce(
      (count, line) =>
        count +
        line.text
          .split(/\s+/)
          .map(normalizeWord)
          .filter((word) => word && terms.some((term) => word.startsWith(term))).length,
      0,
    );
  }, [terms, transcript]);

  return (
    <>
      <h2 className="mb-1 text-xl font-semibold text-gray-900">{record.title}</h2>
      <p className="mb-6 text-sm text-gray-500">
        {formatDate(item.ended_at_unix_ms)}
        {stats && ` · ${stats.duration_minutes.toFixed(1)} min · ${stats.hints_shown} hints`}
        {stats?.cost_usd.total != null && ` · $${stats.cost_usd.total.toFixed(3)}`}
      </p>

      {brief.agenda.trim() && (
        <div className="mb-6 rounded-lg bg-gray-50 p-4 text-sm text-gray-700">
          <p className="mb-1 font-semibold text-gray-800">Your notes before the call</p>
          <p className="whitespace-pre-wrap">{brief.agenda}</p>
        </div>
      )}

      {call.status === 'ready' ? (
        <>
          <SummaryContent summary={call.summary} />
          <FollowUpEmail summary={call.summary} brief={brief} />
        </>
      ) : (
        <p className="rounded-lg bg-amber-50 p-4 text-sm text-amber-800">
          {call.status === 'skipped'
            ? call.reason
            : call.status === 'failed'
              ? `The summary couldn't be written: ${call.error}`
              : 'No summary for this call.'}
        </p>
      )}

      <section className="mt-8">
        <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
          <h3 className="font-semibold text-gray-800">Transcript</h3>
          {transcript.length > 0 && (
            <div className="flex items-center gap-2">
              {terms.length > 0 && (
                <span className="text-xs text-gray-500" aria-live="polite">
                  {matches === 1 ? '1 match' : `${matches} matches`}
                </span>
              )}
              <div className="relative">
                <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-gray-400" />
                <Input
                  type="search"
                  aria-label="Find in transcript"
                  placeholder="Find in transcript"
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  className="h-8 w-56 pl-8 text-sm"
                />
              </div>
            </div>
          )}
        </div>
        {transcript.length === 0 ? (
          <p className="text-sm text-gray-500">Nothing was transcribed.</p>
        ) : (
          <div className="space-y-3">
            {transcript.map((line, index) => (
              <div key={index} className="text-sm">
                <span className={`block text-xs font-semibold ${line.channel === 'them' ? 'text-blue-600' : 'text-gray-500'}`}>
                  {line.channel === 'them' ? 'Prospect' : 'You'}
                </span>
                <TranscriptWords text={line.text} terms={terms} className="text-gray-800" />
              </div>
            ))}
          </div>
        )}
      </section>
    </>
  );
}

export default function HistoryPage() {
  const { status } = useCall();
  const [calls, setCalls] = useState<CallListItem[] | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [saved, setSaved] = useState<SavedCall | null>(null);
  const [loadingCall, setLoadingCall] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const list = await backend.listCalls();
      setCalls(list);
      setSelectedId((current) => (current && list.some((c) => c.id === current) ? current : (list[0]?.id ?? null)));
    } catch (error) {
      toast.error(String(error));
      setCalls([]);
    }
  }, []);

  // Reload when a call finishes, so it appears without leaving the page.
  useEffect(() => {
    if (status === 'idle') refresh();
  }, [status, refresh]);

  useEffect(() => {
    if (!selectedId) {
      setSaved(null);
      return;
    }
    let cancelled = false;
    setLoadingCall(true);
    backend
      .getCall(selectedId)
      .then((call) => !cancelled && setSaved(call))
      .catch((error) => {
        if (!cancelled) {
          setSaved(null);
          toast.error(String(error));
        }
      })
      .finally(() => !cancelled && setLoadingCall(false));
    return () => {
      cancelled = true;
    };
  }, [selectedId]);

  const selected = calls?.find((c) => c.id === selectedId) ?? null;
  const summary = saved?.record?.call.status === 'ready' ? saved.record.call.summary : null;

  const copySummary = async () => {
    if (!summary) return;
    await navigator.clipboard.writeText(summaryAsText(summary));
    toast.success('Summary copied');
  };

  const deleteSelected = async () => {
    if (!selected) return;
    try {
      await backend.deleteCall(selected.id);
      toast.success('Call deleted');
      setConfirmDelete(false);
      setSelectedId(null);
      refresh();
    } catch (error) {
      toast.error(String(error));
    }
  };

  return (
    <div className="flex h-screen bg-gray-50">
      <div className="w-80 shrink-0 flex flex-col bg-white border-r border-gray-200">
        <div className="p-4 border-b border-gray-200 flex items-center justify-center gap-2 text-sm font-medium text-gray-700">
          <History className="w-4 h-4" />
          Call history
        </div>
        <div className="flex-1 overflow-y-auto">
          {calls == null ? (
            <div className="flex justify-center py-12">
              <Loader2 className="h-6 w-6 animate-spin text-gray-400" />
            </div>
          ) : calls.length === 0 ? (
            <p className="px-6 py-12 text-center text-sm text-gray-500">
              No saved calls yet. Calls are saved here when they end.
            </p>
          ) : (
            <ul>
              {calls.map((call) => {
                const mode = call.mode ? MODE_LABEL[call.mode] : null;
                const statusLabel = STATUS_LABEL[call.status];
                return (
                  <li key={call.id}>
                    <button
                      type="button"
                      onClick={() => setSelectedId(call.id)}
                      className={`w-full border-b border-gray-100 px-4 py-3 text-left transition-colors ${
                        call.id === selectedId ? 'bg-blue-50' : 'hover:bg-gray-50'
                      }`}
                    >
                      <p className="truncate text-sm font-medium text-gray-900">{call.title}</p>
                      <p className="mt-0.5 text-xs text-gray-500">{formatDate(call.ended_at_unix_ms)}</p>
                      {(mode || statusLabel) && (
                        <div className="mt-1.5 flex flex-wrap gap-1">
                          {mode && <Badge>{mode}</Badge>}
                          {statusLabel && <Badge>{statusLabel}</Badge>}
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

      <div className="flex-1 min-w-0 flex flex-col bg-white overflow-hidden">
        <div className="p-4 border-b border-gray-200 flex justify-center">
          <ButtonGroup>
            <Button variant="outline" size="sm" onClick={copySummary} disabled={!summary}>
              <Copy />
              <span className="hidden md:inline">Copy summary</span>
            </Button>
            <Button variant="outline" size="sm" onClick={() => setConfirmDelete(true)} disabled={!selected}>
              <Trash2 />
              <span className="hidden md:inline">Delete</span>
            </Button>
          </ButtonGroup>
        </div>

        <div className="flex-1 overflow-y-auto">
          <div className="p-6 w-full max-w-3xl mx-auto">
            {!selected ? (
              <div className="flex flex-col items-center justify-center py-24 text-center">
                <FileText className="w-16 h-16 text-gray-300 mb-4" />
                <p className="text-sm text-gray-500">Select a call to see its summary and transcript.</p>
              </div>
            ) : loadingCall || !saved ? (
              <div className="flex justify-center py-24">
                <Loader2 className="h-8 w-8 animate-spin text-gray-400" />
              </div>
            ) : (
              <CallDetails key={selected.id} saved={saved} item={selected} />
            )}
          </div>
        </div>
      </div>

      <Dialog open={confirmDelete} onOpenChange={setConfirmDelete}>
        <DialogContent className="sm:max-w-[420px]">
          <DialogHeader>
            <DialogTitle>Delete this call?</DialogTitle>
            <DialogDescription>
              {selected?.title} will be removed from this computer, including its transcript and summary. This can't
              be undone.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <button
              onClick={() => setConfirmDelete(false)}
              className="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 hover:bg-gray-200 rounded-md transition-colors"
            >
              Cancel
            </button>
            <button
              onClick={deleteSelected}
              className="px-4 py-2 text-sm font-medium text-white bg-red-600 hover:bg-red-700 rounded-md transition-colors"
            >
              Delete call
            </button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
