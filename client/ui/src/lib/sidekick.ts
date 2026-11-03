// Typed access to the Sidekick Rust backend: commands and events.
// This replaces Meetily's own backend calls; UI components only talk to the backend through here.

import { invoke as invokeTauri } from '@tauri-apps/api/core';
import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event';

/**
 * The webview runs this bundle a moment before it injects the Tauri API, so a command fired on
 * mount can throw before it ever reaches Rust. Measured on a cold start of the installed build:
 * about 700 ms. A screen that asked a question in that window used to keep the answer forever —
 * the Send to CRM panel decided the workspace had no CRM and hid itself for the rest of the
 * session — so every command waits for the API instead of failing.
 *
 * Only the "API isn't there yet" failure is retried: the command never ran, so nothing can
 * happen twice. Anything Rust itself returned is passed straight on.
 */
const READY_RETRY_MS = 100;
const READY_RETRIES = 20;

function notReady(error: unknown): boolean {
  return error instanceof TypeError || /__TAURI/.test(String(error));
}

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      return await invokeTauri<T>(command, args);
    } catch (error) {
      if (attempt >= READY_RETRIES || !notReady(error)) throw error;
      await new Promise((resolve) => setTimeout(resolve, READY_RETRY_MS));
    }
  }
}

export type Channel = 'me' | 'them';

export interface TranscriptEvent {
  channel: Channel;
  text: string;
  is_final: boolean;
  speech_final: boolean;
}

export type TriggerReason =
  | { kind: 'keyword'; keyword: string }
  | { kind: 'turn_end' }
  | { kind: 'silence' }
  /** The rep asked: the hotkey or button (`question` null), or a typed question. */
  | { kind: 'asked'; question: string | null };

export interface TriggerEvent {
  reason: TriggerReason;
  lines: { channel: Channel; text: string }[];
}

export type HintKind = 'question' | 'objection' | 'buying_signal' | 'reminder' | 'fact';
export type Urgency = 'high' | 'medium' | 'low';

export type HintRating = 'up' | 'down';

export interface HintEvent {
  id: string;
  kind: HintKind;
  urgency: Urgency;
  text: string;
  reason: TriggerReason;
  speech_to_hint_ms: number | null;
}

/** One of the hints shown when a call starts, before anyone has spoken. */
export interface OpeningHint {
  id: string;
  kind: HintKind;
  urgency: Urgency;
  text: string;
}

export interface LevelEvent {
  channel: Channel;
  rms: number;
}

export interface StreamErrorEvent {
  channel: Channel;
  message: string;
}

/** Metrics summary written by the coach when a call ends. */
export interface CallStats {
  mode: 'live' | 'solo_test' | 'demo';
  model: string;
  duration_minutes: number;
  triggers: number;
  llm_calls: number;
  hints_shown: number;
  errors: number;
  cpu_percent: { avg: number; p90: number; max: number } | null;
  latency_ms: {
    speech_to_hint_p50: number | null;
    speech_to_hint_p90: number | null;
    keyword_turn_end_p90: number | null;
    silence_p90: number | null;
    llm_p50: number | null;
    llm_p90: number | null;
    /** Capture to transcript: words appearing (interim) and a line settling (final). Absent in older logs. */
    stt_interim_p50?: number | null;
    stt_interim_p90?: number | null;
    stt_final_p50?: number | null;
    stt_final_p90?: number | null;
  };
  tokens: { input: number; output: number; cache_read: number; cache_write: number; cache_ratio: number | null };
  cost_usd: { llm: number | null; stt: number | null; total: number | null };
  cost_per_call_hour_usd: { llm: number | null; stt: number | null; total: number | null };
  final_transcripts: { me: number; them: number };
  stream_errors: string[];
  metrics_file: string | null;
}

/** Structured post-call summary written by Claude Sonnet. */
export interface CallSummaryContent {
  overview: string;
  prospect: { who: string; needs: string[]; current_setup: string };
  pain_points: string[];
  objections: { objection: string; response: string }[];
  qualification: { budget: string; timeline: string; decision_process: string };
  commitments: { owner: 'us' | 'prospect'; action: string }[];
  next_step: string;
  sentiment: 'positive' | 'neutral' | 'negative';
}

export type CallSummaryStatus =
  | { status: 'pending' }
  | { status: 'ready'; summary: CallSummaryContent; file: string | null; cost_usd: number | null; elapsed_ms: number }
  | { status: 'failed'; error: string; file: string | null }
  | { status: 'skipped'; reason: string; file: string | null };

/** Details entered before a call; every field may be empty. */
export interface CallBrief {
  prospect: string;
  company: string;
  agenda: string;
  /** Set when the rep picked the prospect from the CRM, so the call writes back to that deal. */
  crm_person_id?: string | null;
  crm_company_id?: string | null;
  crm_opportunity_id?: string | null;
  /** What the CRM already knew about them, read before the call and given to the coach. */
  crm_context?: string;
  /** Summaries of earlier calls with them from the whole workspace, read before the call. */
  past_calls?: string;
}

/** What the CRM, earlier calls and the client's files say, read while the call starts. */
export interface CallHistory {
  crm_context: string;
  past_calls: string;
  client_files: string;
}

/** A file or typed note about a client company, kept on the workspace server. */
export interface ClientDocument {
  id: string;
  kind: 'file' | 'note';
  filename: string;
  client_name: string;
  words: number;
  /** A note's text, for editing; empty for files. */
  text: string;
  created_by: string;
  created_at: string;
  updated_at: string;
}

export interface ClientDocuments {
  documents: ClientDocument[];
  words: number;
  word_limit: number;
}

/**
 * Names a client on the server: their CRM company when there is one, else the typed name (the
 * server lower-cases it). Null when there's nothing to name them by.
 */
export function clientKey(companyId: string | null | undefined, companyName: string): string | null {
  if (companyId) return `crm:${companyId}`;
  const name = companyName.trim();
  return name ? `name:${name}` : null;
}

/** A deal in the CRM, as the start-call dialog lists it. */
export interface CrmDeal {
  id: string;
  name: string;
  stage: string;
  last_call_at: string | null;
}

export interface CrmPerson {
  id: string;
  name: string;
  email: string;
  job_title: string;
  phone: string;
  company_id: string | null;
  company_name: string;
  opportunities: CrmDeal[];
}

export interface CrmContext {
  summary: string;
  notes: number;
}

/** "Call with Priya (Acme)", or null when neither name was entered. */
export function briefTitle(brief: CallBrief | null): string | null {
  const prospect = brief?.prospect.trim() ?? '';
  const company = brief?.company.trim() ?? '';
  if (prospect && company) return `Call with ${prospect} (${company})`;
  if (prospect || company) return `Call with ${prospect || company}`;
  return null;
}

/** A call app that started using the microphone. */
export interface DetectedCall {
  key: string;
  app: string;
  pid: number | null;
  /** True when a call started from this offer captures only this app's audio. */
  app_audio_only: boolean;
}

export interface TestCallReport {
  calls: {
    id: string;
    title: string;
    minutes: number;
    hints_shown: number;
    rated_up: number;
    rated_down: number;
    latency_p90_ms: number | null;
    cpu_avg_percent: number | null;
    cost_per_hour_usd: number | null;
    stt_sent_ratio: number | null;
    stream_errors: number;
    short: boolean;
  }[];
  real_calls: number;
  calls_needed: number;
  total_minutes: number;
  checks: { label: string; target: string; value: string | null; status: 'pass' | 'fail' | 'no_data' }[];
  rated_useful_percent: number | null;
  hints_rated: number;
  hints_shown: number;
  cache_hit_percent: number | null;
  stt_sent_percent: number | null;
  summary_cost_usd: number;
}

/** A saved call in the history list. */
export interface CallListItem {
  id: string;
  title: string;
  ended_at_unix_ms: number | null;
  status: 'ready' | 'failed' | 'skipped' | 'transcript_only' | string;
  mode: 'live' | 'solo_test' | 'demo' | null;
}

/** The structured copy of a call saved when it ended. */
export interface CallRecord {
  version: number;
  title: string;
  brief: CallBrief;
  mode: 'live' | 'solo_test' | 'demo';
  call: CallSummaryStatus;
  stats: CallStats;
  transcript: { channel: Channel; text: string }[];
}

export interface SavedCall {
  id: string;
  /** Null for calls saved before call history existed; show `markdown` instead. */
  record: CallRecord | null;
  markdown: string | null;
}

export type SettingSource = 'environment' | 'app' | 'env_file' | 'default';

export interface AppSettings {
  values: Record<
    'DEEPGRAM_LANGUAGE' | 'DEEPGRAM_MIP_OPT_OUT' | 'CALL_DETECTION' | 'CALL_MAX_MINUTES' | 'CALL_AUTO_STOP_SILENCE_MINUTES' | 'CALL_MAX_COST_USD',
    { value: string; source: SettingSource }
  >;
  keys: { deepgram: boolean; anthropic: boolean };
  playbook: { path: string; text: string | null };
  settings_file: string | null;
}

export interface InputDevice {
  id: string;
  name: string;
  /** The Windows default communications mic. */
  is_default: boolean;
}

/** The Sidekick backend this app talks to, if one is configured. */
export interface BackendStatus {
  url: string | null;
  signed_in: boolean;
  email: string | null;
  workspace: string | null;
  is_admin: boolean;
}

/** One rep's numbers over a period, from the server. Rates are 0–1; null when there's no data. */
export interface RepStats {
  email: string;
  calls: number;
  minutes: number;
  avg_minutes: number;
  last_call_at: string;
  prospect_heard_rate: number;
  hints_per_call: number;
  calls_with_hints_rate: number;
  hint_p90_ms: number | null;
  /** "hints": p90 over every hint shown; "slowest call": the worst call's own p90 (no logs yet). */
  hint_p90_source: 'hints' | 'slowest call' | 'none';
  calls_with_transcript: number;
  talk_ratio: number | null;
  longest_monologue_words: number | null;
  questions_per_call: number | null;
  switches_per_call: number | null;
  sentiment: Record<string, number>;
  cost_usd: number;
  cost_per_hour_usd: number | null;
}

/** A call from anyone in the workspace, as the server keeps it. */
/** One side's audio over a call: speech time, share with nothing playing, levels in dBFS. */
export interface AudioLevels {
  speech_seconds: number;
  silent_share: number | null;
  avg_db: number | null;
  peak_db: number | null;
}

export interface TeamCall {
  call_id: string;
  user_email: string;
  prospect: string;
  company: string;
  mode: string;
  duration_minutes: number;
  cost_usd: number | null;
  created_at: string;
  summary: CallSummaryContent | null;
  /** The call's session numbers; older calls may lack some fields. */
  metrics: {
    final_transcripts?: { me?: number; them?: number };
    stt_minutes?: { me?: number; them?: number };
    triggers?: number;
    claude_requests_sent?: number;
    hints_shown?: number;
    opening_hints?: number;
    errors?: number;
    stream_errors?: string[];
    /** Where the prospect's audio was captured from and each side's levels (0.4.1+). */
    audio?: {
      output_device?: string | null;
      prospect_warnings?: number | null;
      me?: AudioLevels | null;
      them?: AudioLevels | null;
    };
    cost_usd?: { total?: number | null };
    latency_ms?: Record<string, number | null>;
  } | null;
}

/** Someone in the workspace, as the Members screen shows them. */
export interface Member {
  id: string;
  email: string;
  name: string;
  is_admin: boolean;
  is_active: boolean;
  monthly_spend_limit_usd: number | null;
  calls: number;
  created_at: string;
}

/** A member just added or given a new password; the password is shown once and never again. */
export interface NewMember {
  member: Member;
  temporary_password: string;
}

/** What a check for a newer build came back with. */
export interface UpdateCheck {
  available: boolean;
  current_version: string;
  version: string | null;
  notes: string | null;
}

export interface CrmStatus {
  configured: boolean;
  base_url: string;
  /** Where the CRM connection comes from: "your workspace", "this computer" or "nothing". */
  through: string;
}

/** Where a sent call landed in the CRM. */
export interface CrmResult {
  record_url: string;
  record_name: string;
  action: 'created' | 'updated';
  matched_person: string | null;
  matched_company: string | null;
}

export interface Summaries {
  /** The call these summaries belong to: the most recently started one. */
  session_id: number;
  session: CallStats | null;
  call: CallSummaryStatus | null;
}

export interface LiveTranscript {
  started_at_unix_ms: number;
  lines: { channel: Channel; text: string; at_unix_ms: number }[];
}

export const backend = {
  consentStatus: () => invoke<boolean>('consent_status'),
  acknowledgeConsent: (acknowledgedBy: string, callLabel: string) =>
    invoke<void>('acknowledge_consent', { acknowledgedBy, callLabel }),
  captureStatus: () => invoke<boolean>('capture_status'),
  /** Ask the coach for a hint now; `question` null means "what should I say?". */
  askCoach: (question: string | null) => invoke<void>('ask_coach', { question }),
  /** The running call's transcript so far (from its crash-safe journal), or null between calls. */
  liveTranscript: () => invoke<LiveTranscript | null>('live_transcript'),
  /** Resolves to the new call's session ID. With `historyPending`, capture starts at once and the
   * opening hints wait for `provideCallHistory`. */
  startCapture: (soloTest: boolean, brief: CallBrief | null, callApp: DetectedCall | null, historyPending = false) =>
    invoke<number>('start_capture', { soloTest, brief, callApp, historyPending }),
  provideCallHistory: (sessionId: number, history: CallHistory) =>
    invoke<void>('provide_call_history', { sessionId, history }),
  /** Opens the backend connection and refreshes an ageing transcription key before a call. */
  warmConnections: () => invoke<void>('warm_connections'),
  detectedCall: () => invoke<DetectedCall | null>('detected_call'),
  dismissDetectedCall: (key: string) => invoke<void>('dismiss_detected_call', { key }),
  requestStartCall: () => invoke<void>('request_start_call'),
  startDemoCall: () => invoke<number>('start_demo_call'),
  /** Resolves to the stopped call's session ID, or null if no call was running. */
  stopCapture: () => invoke<number | null>('stop_capture'),
  setCallPaused: (paused: boolean) => invoke<void>('set_call_paused', { paused }),
  rateHint: (hintId: string, text: string, rating: HintRating | null) =>
    invoke<void>('rate_hint', { hintId, text, rating }),
  hintPainted: (hintId: string) => invoke<void>('hint_painted', { hintId }),
  testCallReport: () => invoke<TestCallReport>('test_call_report'),
  listCalls: () => invoke<CallListItem[]>('list_calls'),
  getCall: (id: string) => invoke<SavedCall>('get_call', { id }),
  deleteCall: (id: string) => invoke<void>('delete_call', { id }),
  getSettings: () => invoke<AppSettings>('get_settings'),
  saveSettings: (input: {
    language: string;
    max_call_minutes: number;
    auto_stop_silence_minutes: number;
    max_call_cost_usd: number;
    call_detection: boolean;
    deepgram_mip_opt_out: boolean;
  }) =>
    invoke<void>('save_settings', { input }),
  savePlaybook: (text: string) => invoke<void>('save_playbook', { text }),
  backendStatus: () => invoke<BackendStatus>('backend_status'),
  /** Signs in and pulls the workspace's settings and playbook. */
  backendLogin: (email: string, password: string) =>
    invoke<{ status: BackendStatus; settings: { playbook_version: number } }>('backend_login', { email, password }),
  backendLogout: () => invoke<void>('backend_logout'),
  backendSyncSettings: () => invoke<{ playbook_version: number }>('backend_sync_settings'),
  /** Sends a finished call to the backend; sending the same call again updates it. */
  pushCallToBackend: (call: {
    call_id: string;
    prospect: string;
    company: string;
    mode: string;
    duration_minutes: number;
    cost_usd: number | null;
    crm_record_url: string | null;
    crm_person_id: string | null;
    crm_company_id: string | null;
    summary: unknown | null;
    metrics: unknown | null;
  }) => invoke<{ id: string }>('push_call_to_backend', { call }),
  /** Earlier calls with this prospect, or anyone at their company, from the whole workspace,
   * as text for the coach's prompt. */
  previousCalls: (crmPersonId: string | null, crmCompanyId: string | null, prospect: string, company: string) =>
    invoke<string>('backend_previous_calls', { crmPersonId, crmCompanyId, prospect, company }),
  /** A client company's files and notes, newest first. */
  clientDocuments: (clientKey: string) => invoke<ClientDocuments>('client_documents', { clientKey }),
  /** Uploads a file about a client. Its bytes go as the raw body, not through JSON. */
  clientUpload: async (clientKey: string, clientName: string, file: File) =>
    invokeTauri<ClientDocument>('client_upload', new Uint8Array(await file.arrayBuffer()), {
      headers: {
        'client-key': encodeURIComponent(clientKey),
        'client-name': encodeURIComponent(clientName),
        'file-name': encodeURIComponent(file.name),
      },
    }),
  clientAddNote: (clientKey: string, clientName: string, text: string) =>
    invoke<ClientDocument>('client_add_note', { clientKey, clientName, text }),
  clientUpdateNote: (id: string, text: string) => invoke<ClientDocument>('client_update_note', { id, text }),
  clientRemove: (id: string) => invoke<void>('client_remove', { id }),
  /** Everything kept about a client as one text for the coach; empty when nothing is. */
  clientContext: (clientKey: string) => invoke<string>('client_context', { clientKey }),
  /** Calls from the server, newest first: the whole team's for an admin, your own otherwise. */
  teamCalls: () => invoke<TeamCall[]>('backend_team_calls'),
  /** What was said on a call (admins, or the rep who made it). */
  callTranscript: (callId: string) =>
    invoke<{ transcript: { channel: 'me' | 'them'; text: string }[] }>('backend_call_transcript', { callId }),
  /** Each rep's numbers over the last `days` days (admins only). */
  repStats: (days: number) => invoke<{ days: number; reps: RepStats[] }>('backend_rep_stats', { days }),
  /** Re-reads who we are (role may have changed since signing in). */
  backendRefreshAccount: () => invoke<BackendStatus>('backend_refresh_account'),
  appVersion: () => invoke<{ version: string }>('app_version'),
  /** Asks the workspace whether a newer build exists. */
  checkForUpdate: () => invoke<UpdateCheck>('check_for_update'),
  /** Installs the build found by the last check, then restarts. */
  installUpdate: () => invoke<void>('install_update'),
  listMembers: () => invoke<Member[]>('list_members'),
  addMember: (email: string, name: string, isAdmin: boolean, monthlySpendLimitUsd?: number) =>
    invoke<NewMember>('add_member', {
      email,
      name,
      isAdmin,
      monthlySpendLimitUsd: monthlySpendLimitUsd ?? null,
    }),
  updateMember: (memberId: string, changes: Partial<Pick<Member, 'name' | 'is_admin' | 'is_active' | 'monthly_spend_limit_usd'>>) =>
    invoke<Member>('update_member', { memberId, changes }),
  resetMemberPassword: (memberId: string) =>
    invoke<NewMember>('reset_member_password', { memberId }),
  removeMember: (memberId: string) => invoke<void>('remove_member', { memberId }),
  crmStatus: () => invoke<CrmStatus>('crm_status'),
  /** Drafts the follow-up email for a finished call from its summary. */
  draftFollowUp: (summary: CallSummaryContent, brief: CallBrief | null, repName: string | null) =>
    invoke<{ subject: string; body: string; cost_usd: number | null }>('draft_follow_up', {
      summary,
      brief,
      repName,
    }),
  /** Writes a finished call to the CRM as an Opportunity; sending the same call again updates it. */
  sendCallToCrm: (call: {
    call_id: string;
    prospect: string;
    company: string;
    summary: string;
    /** The summary in its parts, so each lands in its own CRM field. */
    details: CallSummaryContent;
    next_step: string;
    sentiment: string;
    duration_minutes: number;
    /** The records picked before the call, when there were any. */
    crm_person_id?: string | null;
    crm_company_id?: string | null;
    crm_opportunity_id?: string | null;
  }) => invoke<CrmResult>('send_call_to_crm', { call }),
  /** People in the CRM matching what the rep typed, with their deals. */
  crmSearchPeople: (query: string) => invoke<CrmPerson[]>('crm_search_people', { query }),
  /** Everyone in the workspace CRM, without deals, for searching as the rep types. */
  crmPeopleIndex: () => invoke<CrmPerson[]>('crm_people_index'),
  /** Someone's deals, newest first, read once they are picked from the people list. */
  crmPersonDeals: (personId: string) => invoke<CrmDeal[]>('crm_person_deals', { personId }),
  /** What the CRM knows about a prospect, as text for the coach's prompt. */
  crmPersonContext: (personId: string, opportunityId: string | null) =>
    invoke<CrmContext>('crm_person_context', { personId, opportunityId }),
  /** Mics Windows can record from, and the one chosen in the app (null: Windows default). */
  listMics: () => invoke<{ devices: InputDevice[]; chosen: string | null }>('list_mics'),
  setMic: (id: string | null) => invoke<void>('set_mic', { id }),
  lastSummaries: () => invoke<Summaries>('last_summaries'),
  overlaySetExpanded: (expanded: boolean) => invoke<void>('overlay_set_expanded', { expanded }),
};

export interface BackendEvents {
  transcript: TranscriptEvent;
  trigger: TriggerEvent;
  hint: HintEvent;
  /** A live hint's words so far, while Claude is still writing it; `hint` (same id) follows. */
  'hint-delta': { id: string; text: string };
  /** Words shown for a hint that won't come after all (no hint, or the request failed). */
  'hint-discarded': { id: string };
  /** Hints for the start of the call, from the notes, the CRM and earlier calls. */
  'opening-hints': OpeningHint[];
  /** Opening hints the call has covered (spotted in the conversation, or ticked by the rep). */
  'opening-covered': string[];
  /** Opening hints the rep marked as not covered after all. */
  'opening-reopened': string[];
  'hint-rated': { hint_id: string; rating: HintRating | null };
  'call-detected': DetectedCall;
  'call-detection-cleared': { key: string };
  /** The overlay's prompt asked the main window to start a call. */
  'start-call-requested': null;
  'audio-level': LevelEvent;
  'capture-error': StreamErrorEvent;
  'coach-error': string;
  'session-started': { session_id: number; mode: 'live' | 'solo_test' | 'demo' };
  /** A running call was stopped: from the main window or the overlay (`reason` null), or
   * automatically by the call length / silence limits (`reason` says which). */
  'call-stopped': { session_id: number; reason: string | null };
  /** A running call will end automatically soon (length, silence, or spend limit);
   * `message` null means the warning no longer applies (someone spoke). */
  'call-ending-soon': { message: string | null };
  /** The rep has been talking with nothing from the prospect's side (wrong speaker or headset,
   * usually); `message` null means the prospect is heard again. */
  'prospect-not-heard': { message: string | null };
  'call-paused': { paused: boolean };
  'stt-status': {
    channel: Channel;
    state: 'reconnecting' | 'connected';
    message: string | null;
    retry_in_secs: number | null;
  };
  'overlay-mode': { click_through: boolean };
  /** A demo call's script finished. */
  'session-ended': { session_id: number };
}

/** Tells every window (the overlay too) that the rep ticked an opening hint off, or back on. */
export function shareOpeningTick(hintId: string, covered: boolean) {
  emit(covered ? 'opening-covered' : 'opening-reopened', [hintId]).catch(() => {});
}

/** Tells the coach once a hint's first frame is on screen: where its latency measurement ends.
 * A hidden window paints nothing, so it reports nothing rather than a late time. */
export function reportPainted(hintId: string) {
  if (typeof document === 'undefined' || document.visibilityState !== 'visible') return;
  // The first frame callback runs before the paint that shows the hint, the second after it.
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      backend.hintPainted(hintId).catch(() => {});
    }),
  );
}

export function onBackend<K extends keyof BackendEvents>(
  event: K,
  handler: (payload: BackendEvents[K]) => void,
): Promise<UnlistenFn> {
  // Subscribing has the same cold-start race as a command, and losing a subscription is worse:
  // the screen stays silent for the whole call instead of failing once.
  return (async () => {
    for (let attempt = 0; ; attempt++) {
      try {
        return await listen<BackendEvents[K]>(event, (e) => handler(e.payload));
      } catch (error) {
        if (attempt >= READY_RETRIES || !notReady(error)) throw error;
        await new Promise((resolve) => setTimeout(resolve, READY_RETRY_MS));
      }
    }
  })();
}
