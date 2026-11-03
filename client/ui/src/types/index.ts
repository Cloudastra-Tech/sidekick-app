// Transcript segment shape used by the transcript view (from Meetily, MIT).
export interface TranscriptSegmentData {
  id: string;
  timestamp: number; // seconds since the call started
  endTime?: number;
  text: string;
  confidence?: number;
  /** Sidekick addition: who said it. */
  speaker?: 'me' | 'them';
}
