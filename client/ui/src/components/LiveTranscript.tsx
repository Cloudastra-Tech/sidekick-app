'use client';

// The live call transcript as a chat: the prospect on the left, you on the right. Built from
// ElevenLabs UI's Conversation and Message; it stays pinned to the newest line unless you scroll
// up, and a button brings you back down.

import { memo } from 'react';
import { Conversation, ConversationContent, ConversationEmptyState, ConversationScrollButton } from '@/components/ui/conversation';
import { Message, MessageContent } from '@/components/ui/message';
import { RecordingStatusBar } from '@/components/RecordingStatusBar';
import { cleanStopWords } from '@/components/VirtualizedTranscriptView';
import { cn } from '@/lib/utils';
import type { TranscriptSegmentData } from '@/types';

interface LiveTranscriptProps {
  segments: TranscriptSegmentData[];
  isRecording: boolean;
  isPaused: boolean;
  isProcessing: boolean;
}

function formatTime(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  return `${Math.floor(total / 60)
    .toString()
    .padStart(2, '0')}:${(total % 60).toString().padStart(2, '0')}`;
}

const TranscriptMessage = memo(function TranscriptMessage({ segment }: { segment: TranscriptSegmentData }) {
  const isMe = segment.speaker === 'me';
  // Interim lines are still being recognised and will be replaced.
  const interim = segment.id.startsWith('interim-');
  const text = cleanStopWords(segment.text) || segment.text;

  return (
    <Message from={isMe ? 'user' : 'assistant'} className="py-1.5">
      <div className={cn('flex max-w-[80%] flex-col gap-1', isMe ? 'items-end' : 'items-start')}>
        <span className="px-1 text-xs text-muted-foreground">
          <span className={cn('font-semibold', !isMe && 'text-blue-600')}>{isMe ? 'You' : 'Prospect'}</span>
          <span className="ml-2 tabular-nums">{formatTime(segment.timestamp)}</span>
        </span>
        <MessageContent className={cn('max-w-full text-base leading-relaxed', interim && 'opacity-60')}>
          {text}
        </MessageContent>
      </div>
    </Message>
  );
});

export function LiveTranscript({ segments, isRecording, isPaused, isProcessing }: LiveTranscriptProps) {
  return (
    <div className="flex h-full min-h-0 flex-col">
      {isRecording && (
        <div className="px-4 pb-2">
          <RecordingStatusBar isPaused={isPaused} />
        </div>
      )}
      <Conversation className="min-h-0">
        <ConversationContent className="pb-28">
          {segments.length === 0 ? (
            <ConversationEmptyState
              className="mt-8"
              title={isRecording ? (isPaused ? 'Call paused' : 'Listening for speech...') : 'Ready for your call'}
              description={
                isRecording
                  ? isPaused
                    ? 'Resume to keep transcribing'
                    : 'The transcript appears here as people speak'
                  : 'Start a call, or run the demo call, to see the live transcript'
              }
            />
          ) : (
            segments.map((segment) => <TranscriptMessage key={segment.id} segment={segment} />)
          )}
          {isRecording && !isPaused && !isProcessing && segments.length > 0 && (
            <div className="mt-3 flex items-center gap-2 text-sm text-muted-foreground">
              <span className="h-2 w-2 animate-pulse rounded-full bg-blue-500" />
              Listening...
            </div>
          )}
        </ConversationContent>
        <ConversationScrollButton className="bottom-32" />
      </Conversation>
    </div>
  );
}
