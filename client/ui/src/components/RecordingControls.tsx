'use client';

// Adapted from Meetily's RecordingControls (MIT, see MEETILY-LICENSE.md): same floating pill,
// start button, pause/resume and stop buttons, and level bars. Sidekick changes: no
// device/model validation, and starting a live call asks for consent first. The start button
// and level bars are ElevenLabs UI's Orb (it swirls with your voice and the prospect's, from the
// levels Rust measures), with its mic picker beside the start button.

import dynamic from 'next/dynamic';
import { Pause, Play, Square } from 'lucide-react';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from './ui/tooltip';
import { MicSelector } from './ui/mic-selector';
import { useCall } from '@/contexts/CallContext';

// WebGL: rendered only in the window, never while pre-rendering the static UI.
const Orb = dynamic(() => import('./ui/orb').then((m) => m.Orb), {
  ssr: false,
  loading: () => <div className="h-full w-full rounded-full bg-[#7651E6]/20" />,
});

// Iris, the app's brand colour (the dark theme's --primary).
const ORB_COLORS: [string, string] = ['#7651E6', '#B9A8F5'];
// The orb is decoration; keep it cheap, since CPU during a call must stay under 15%.
const IDLE_FPS = 12;
const CALL_FPS = 20;

interface RecordingControlsProps {
  onStartRequested: () => void;
}

// Audio RMS (0–1) to a 0–1 level on a -60..0 dBFS scale, so normal speech moves the orb.
function level(rms: number): number {
  const db = 20 * Math.log10(Math.max(rms, 1e-6));
  return Math.min(1, Math.max(0, (db + 60) / 60));
}

export function RecordingControls({ onStartRequested }: RecordingControlsProps) {
  const { status, levels, paused, stopCall, togglePause } = useCall();
  const isRecording = status === 'live';
  const isFinishing = status === 'finishing';

  return (
    <TooltipProvider>
      <div className="flex flex-col space-y-2">
        <div className="flex items-center space-x-2 bg-white rounded-full shadow-lg px-4 py-2">
          {isFinishing ? (
            <div className="flex items-center space-x-2">
              <div className="animate-spin rounded-full h-5 w-5 border-b-2 border-gray-900"></div>
              <span className="text-sm text-gray-600">Writing call summary...</span>
            </div>
          ) : !isRecording ? (
            <>
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    onClick={onStartRequested}
                    aria-label="Start call"
                    className="h-12 w-12 overflow-hidden rounded-full transition-transform hover:scale-105 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#7651E6]"
                  >
                    <Orb colors={ORB_COLORS} seed={7} agentState={null} maxFps={IDLE_FPS} className="pointer-events-none h-full w-full" />
                  </button>
                </TooltipTrigger>
                <TooltipContent>
                  <p>Start call</p>
                </TooltipContent>
              </Tooltip>
              <MicSelector />
            </>
          ) : (
            <>
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    onClick={togglePause}
                    aria-label={paused ? 'Resume call' : 'Pause call'}
                    className="w-10 h-10 flex items-center justify-center bg-white border-2 border-gray-300 text-gray-600 hover:border-gray-400 hover:bg-gray-50 rounded-full transition-colors relative"
                  >
                    {paused ? <Play size={16} /> : <Pause size={16} />}
                  </button>
                </TooltipTrigger>
                <TooltipContent>
                  <p>{paused ? 'Resume: transcription and hints continue' : 'Pause: stop transcribing and hints'}</p>
                </TooltipContent>
              </Tooltip>

              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    onClick={stopCall}
                    aria-label="End call"
                    className="w-10 h-10 flex items-center justify-center bg-red-500 hover:bg-red-600 rounded-full text-white transition-colors relative"
                  >
                    <Square size={16} />
                  </button>
                </TooltipTrigger>
                <TooltipContent>
                  <p>End call</p>
                </TooltipContent>
              </Tooltip>

              {/* Your voice drives the inner rings, the prospect's the swirl; still while paused. */}
              <div className="ml-2 h-10 w-10" aria-hidden="true">
                <Orb
                  colors={ORB_COLORS}
                  seed={7}
                  maxFps={CALL_FPS}
                  volumeMode="manual"
                  manualInput={paused ? 0 : level(levels.me)}
                  manualOutput={paused ? 0 : level(levels.them)}
                  className="h-full w-full"
                />
              </div>
            </>
          )}
        </div>
      </div>
    </TooltipProvider>
  );
}
