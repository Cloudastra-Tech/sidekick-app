'use client';

// Live hints and triggers. Meetily has no equivalent panel, so this is assembled from its
// panel, header, and empty-state styling (MIT, see MEETILY-LICENSE.md).

import { useEffect, useState } from 'react';
import { Check, Lightbulb, ListTree, Send, ThumbsDown, ThumbsUp } from 'lucide-react';
import { toast } from 'sonner';
import { useCall, type ChecklistHint, type TimedHint } from '@/contexts/CallContext';
import { backend, reportPainted, type HintKind, type HintRating, type TriggerReason } from '@/lib/sidekick';
import { hintAgeLabel, useNow } from '@/lib/hintAge';

const ACTIONS: Record<HintKind, string> = {
  question: 'Ask',
  objection: 'Handle objection',
  buying_signal: 'Move to next step',
  reminder: "Don't forget",
  fact: 'Mention',
};

// Urgency reads from a dot beside the action and, for high, a faint tint of the whole card.
const URGENCY_DOT = {
  high: 'bg-red-500',
  medium: 'bg-amber-500',
  low: 'bg-gray-400',
} as const;

const URGENCY_CARD = {
  high: 'border-red-200 bg-red-50',
  medium: 'border-gray-200 bg-white',
  low: 'border-gray-200 bg-white',
} as const;

const URGENCY_TEXT = {
  high: 'text-red-600',
  medium: 'text-amber-600',
  low: 'text-gray-500',
} as const;

function reasonLabel(reason: TriggerReason): string {
  if (reason.kind === 'keyword') return `Keyword "${reason.keyword}"`;
  if (reason.kind === 'asked') return reason.question ? `You asked: ${reason.question}` : 'You asked what to say';
  return reason.kind === 'turn_end' ? 'Prospect finished speaking' : 'Pause in conversation';
}

function RateButton({
  hint,
  rating,
  label,
  onRate,
}: {
  hint: TimedHint;
  rating: HintRating;
  label: string;
  onRate: (hint: TimedHint, rating: HintRating | null) => void;
}) {
  const active = hint.rating === rating;
  const Icon = rating === 'up' ? ThumbsUp : ThumbsDown;
  return (
    <button
      type="button"
      aria-label={label}
      aria-pressed={active}
      title={label}
      onClick={() => onRate(hint, active ? null : rating)}
      className={`rounded p-1 transition-colors ${
        active
          ? rating === 'up'
            ? 'text-green-600 bg-green-50'
            : 'text-red-600 bg-red-50'
          : 'text-gray-300 hover:text-gray-600 hover:bg-gray-100'
      }`}
    >
      <Icon className="w-3.5 h-3.5" fill={active ? 'currentColor' : 'none'} />
    </button>
  );
}

function HintCard({
  hint,
  latest,
  onRate,
}: {
  hint: TimedHint;
  latest: boolean;
  onRate: (hint: TimedHint, rating: HintRating | null) => void;
}) {
  const now = useNow();
  // Only the latest needs it: the older ones are already dimmed, below it.
  const age = latest ? hintAgeLabel(hint.receivedAt, now) : null;
  return (
    <div
      className={`rounded-lg border ${URGENCY_CARD[hint.urgency]} px-3 py-2 transition-opacity duration-700 ${
        latest && age == null ? 'shadow-sm' : 'opacity-70'
      }`}
    >
      <div className="flex items-center justify-between gap-2">
        <span className={`flex items-center gap-1.5 text-xs font-semibold ${URGENCY_TEXT[hint.urgency]}`}>
          <span aria-hidden className={`h-1.5 w-1.5 rounded-full ${URGENCY_DOT[hint.urgency]}`} />
          {ACTIONS[hint.kind]}
        </span>
        <div className="flex items-center gap-1">
          {age != null && <span className="text-xs text-gray-400 mr-1">{age}</span>}
          {hint.speech_to_hint_ms != null && (
            <span className="text-xs text-gray-400 mr-1">{(hint.speech_to_hint_ms / 1000).toFixed(1)}s</span>
          )}
          <RateButton hint={hint} rating="up" label="Useful hint" onRate={onRate} />
          <RateButton hint={hint} rating="down" label="Not useful" onRate={onRate} />
        </div>
      </div>
      <p className={`text-gray-800 leading-relaxed ${latest ? 'text-base font-medium' : 'text-sm'}`}>{hint.text}</p>
    </div>
  );
}

// What to raise in this call, from the notes, the CRM and earlier calls. An item leaves the list
// once the conversation covers it (the coach spots it) or the rep ticks it; covered ones can be
// shown again and reopened.
function OpeningChecklist({ items, onToggle }: { items: ChecklistHint[]; onToggle: (id: string) => void }) {
  const [showCovered, setShowCovered] = useState(false);
  const done = items.filter((item) => item.done).length;
  const shown = showCovered ? items : items.filter((item) => !item.done);
  return (
    <section className="rounded-lg border border-blue-200 bg-blue-50/60 px-3 py-2">
      <div className="flex items-center justify-between gap-2 text-xs font-semibold text-blue-700">
        <span>For this call</span>
        <span className="flex items-center gap-2 font-normal text-blue-600/80">
          {done} of {items.length} covered
          {done > 0 && (
            <button type="button" onClick={() => setShowCovered((v) => !v)} className="underline hover:text-blue-800">
              {showCovered ? 'Hide covered' : 'Show covered'}
            </button>
          )}
        </span>
      </div>
      {shown.length === 0 && <p className="mt-1 text-xs text-blue-600/80">All covered.</p>}
      <ul className="mt-1.5 space-y-1">
        {shown.map((item) => (
          <li key={item.id}>
            <button
              type="button"
              role="checkbox"
              aria-checked={item.done}
              onClick={() => onToggle(item.id)}
              className="flex w-full items-start gap-2 rounded px-1 py-1 text-left hover:bg-blue-100/60"
            >
              <span
                aria-hidden
                className={`mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded border ${
                  item.done ? 'border-blue-600 bg-blue-600 text-white' : 'border-blue-300 bg-white'
                }`}
              >
                {item.done && <Check className="h-3 w-3" strokeWidth={3} />}
              </span>
              <span
                className={`text-sm leading-snug ${item.done ? 'text-gray-400 line-through' : 'text-gray-800'}`}
              >
                <span className="font-medium text-blue-700">{ACTIONS[item.kind]}: </span>
                {item.text}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

/** Ask the coach mid-call: a typed question, or Enter on an empty box for "what should I say?".
 * Ctrl+Shift+Space does the same from anywhere. */
function AskBox() {
  const [question, setQuestion] = useState('');
  const send = () => {
    const text = question.trim();
    backend
      .askCoach(text || null)
      .then(() => setQuestion(''))
      .catch((e) => toast.error(`Couldn't ask the coach: ${e}`));
  };
  return (
    <form
      className="flex items-center gap-2"
      onSubmit={(e) => {
        e.preventDefault();
        send();
      }}
    >
      <input
        value={question}
        onChange={(e) => setQuestion(e.target.value)}
        maxLength={300}
        placeholder="Ask the coach, or Enter for what to say (Ctrl+Shift+Space)"
        aria-label="Ask the coach"
        className="flex-1 rounded-md border border-gray-200 px-3 py-1.5 text-sm placeholder:text-gray-400 focus:outline-none focus:ring-2 focus:ring-gray-300"
      />
      <button
        type="submit"
        title={question.trim() ? 'Ask' : 'What should I say?'}
        aria-label={question.trim() ? 'Ask' : 'What should I say?'}
        className="rounded-md border border-gray-200 p-2 text-gray-600 hover:bg-gray-50"
      >
        <Send className="w-4 h-4" />
      </button>
    </form>
  );
}

export function HintsPanel() {
  const { hints, draftHint, openingHints, toggleOpeningHint, triggers, status, paused, rateHint } = useCall();

  // Reported from here, where the hints are actually drawn, rather than wherever the event lands.
  const latestId = hints[0]?.id;
  const openingId = openingHints[0]?.id;
  useEffect(() => {
    if (latestId) reportPainted(latestId);
  }, [latestId]);
  useEffect(() => {
    if (openingId) reportPainted(openingId);
  }, [openingId]);

  return (
    <div className="w-full bg-white flex flex-col overflow-hidden">
      <div className="p-4 border-b border-gray-200 flex items-center justify-center gap-2 text-sm font-medium text-gray-700">
        <Lightbulb className="w-4 h-4" />
        Hints
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-2 pb-24">
        {status === 'live' && !paused && <AskBox />}
        {openingHints.length > 0 && <OpeningChecklist items={openingHints} onToggle={toggleOpeningHint} />}
        {draftHint && (
          <div className="rounded-lg border border-gray-200 bg-gray-50 px-3 py-2 shadow-sm" aria-live="polite">
            <span className="text-xs font-semibold text-gray-400 animate-pulse">Writing…</span>
            <p className="mt-1 text-sm text-gray-800">{draftHint.text}</p>
          </div>
        )}
        {hints.length === 0 && !draftHint ? (
          <div className="text-center text-gray-500 mt-8">
            <p className="text-sm text-gray-600">
              {status === 'live' ? (paused ? 'Call paused' : 'Listening for the prospect...') : 'No hints yet'}
            </p>
            <p className="text-xs mt-1 text-gray-400">Hints appear here and in the overlay when the prospect speaks</p>
          </div>
        ) : (
          hints.map((hint, index) => <HintCard key={hint.id} hint={hint} latest={index === 0} onRate={rateHint} />)
        )}

        {triggers.length > 0 && (
          <details className="pt-4">
            <summary className="flex items-center gap-2 text-xs text-gray-500 cursor-pointer select-none">
              <ListTree className="w-3.5 h-3.5" />
              Triggers ({triggers.length})
            </summary>
            <ul className="mt-2 space-y-1">
              {triggers.map((trigger) => {
                const last = trigger.lines[trigger.lines.length - 1];
                return (
                  <li key={trigger.id} className="text-xs text-gray-500">
                    <span className="text-gray-400">{new Date(trigger.receivedAt).toLocaleTimeString()}</span>{' '}
                    <span className="font-medium text-gray-700">{reasonLabel(trigger.reason)}</span>
                    {last && <span> — {last.text}</span>}
                  </li>
                );
              })}
            </ul>
          </details>
        )}
      </div>
    </div>
  );
}
