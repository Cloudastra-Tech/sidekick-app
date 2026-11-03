'use client';

// Adapted from Anarlog's floating meeting bar (MIT, Copyright (c) 2023-present Fastrepl, Inc.;
// see ANARLOG-LICENSE.md): apps/desktop/src/meeting-float/overlay/bar.tsx.
//
// Kept from Anarlog: the floating bar and panel shapes, hover drag handle, stop control with
// dancing sticks (turns into "Stop" on hover), expand/collapse control, title row, transcript
// bubbles with speaker labels and a jump-to-latest button.
// Sidekick changes: the latest coaching hint leads, above the transcript (and under the bar when
// collapsed), with its action and urgency readable at a glance; a near-opaque Midnight Plum
// surface so faces and slides behind it don't bleed through the text; iris brand accents; state
// comes from Sidekick's events; icons are lucide instead of Phosphor.

import { useEffect, useRef, useState, type CSSProperties } from 'react';
import {
  BookOpen,
  ChevronDown,
  Lightbulb,
  Hourglass,
  ListChecks,
  Maximize2,
  MessageCircleQuestion,
  Mic,
  Minimize2,
  ShieldCheck,
  Square,
  ThumbsDown,
  ThumbsUp,
  TrendingUp,
  type LucideIcon,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import type { HintKind, HintRating, Urgency } from '@/lib/sidekick';
import { hintAgeLabel, useNow } from '@/lib/hintAge';
import { DancingSticks } from './dancing-sticks';
import {
  FLOATING_BAR_COMPACT_GAP,
  FLOATING_BAR_COMPACT_HEIGHT,
  FLOATING_BAR_COMPACT_ICON_SIZE,
  FLOATING_BAR_COMPACT_STOP_WIDTH,
  FLOATING_BAR_CONTROL_RADIUS,
} from './layout';

export type ColorScheme = 'light' | 'dark';

export interface OverlayHint {
  id: string;
  kind: HintKind;
  urgency: Urgency;
  text: string;
  rating: HintRating | null;
  /** When it appeared (ms since epoch): an old hint is dimmed and shows its age. */
  shownAt?: number;
}

export interface TranscriptBubbleData {
  id: string;
  text: string;
  isSelf: boolean;
  speakerLabel: string;
  /** Still being recognised; replaced when the line settles. */
  interim?: boolean;
}

export interface OverlayState {
  title: string;
  /** 0–1 speech level driving the sticks. */
  amplitude: number;
  expanded: boolean;
  /** Anarlog's opacity setting; clamped to 0.35–0.95. */
  opacity: number;
  colorScheme: ColorScheme;
  hint: OverlayHint | null;
  /** What to raise in this call; shown until the first live hint arrives. */
  opening: string[];
  bubbles: TranscriptBubbleData[];
  /** A detected call app to offer coaching for, when no call is running. */
  detectedApp: string | null;
  /** Warning that the call will end automatically soon. */
  notice: string | null;
}

const HINT_ACTIONS: Record<HintKind, { label: string; icon: LucideIcon }> = {
  question: { label: 'Ask', icon: MessageCircleQuestion },
  objection: { label: 'Handle objection', icon: ShieldCheck },
  buying_signal: { label: 'Move to next step', icon: TrendingUp },
  reminder: { label: "Don't forget", icon: ListChecks },
  fact: { label: 'Mention', icon: BookOpen },
};

/** Matches CORNER_RADIUS in overlay.rs, which clips the window to the same shape. */
const GLASS_RADIUS = 18;

const URGENCY_LABEL: Record<Urgency, string | null> = { high: 'Now', medium: null, low: 'At a pause' };

export function FloatingHintsOverlay({
  state,
  onStop,
  onToggleExpanded,
  onRateHint,
  onStartDetected,
  onDismissDetected,
  onAsk,
}: {
  state: OverlayState;
  onStop: () => void;
  /** "What should I say?": a hint now. */
  onAsk: () => void;
  onToggleExpanded: (expanded: boolean) => void;
  onRateHint: (hint: OverlayHint, rating: HintRating | null) => void;
  onStartDetected: () => void;
  onDismissDetected: () => void;
}) {
  const [hovered, setHovered] = useState(false);
  const isExpanded = state.expanded;
  const p = palette(state);
  const paused = state.title.endsWith('Paused');
  const detecting = state.detectedApp != null;
  const dot = detecting ? p.brand : paused ? p.amber : p.live;

  return (
    // The window itself is the glass: Windows blurs what's behind it and clips the window to
    // these rounded corners (overlay.rs), so the tint fills the whole window edge to edge.
    <div
      className="relative flex h-full w-full flex-col overflow-hidden"
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      style={{
        colorScheme: 'dark',
        borderRadius: GLASS_RADIUS,
        background: `${p.glint}, ${p.sheen}, ${p.surface}`,
        color: p.text,
      }}
    >
      <GlassEdge from={p.edgeFrom} to={p.edgeTo} />
      <HoverHandle color={p.handle} visible={hovered} />

      <div
        data-tauri-drag-region
        className="flex shrink-0 items-center justify-between gap-2 pl-3.5 pr-1.5 pt-1"
        style={{ height: FLOATING_BAR_COMPACT_HEIGHT + 4 }}
      >
        <p data-tauri-drag-region className="flex min-w-0 items-center gap-2 text-[13px] font-semibold tracking-[-0.01em]">
          <span
            data-tauri-drag-region
            aria-hidden="true"
            className={cn('inline-flex h-2 w-2 shrink-0 rounded-full', !paused && !detecting && 'sk-breathe')}
            style={{ background: dot, boxShadow: `0 0 8px ${dot}` }}
          />
          <span data-tauri-drag-region className="truncate">
            {state.title}
          </span>
        </p>
        <FloatingControls
          state={state}
          isExpanded={isExpanded}
          p={p}
          onStop={onStop}
          onToggleExpanded={onToggleExpanded}
          onAsk={detecting ? null : onAsk}
        />
      </div>

      <div className="flex min-h-0 flex-1 flex-col gap-2 px-2.5 pb-2.5">
        {state.detectedApp ? (
          <DetectedCallPrompt app={state.detectedApp} p={p} onStart={onStartDetected} onDismiss={onDismissDetected} />
        ) : (
          <>
            {state.notice && (
              <p
                role="status"
                className="flex shrink-0 items-start gap-2 rounded-[14px] px-3 py-2 text-[12px] leading-[17px]"
                style={{ background: p.card, boxShadow: `inset 0 0 0 1px ${p.cardStroke}`, color: p.muted }}
              >
                <Hourglass className="mt-px h-3.5 w-3.5 shrink-0" style={{ color: p.amber }} aria-hidden="true" />
                {state.notice}
              </p>
            )}
            {!state.hint && state.opening.length > 0 ? (
              <OpeningCard items={state.opening} p={p} fill={!isExpanded} />
            ) : (
              <HintCard hint={state.hint} p={p} prominent={isExpanded} fill={!isExpanded} onRate={onRateHint} />
            )}
            {isExpanded && <TranscriptList bubbles={state.bubbles} p={p} />}
          </>
        )}
      </div>
    </div>
  );
}

function FloatingControls({
  state,
  isExpanded,
  p,
  onStop,
  onToggleExpanded,
  onAsk,
}: {
  state: OverlayState;
  isExpanded: boolean;
  p: Palette;
  onStop: () => void;
  onToggleExpanded: (expanded: boolean) => void;
  /** Null while offering a detected call (nothing to ask about yet). */
  onAsk: (() => void) | null;
}) {
  const [hovered, setHovered] = useState(false);
  const [askHovered, setAskHovered] = useState(false);
  return (
    <div className="flex items-center" style={{ gap: FLOATING_BAR_COMPACT_GAP + 1 }}>
      {onAsk && (
        <button
          type="button"
          data-tauri-drag-region="false"
          aria-label="What should I say?"
          title="What should I say? (Ctrl+Shift+Space)"
          onClick={onAsk}
          onMouseEnter={() => setAskHovered(true)}
          onMouseLeave={() => setAskHovered(false)}
          className="flex items-center justify-center transition-colors focus-visible:outline-none focus-visible:ring-2"
          style={
            {
              width: FLOATING_BAR_COMPACT_ICON_SIZE,
              height: FLOATING_BAR_COMPACT_ICON_SIZE,
              borderRadius: FLOATING_BAR_CONTROL_RADIUS,
              background: askHovered ? p.controlFill : 'transparent',
              color: askHovered ? p.text : p.muted,
              '--tw-ring-color': p.brand,
            } as CSSProperties
          }
        >
          <Lightbulb size={14} strokeWidth={2.25} />
        </button>
      )}
      <StopControl state={state} p={p} onStop={onStop} />
      <button
        type="button"
        data-tauri-drag-region="false"
        aria-label={isExpanded ? 'Collapse live transcript' : 'Expand live transcript'}
        title={isExpanded ? 'Show the hint only' : 'Show the transcript'}
        onClick={() => onToggleExpanded(!isExpanded)}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        className="flex items-center justify-center transition-colors focus-visible:outline-none focus-visible:ring-2"
        style={
          {
            width: FLOATING_BAR_COMPACT_ICON_SIZE,
            height: FLOATING_BAR_COMPACT_ICON_SIZE,
            borderRadius: FLOATING_BAR_CONTROL_RADIUS,
            background: hovered ? p.controlFill : 'transparent',
            color: hovered ? p.text : p.muted,
            '--tw-ring-color': p.brand,
          } as CSSProperties
        }
      >
        {isExpanded ? <Minimize2 size={14} strokeWidth={2.25} /> : <Maximize2 size={14} strokeWidth={2.25} />}
      </button>
    </div>
  );
}

function StopControl({ state, p, onStop }: { state: OverlayState; p: Palette; onStop: () => void }) {
  const [hovered, setHovered] = useState(false);

  return (
    <button
      type="button"
      data-tauri-drag-region="false"
      aria-label="End call"
      onClick={onStop}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      onFocus={() => setHovered(true)}
      onBlur={() => setHovered(false)}
      className="flex items-center justify-center transition-colors duration-150 focus-visible:outline-none"
      style={{
        width: FLOATING_BAR_COMPACT_STOP_WIDTH,
        height: FLOATING_BAR_COMPACT_ICON_SIZE,
        borderRadius: FLOATING_BAR_CONTROL_RADIUS,
        background: hovered ? p.controlHover : p.controlFill,
        color: p.text,
        boxShadow: `inset 0 0 0 1px ${p.cardStroke}`,
      }}
    >
      {hovered ? (
        <span className="flex items-center gap-1.5 text-[12px] font-semibold">
          <Square size={9} fill="currentColor" strokeWidth={0} />
          Stop
        </span>
      ) : (
        <DancingSticks color={p.text} amplitude={Math.max(state.amplitude, 0.3)} width={26} height={18} stickWidth={3} gap={2} />
      )}
    </button>
  );
}

// Sidekick addition: offer coaching when a call app starts using the mic.
function DetectedCallPrompt({
  app,
  p,
  onStart,
  onDismiss,
}: {
  app: string;
  p: Palette;
  onStart: () => void;
  onDismiss: () => void;
}) {
  const [dismissHovered, setDismissHovered] = useState(false);
  return (
    <div
      className="sk-hint-arrive flex flex-1 flex-col justify-center gap-2.5 rounded-[16px] p-3"
      style={{ background: p.card, boxShadow: `inset 0 0 0 1px ${p.cardStroke}, inset 0 1px 0 ${p.highlight}` }}
    >
      <div className="flex items-start gap-2.5">
        <span
          className="flex h-8 w-8 shrink-0 items-center justify-center rounded-[10px]"
          style={{ background: p.brandWash, color: p.brandText }}
          aria-hidden="true"
        >
          <Mic className="h-4 w-4" strokeWidth={2.25} />
        </span>
        <div className="min-w-0">
          <p className="text-[14px] font-semibold leading-5 tracking-[-0.01em]">{app} is using your microphone</p>
          <p className="text-[12px] leading-[17px]" style={{ color: p.muted }}>
            Coach this call? You’ll confirm consent first.
          </p>
        </div>
      </div>
      <div className="flex gap-1.5 pl-[42px]">
        <button
          type="button"
          onClick={onStart}
          className="h-8 rounded-[9px] px-3 text-[12px] font-semibold text-white transition-[filter] hover:brightness-110 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-offset-1"
          style={{ background: p.brand, '--tw-ring-color': p.brand, '--tw-ring-offset-color': 'transparent' } as CSSProperties}
        >
          Start coaching
        </button>
        <button
          type="button"
          onClick={onDismiss}
          onMouseEnter={() => setDismissHovered(true)}
          onMouseLeave={() => setDismissHovered(false)}
          className="h-8 rounded-[9px] px-3 text-[12px] font-medium transition-colors focus-visible:outline-none focus-visible:ring-2"
          style={
            {
              color: dismissHovered ? p.text : p.muted,
              background: dismissHovered ? p.controlFill : 'transparent',
              '--tw-ring-color': p.brand,
            } as CSSProperties
          }
        >
          Not this call
        </button>
      </div>
    </div>
  );
}

// Sidekick addition: the latest hint, the one thing the rep reads mid-sentence.
function HintCard({
  hint,
  p,
  prominent,
  fill,
  onRate,
}: {
  hint: OverlayHint | null;
  p: Palette;
  prominent: boolean;
  /** Stretch into the space left (compact bar), so the glass has no empty strip under the hint. */
  fill: boolean;
  onRate: (hint: OverlayHint, rating: HintRating | null) => void;
}) {
  const now = useNow();
  if (!hint) {
    return (
      <div
        className={cn('flex shrink-0 items-center gap-2.5 rounded-[16px] px-3 py-3 text-[12.5px] leading-[18px]', fill && 'flex-1')}
        style={{ background: p.cardQuiet, boxShadow: `inset 0 0 0 1px ${p.cardStroke}`, color: p.muted }}
      >
        <span className="sk-breathe h-2 w-2 shrink-0 rounded-full" style={{ background: p.brand }} aria-hidden="true" />
        Listening for the prospect…
      </div>
    );
  }

  const action = HINT_ACTIONS[hint.kind];
  const urgent = hint.urgency === 'high';
  const urgencyLabel = URGENCY_LABEL[hint.urgency];
  const Icon = action.icon;
  const age = hintAgeLabel(hint.shownAt, now);

  return (
    <article
      key={hint.id}
      data-urgent={urgent}
      data-stale={age != null}
      aria-live="polite"
      className={cn(
        'sk-hint-arrive flex shrink-0 flex-col gap-1.5 rounded-[16px] px-3 pb-3 pt-2 transition-opacity duration-700',
        fill && 'flex-1',
        age != null && 'opacity-60',
      )}
      style={
        {
          background: urgent ? p.urgentCard : p.card,
          boxShadow: `inset 0 0 0 1px ${urgent ? p.urgentStroke : p.cardStroke}, inset 0 1px 0 ${p.highlight}`,
          '--sk-flare': urgent ? p.urgentStroke : 'transparent',
          '--sk-flare-soft': p.urgentGlow,
        } as CSSProperties
      }
    >
      <header className="flex items-center gap-1.5 text-[11.5px] font-semibold leading-4">
        <span className="flex items-center gap-1.5" style={{ color: p.brandText }}>
          <Icon className="h-3.5 w-3.5" strokeWidth={2.25} aria-hidden="true" />
          {action.label}
        </span>
        {age != null ? (
          <span className="text-[10.5px] font-medium" style={{ color: p.muted }}>
            {age}
          </span>
        ) : urgencyLabel && (
          <span
            className="rounded-full px-1.5 py-px text-[10.5px] font-semibold"
            style={urgent ? { background: p.brand, color: '#fff' } : { background: p.controlFill, color: p.muted }}
          >
            {urgencyLabel}
          </span>
        )}
        <span className="-mr-1 ml-auto flex items-center">
          {(['up', 'down'] as const).map((rating) => (
            <RateButton key={rating} hint={hint} rating={rating} p={p} onRate={onRate} />
          ))}
        </span>
      </header>
      <p className={cn('font-semibold tracking-[-0.015em] [text-wrap:pretty]', hintTextSize(hint.text, prominent))}>
        {hint.text}
      </p>
    </article>
  );
}

// Sidekick addition: before the prospect has said anything, what to raise in this call.
function OpeningCard({ items, p, fill }: { items: string[]; p: Palette; fill: boolean }) {
  return (
    <article
      aria-live="polite"
      className={cn('sk-hint-arrive flex shrink-0 flex-col gap-1.5 rounded-[16px] px-3 pb-3 pt-2', fill && 'flex-1')}
      style={{ background: p.card, boxShadow: `inset 0 0 0 1px ${p.cardStroke}, inset 0 1px 0 ${p.highlight}` }}
    >
      <header className="flex items-center gap-1.5 text-[11.5px] font-semibold leading-4" style={{ color: p.brandText }}>
        <ListChecks className="h-3.5 w-3.5" strokeWidth={2.25} aria-hidden="true" />
        For this call
      </header>
      <ol className="flex flex-col gap-1">
        {items.map((text, index) => (
          <li key={index} className="flex gap-2 text-[13.5px] font-semibold leading-[18px] tracking-[-0.01em]">
            <span className="shrink-0 tabular-nums" style={{ color: p.faint }}>
              {index + 1}.
            </span>
            <span className="[text-wrap:pretty]">{text}</span>
          </li>
        ))}
      </ol>
    </article>
  );
}

// Long hints step down in size rather than being cut off with an ellipsis.
function hintTextSize(text: string, prominent: boolean): string {
  const length = text.length;
  if (length <= 70) return prominent ? 'text-[17px] leading-[23px]' : 'text-[16px] leading-[21px]';
  if (length <= 110) return prominent ? 'text-[16px] leading-[21px]' : 'text-[15px] leading-[20px]';
  return 'text-[14px] leading-[19px]';
}

function RateButton({
  hint,
  rating,
  p,
  onRate,
}: {
  hint: OverlayHint;
  rating: 'up' | 'down';
  p: Palette;
  onRate: (hint: OverlayHint, rating: HintRating | null) => void;
}) {
  const [hovered, setHovered] = useState(false);
  const active = hint.rating === rating;
  const Icon = rating === 'up' ? ThumbsUp : ThumbsDown;
  const label = rating === 'up' ? 'Useful hint' : 'Not useful';
  return (
    <button
      type="button"
      aria-label={label}
      aria-pressed={active}
      title={label}
      onClick={() => onRate(hint, active ? null : rating)}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      className="flex h-6 w-6 items-center justify-center rounded-md transition-colors focus-visible:outline-none focus-visible:ring-2"
      style={
        {
          color: active ? p.brandText : hovered ? p.text : p.faint,
          '--tw-ring-color': p.brand,
        } as CSSProperties
      }
    >
      <Icon className="h-3.5 w-3.5" fill={active ? 'currentColor' : 'none'} strokeWidth={2} />
    </button>
  );
}

function TranscriptList({ bubbles, p }: { bubbles: TranscriptBubbleData[]; p: Palette }) {
  const bottomRef = useRef<HTMLDivElement | null>(null);
  const [pinned, setPinned] = useState(true);

  useEffect(() => {
    if (pinned) {
      bottomRef.current?.scrollIntoView?.({ block: 'end' });
    }
  }, [bubbles, pinned]);

  return (
    <div className="relative min-h-0 flex-1">
      <div
        // No visible scrollbar: the Windows default bar clashes with the floating surface. The
        // top edge fades so older lines recede under the hint instead of being cut off.
        className="h-full overflow-y-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
        style={{
          maskImage: 'linear-gradient(to bottom, transparent 0, #000 18px)',
          WebkitMaskImage: 'linear-gradient(to bottom, transparent 0, #000 18px)',
        }}
        onScroll={(event) => {
          const target = event.currentTarget;
          const distance = target.scrollHeight - target.scrollTop - target.clientHeight;
          setPinned(distance < 20);
        }}
      >
        <div className="flex min-h-full flex-col justify-end gap-1.5 px-0.5 pb-0.5 pt-4">
          {bubbles.length === 0 && (
            <p className="px-1 pb-1 text-[12px]" style={{ color: p.faint }}>
              The conversation shows up here.
            </p>
          )}
          {bubbles.map((bubble, index) => (
            <TranscriptBubble
              key={bubble.id}
              bubble={bubble}
              p={p}
              showsSpeakerLabel={
                index === 0 ||
                bubbles[index - 1]?.speakerLabel !== bubble.speakerLabel ||
                bubbles[index - 1]?.isSelf !== bubble.isSelf
              }
            />
          ))}
          <div ref={bottomRef} />
        </div>
      </div>
      {!pinned && bubbles.length > 0 ? (
        <button
          type="button"
          data-tauri-drag-region="false"
          onClick={() => {
            setPinned(true);
            bottomRef.current?.scrollIntoView?.({ block: 'end', behavior: 'smooth' });
          }}
          className="absolute bottom-2 left-1/2 flex h-7 -translate-x-1/2 items-center gap-1.5 rounded-full px-3 text-[11px] font-semibold text-white shadow-[0_4px_14px_rgba(0,0,0,0.28)]"
          style={{ background: p.brand }}
        >
          <ChevronDown size={12} strokeWidth={2.75} />
          Latest
        </button>
      ) : null}
    </div>
  );
}

function TranscriptBubble({
  bubble,
  showsSpeakerLabel,
  p,
}: {
  bubble: TranscriptBubbleData;
  showsSpeakerLabel: boolean;
  p: Palette;
}) {
  return (
    <div className={cn('flex flex-col', bubble.isSelf ? 'items-end' : 'items-start', showsSpeakerLabel && 'mt-1')}>
      {showsSpeakerLabel && (
        <p className="mb-0.5 px-1.5 text-[10.5px] font-semibold" style={{ color: bubble.isSelf ? p.faint : p.brandText }}>
          {bubble.speakerLabel}
        </p>
      )}
      <p
        className={cn(
          'max-w-[86%] px-2.5 py-1.5 text-[12.5px] leading-[18px]',
          bubble.isSelf ? 'rounded-[13px] rounded-br-[5px]' : 'rounded-[13px] rounded-bl-[5px]',
        )}
        style={{
          background: bubble.isSelf ? p.selfBubble : p.themBubble,
          color: bubble.interim ? p.muted : p.text,
        }}
      >
        {bubble.text}
      </p>
    </div>
  );
}

// A 1px rim that catches light at the top and fades toward the bottom, like the edge of a pane.
function GlassEdge({ from, to }: { from: string; to: string }) {
  return (
    <div
      aria-hidden="true"
      className="pointer-events-none absolute inset-0 z-20"
      style={{
        borderRadius: GLASS_RADIUS,
        padding: 1,
        background: `linear-gradient(160deg, ${from} 0%, ${to} 38%, ${to} 70%, ${from} 130%)`,
        WebkitMask: 'linear-gradient(#000 0 0) content-box, linear-gradient(#000 0 0)',
        WebkitMaskComposite: 'xor',
        maskComposite: 'exclude',
      }}
    />
  );
}

function HoverHandle({ color, visible }: { color: string; visible: boolean }) {
  return (
    <div
      data-tauri-drag-region
      aria-hidden="true"
      className="absolute inset-x-0 top-0 z-10 flex h-3 justify-center pt-1 transition-opacity duration-200"
      style={{ opacity: visible ? 1 : 0 }}
    >
      <div data-tauri-drag-region className="h-1 w-9 rounded-full" style={{ background: color }} />
    </div>
  );
}

type Palette = ReturnType<typeof palette>;

// Smoky glass, the same in both app themes: over a video call the window behind is sometimes
// light and sometimes dark, and a light, milky pane turns patchy. A neutral dark tint over the blur
// Windows draws behind the window keeps text steady; a faint top sheen and a soft rim give it
// depth. Colour is kept for meaning: iris for the brand and urgent hints, green for a live call,
// amber for paused. (`colorScheme` still arrives from the app theme; the overlay ignores it.)
function palette(state: OverlayState) {
  const opacity = Math.min(Math.max(state.opacity, 0.35), 0.95);
  const tint = 0.62 + (opacity - 0.35) * 0.2;
  return {
    brand: '#7651E6',
    live: 'rgb(48, 209, 88)',
    amber: 'rgb(255, 196, 64)',
    surface: `rgba(36, 36, 40, ${tint})`,
    sheen: 'linear-gradient(180deg, rgba(255, 255, 255, 0.06) 0%, rgba(255, 255, 255, 0) 40%)',
    glint: 'linear-gradient(0deg, rgba(0, 0, 0, 0) 0%, rgba(0, 0, 0, 0) 100%)',
    edgeFrom: 'rgba(255, 255, 255, 0.22)',
    edgeTo: 'rgba(255, 255, 255, 0.08)',
    highlight: 'rgba(255, 255, 255, 0.06)',
    text: 'rgba(255, 255, 255, 0.95)',
    muted: 'rgba(255, 255, 255, 0.66)',
    faint: 'rgba(255, 255, 255, 0.5)',
    handle: 'rgba(255, 255, 255, 0.35)',
    controlFill: 'rgba(255, 255, 255, 0.08)',
    controlHover: 'rgba(255, 255, 255, 0.16)',
    card: 'rgba(255, 255, 255, 0.06)',
    cardQuiet: 'rgba(255, 255, 255, 0.04)',
    cardStroke: 'rgba(255, 255, 255, 0.07)',
    brandText: 'rgb(190, 176, 255)',
    brandWash: 'rgba(118, 81, 230, 0.3)',
    urgentCard: 'rgba(118, 81, 230, 0.16)',
    urgentStroke: 'rgba(150, 125, 255, 0.45)',
    urgentGlow: 'rgba(118, 81, 230, 0.3)',
    selfBubble: 'rgba(255, 255, 255, 0.14)',
    themBubble: 'rgba(255, 255, 255, 0.06)',
  };
}
