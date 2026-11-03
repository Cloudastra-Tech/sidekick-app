'use client';

// Copied from Meetily (MIT, see MEETILY-LICENSE.md). Sidekick change: the duration comes from
// CallContext (time in the call, excluding pauses) instead of Meetily's backend polling.

import { motion } from 'framer-motion';
import { useCall } from '@/contexts/CallContext';
import { useEffect, useState } from 'react';

interface RecordingStatusBarProps {
  isPaused?: boolean;
}

export const RecordingStatusBar: React.FC<RecordingStatusBarProps> = ({ isPaused = false }) => {
  const { startedAt, mode, activeSeconds } = useCall();

  const [displaySeconds, setDisplaySeconds] = useState(0);

  useEffect(() => {
    if (startedAt == null) return;
    const tick = () => setDisplaySeconds(Math.floor(activeSeconds()));
    tick();
    const interval = setInterval(tick, 500);
    return () => clearInterval(interval);
  }, [startedAt, activeSeconds]);

  const formatDuration = (seconds: number): string => {
    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
  };

  const label = mode === 'demo' ? 'Demo call' : mode === 'solo_test' ? 'Solo test' : 'Recording';

  return (
    <motion.div
      initial={{ opacity: 0, y: -10 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -10 }}
      transition={{ duration: 0.2 }}
      className="flex items-center gap-2 px-3 py-2 bg-gray-50 rounded-lg mb-2"
    >
      <div className={`w-2 h-2 rounded-full ${isPaused ? 'bg-orange-500' : 'bg-red-500 animate-pulse'}`} />
      <span className={`text-sm ${isPaused ? 'text-orange-700' : 'text-gray-700'}`}>
        {isPaused ? 'Paused' : label} • {formatDuration(displaySeconds)}
      </span>
    </motion.div>
  );
};
