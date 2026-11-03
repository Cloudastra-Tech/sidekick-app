'use client';

// Live call screen. Layout follows Meetily's home page (MIT, see MEETILY-LICENSE.md): transcript
// panel with the recording controls floating at the bottom. Sidekick adds the hints panel.

import { useCallback, useEffect, useState } from 'react';
import { PhoneCall } from 'lucide-react';
import { RecordingControls } from '@/components/RecordingControls';
import { ConsentDialog } from '@/components/ConsentDialog';
import { useCall } from '@/contexts/CallContext';
import { TranscriptPanel } from './_components/TranscriptPanel';
import { HintsPanel } from './_components/HintsPanel';

export default function Home() {
  const { status, consented, startCall, detectedCall, dismissDetectedCall } = useCall();
  const [consentOpen, setConsentOpen] = useState(false);

  // Consent is per call: ask for it unless it was just given.
  const requestStart = useCallback(() => {
    if (status !== 'idle') return;
    if (consented) {
      startCall();
    } else {
      setConsentOpen(true);
    }
  }, [status, consented, startCall]);

  // The sidebar's start button asks this page to start a call: by event when already here, or
  // by a note left before navigating here from another page.
  useEffect(() => {
    const onStart = () => requestStart();
    window.addEventListener('sidekick:start-call', onStart);
    if (sessionStorage.getItem('sidekick:start-call')) {
      sessionStorage.removeItem('sidekick:start-call');
      requestStart();
    }
    return () => window.removeEventListener('sidekick:start-call', onStart);
  }, [requestStart]);

  return (
    <div className="flex flex-col h-screen bg-gray-50">
      <ConsentDialog open={consentOpen} onOpenChange={setConsentOpen} onConsented={startCall} />

      {detectedCall && status === 'idle' && (
        <div className="flex items-center justify-center gap-3 border-b border-blue-100 bg-blue-50 px-4 py-2 text-sm text-blue-900">
          <PhoneCall className="h-4 w-4 shrink-0" />
          <span>{detectedCall.app} is using your microphone. Start coaching this call?</span>
          <button
            onClick={requestStart}
            className="rounded-md bg-blue-600 px-3 py-1 text-xs font-medium text-white hover:bg-blue-700"
          >
            Start coaching
          </button>
          <button onClick={dismissDetectedCall} className="rounded-md px-2 py-1 text-xs text-blue-800 hover:bg-blue-100">
            Not this call
          </button>
        </div>
      )}

      <div className="flex flex-1 overflow-hidden">
        {/* Meetily centers the controls under the transcript; with the hints panel beside it,
            they're anchored to the transcript column instead of the whole window. */}
        <div className="flex-[3] min-w-0 flex relative">
          <TranscriptPanel />
          <div className="absolute bottom-12 left-0 right-0 z-10 flex justify-center pointer-events-none">
            <div className="bg-white rounded-full shadow-lg flex items-center pointer-events-auto">
              <RecordingControls onStartRequested={requestStart} />
            </div>
          </div>
        </div>
        <div className="flex-[2] min-w-0 flex">
          <HintsPanel />
        </div>
      </div>
    </div>
  );
}
