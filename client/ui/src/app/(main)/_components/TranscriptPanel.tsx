'use client';

// Adapted from Meetily's home TranscriptPanel (MIT, see MEETILY-LICENSE.md): same sticky header
// with a button group and centered transcript column. Sidekick changes: the header holds Copy,
// Run demo call, and the solo-test switch; data comes from CallContext; the transcript is a chat
// built from ElevenLabs UI components (LiveTranscript).

import { Copy, PlayCircle } from 'lucide-react';
import { toast } from 'sonner';
import { LiveTranscript } from '@/components/LiveTranscript';
import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import { Switch } from '@/components/ui/switch';
import { useCall } from '@/contexts/CallContext';

export function TranscriptPanel() {
  const { segments, status, paused, soloTest, setSoloTest, startDemoCall } = useCall();
  const isRecording = status === 'live';

  const copyTranscript = async () => {
    const text = segments
      .filter((s) => !s.id.startsWith('interim-'))
      .map((s) => `${s.speaker === 'them' ? 'Prospect' : 'You'}: ${s.text}`)
      .join('\n');
    await navigator.clipboard.writeText(text);
    toast.success('Transcript copied');
  };

  return (
    <div className="w-full border-r border-gray-200 bg-white flex flex-col overflow-hidden">
      <div className="sticky top-0 z-10 bg-white p-4 border-gray-200">
        <div className="flex justify-center items-center gap-4 flex-wrap">
          <ButtonGroup>
            {segments.length > 0 && (
              <Button variant="outline" size="sm" onClick={copyTranscript} title="Copy transcript">
                <Copy />
                <span className="hidden md:inline">Copy</span>
              </Button>
            )}
            <Button
              variant="outline"
              size="sm"
              onClick={startDemoCall}
              disabled={status !== 'idle'}
              title="Plays a scripted sales call: no mic, no recording, no Deepgram usage"
            >
              <PlayCircle />
              <span className="hidden md:inline">Run demo call</span>
            </Button>
          </ButtonGroup>
          <label
            className="flex items-center gap-2 text-sm text-gray-600"
            title="Treats your mic as the prospect, so you can test hints alone"
          >
            <Switch checked={soloTest} onCheckedChange={setSoloTest} disabled={isRecording} />
            Solo test
          </label>
        </div>
      </div>

      <div className="flex min-h-0 flex-1 justify-center">
        <div className="w-full max-w-[750px] min-h-0">
          <LiveTranscript
            segments={segments}
            isRecording={isRecording}
            isPaused={paused}
            isProcessing={status === 'finishing'}
          />
        </div>
      </div>
    </div>
  );
}
