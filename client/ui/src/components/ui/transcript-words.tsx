// Adapted from ElevenLabs UI (MIT, see ELEVENLABS-UI-LICENSE.md): the word rendering of
// `transcript-viewer` (TranscriptViewerWord and TranscriptViewerWords). Sidekick changes: padding only on highlighted
// words (the viewer pads every word, which widened spacing in running text); no audio
// playback or character alignment, since calls aren't recorded; words come from the saved
// transcript text, and a word's status is whether it matches a search instead of whether it has
// been played ("current" highlight for a match, "unspoken" dimming for the rest while searching).

import { memo, type HTMLAttributes } from 'react';
import { cn } from '@/lib/utils';

export type TranscriptWordStatus = 'spoken' | 'unspoken' | 'current';

interface TranscriptWordProps extends Omit<HTMLAttributes<HTMLSpanElement>, 'children'> {
  text: string;
  status: TranscriptWordStatus;
}

export function TranscriptWord({ text, status, className, ...props }: TranscriptWordProps) {
  return (
    <span
      data-slot="transcript-word"
      data-kind="word"
      data-status={status}
      className={cn(
        'rounded-sm transition-colors',
        status === 'spoken' && 'text-foreground',
        status === 'unspoken' && 'text-muted-foreground',
        // Padding only on a highlight, offset by a negative margin so the text doesn't shift.
        status === 'current' && '-mx-0.5 bg-primary px-0.5 text-primary-foreground',
        className,
      )}
      {...props}
    >
      {text}
    </span>
  );
}

/** Normalizes a word for matching: lower case, without surrounding punctuation. */
export function normalizeWord(word: string): string {
  return word.toLowerCase().replace(/^[^\p{L}\p{N}]+|[^\p{L}\p{N}]+$/gu, '');
}

/** Search terms from a query; empty when there's nothing to search for. */
export function searchTerms(query: string): string[] {
  return query.split(/\s+/).map(normalizeWord).filter(Boolean);
}

interface TranscriptWordsProps extends HTMLAttributes<HTMLParagraphElement> {
  text: string;
  /** Normalized search terms; a word matches when it starts with any of them. */
  terms: string[];
}

export const TranscriptWords = memo(function TranscriptWords({ text, terms, className, ...props }: TranscriptWordsProps) {
  const searching = terms.length > 0;
  // Keep the whitespace between words as gaps, as the viewer does.
  const parts = text.split(/(\s+)/);
  return (
    <p data-slot="transcript-words" className={cn('leading-relaxed', className)} {...props}>
      {parts.map((part, index) => {
        if (part === '' || /^\s+$/.test(part)) {
          return <span key={index} data-kind="gap">{part}</span>;
        }
        const word = normalizeWord(part);
        const status: TranscriptWordStatus = !searching
          ? 'spoken'
          : word && terms.some((term) => word.startsWith(term))
            ? 'current'
            : 'unspoken';
        return <TranscriptWord key={index} text={part} status={status} />;
      })}
    </p>
  );
});
