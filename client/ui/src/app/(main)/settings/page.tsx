'use client';

// Settings: transcription language, call limits, API key status, and the playbook. Form and
// section styling follows Meetily's settings screen (MIT, see MEETILY-LICENSE.md).

import { useCallback, useEffect, useState } from 'react';
import { CheckCircle2, Loader2, XCircle } from 'lucide-react';
import { toast } from 'sonner';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Switch } from '@/components/ui/switch';
import { Textarea } from '@/components/ui/textarea';
import { BackendAccount } from '@/components/BackendAccount';
import { Members } from '@/components/Members';
import { useCall } from '@/contexts/CallContext';
import { backend, type AppSettings, type BackendStatus, type SettingSource } from '@/lib/sidekick';
import { useTheme, type ThemeChoice } from '@/lib/theme';

const THEMES: { value: ThemeChoice; label: string }[] = [
  { value: 'system', label: 'Match Windows' },
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
];

const LANGUAGES = [
  { value: 'en', label: 'English' },
  { value: 'en-IN', label: 'Indian English' },
  { value: 'multi', label: 'Hindi and English mixed' },
  { value: 'hi', label: 'Hindi' },
];

function Section({ title, description, children }: { title: string; description: string; children: React.ReactNode }) {
  return (
    <section className="rounded-lg border border-gray-200 bg-white p-6">
      <h2 className="text-base font-semibold text-gray-900">{title}</h2>
      <p className="mt-1 mb-5 text-sm text-gray-500">{description}</p>
      {children}
    </section>
  );
}

/** A note when a saved value is overridden by an environment variable. */
function OverriddenNote({ source }: { source: SettingSource }) {
  if (source !== 'environment') return null;
  return <p className="mt-1 text-xs text-amber-700">Set by an environment variable, which takes priority over this.</p>;
}

function SaveButton({ saving, disabled, onClick, children }: { saving: boolean; disabled?: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      onClick={onClick}
      disabled={saving || disabled}
      className="inline-flex items-center gap-2 px-4 py-2 text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 disabled:bg-blue-300 rounded-md transition-colors"
    >
      {saving && <Loader2 className="h-4 w-4 animate-spin" />}
      {children}
    </button>
  );
}

export default function SettingsPage() {
  const { status } = useCall();
  const { choice: themeChoice, setChoice: setThemeChoice } = useTheme();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [language, setLanguage] = useState('en');
  const [maxMinutes, setMaxMinutes] = useState('180');
  const [silenceMinutes, setSilenceMinutes] = useState('15');
  const [maxCost, setMaxCost] = useState('5');
  const [account, setAccount] = useState<BackendStatus | null>(null);
  const [callDetection, setCallDetection] = useState(true);
  const [mipOptOut, setMipOptOut] = useState(true);
  const [playbook, setPlaybook] = useState('');
  const [savingSettings, setSavingSettings] = useState(false);
  const [savingPlaybook, setSavingPlaybook] = useState(false);

  const loadSettings = useCallback(() => {
    backend
      .getSettings()
      .then((loaded) => {
        setSettings(loaded);
        setLanguage(loaded.values.DEEPGRAM_LANGUAGE.value);
        setMaxMinutes(loaded.values.CALL_MAX_MINUTES.value);
        setSilenceMinutes(loaded.values.CALL_AUTO_STOP_SILENCE_MINUTES.value);
        setMaxCost(loaded.values.CALL_MAX_COST_USD.value);
        setCallDetection(loaded.values.CALL_DETECTION.value.toLowerCase() !== 'off');
        setMipOptOut(loaded.values.DEEPGRAM_MIP_OPT_OUT.value.toLowerCase() !== 'off');
        setPlaybook(loaded.playbook.text ?? '');
      })
      .catch((error) => toast.error(String(error)));
  }, []);

  useEffect(loadSettings, [loadSettings]);

  if (!settings) {
    return (
      <div className="flex h-screen items-center justify-center bg-gray-50">
        <Loader2 className="h-8 w-8 animate-spin text-gray-400" />
      </div>
    );
  }

  const appliesNote = status === 'idle' ? 'Changes apply from the next call.' : 'Changes apply from the next call, not the one running now.';
  const playbookChanged = playbook !== (settings.playbook.text ?? '');

  const saveSettings = async () => {
    setSavingSettings(true);
    try {
      await backend.saveSettings({
        language,
        max_call_minutes: Number(maxMinutes),
        auto_stop_silence_minutes: Number(silenceMinutes),
        max_call_cost_usd: Number(maxCost),
        call_detection: callDetection,
        deepgram_mip_opt_out: mipOptOut,
      });
      toast.success('Settings saved');
    } catch (error) {
      toast.error(String(error));
    } finally {
      setSavingSettings(false);
    }
  };

  const savePlaybook = async () => {
    setSavingPlaybook(true);
    try {
      await backend.savePlaybook(playbook);
      setSettings({ ...settings, playbook: { ...settings.playbook, text: playbook } });
      toast.success('Playbook saved');
    } catch (error) {
      toast.error(String(error));
    } finally {
      setSavingPlaybook(false);
    }
  };

  return (
    <div className="h-screen overflow-y-auto bg-gray-50">
      <div className="mx-auto max-w-3xl space-y-6 p-8">
        <div>
          <h1 className="text-2xl font-semibold text-gray-900">Settings</h1>
          <p className="mt-1 text-sm text-gray-500">{appliesNote}</p>
        </div>

        <Section title="Appearance" description="Applies right away to this window and the hint overlay.">
          <div role="radiogroup" aria-label="Theme" className="inline-flex rounded-lg bg-gray-100 p-1">
            {THEMES.map((option) => {
              const selected = themeChoice === option.value;
              return (
                <button
                  key={option.value}
                  role="radio"
                  aria-checked={selected}
                  onClick={() => setThemeChoice(option.value)}
                  className={`rounded-md px-4 py-1.5 text-sm transition-colors ${
                    selected
                      ? 'bg-white font-medium text-gray-900 shadow-sm dark:bg-gray-300'
                      : 'text-gray-600 hover:text-gray-900'
                  }`}
                >
                  {option.label}
                </button>
              );
            })}
          </div>
        </Section>

        <Section title="Transcription and call limits" description="How calls are transcribed, and when a forgotten call ends on its own.">
          <div className="space-y-5">
            <div className="space-y-2">
              <Label htmlFor="language">Language spoken on calls</Label>
              <select
                id="language"
                value={language}
                onChange={(e) => setLanguage(e.target.value)}
                className="flex h-9 w-full max-w-xs rounded-md border border-input bg-transparent px-3 text-sm shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {LANGUAGES.map((l) => (
                  <option key={l.value} value={l.value}>
                    {l.label}
                  </option>
                ))}
              </select>
              <OverriddenNote source={settings.values.DEEPGRAM_LANGUAGE.source} />
            </div>
            <div className="space-y-1">
              <label className="flex items-center gap-3 text-sm font-medium text-gray-900">
                <Switch checked={mipOptOut} onCheckedChange={setMipOptOut} />
                Don&apos;t let Deepgram keep call audio for training
              </label>
              <p className="pl-12 text-xs text-gray-500">
                Keep this on for real calls: the other person hasn&apos;t agreed to it. Turning it off halves the
                transcription price, so only do that for your own test calls.
              </p>
              <OverriddenNote source={settings.values.DEEPGRAM_MIP_OPT_OUT.source} />
            </div>
            <div className="space-y-1">
              <label className="flex items-center gap-3 text-sm font-medium text-gray-900">
                <Switch checked={callDetection} onCheckedChange={setCallDetection} />
                Offer to coach when a call app starts using the microphone
              </label>
              <p className="pl-12 text-xs text-gray-500">
                Zoom, Teams, browsers (Google Meet), Slack, Webex and others. Coaching still starts only after you confirm
                consent.
              </p>
              <OverriddenNote source={settings.values.CALL_DETECTION.source} />
            </div>
            <div className="grid grid-cols-2 gap-4 max-w-md">
              <div className="space-y-2">
                <Label htmlFor="max-minutes">End calls after (minutes)</Label>
                <Input id="max-minutes" type="number" min={1} value={maxMinutes} onChange={(e) => setMaxMinutes(e.target.value)} />
                <OverriddenNote source={settings.values.CALL_MAX_MINUTES.source} />
              </div>
              <div className="space-y-2">
                <Label htmlFor="silence-minutes">End after silence (minutes)</Label>
                <Input
                  id="silence-minutes"
                  type="number"
                  min={1}
                  value={silenceMinutes}
                  onChange={(e) => setSilenceMinutes(e.target.value)}
                />
                <OverriddenNote source={settings.values.CALL_AUTO_STOP_SILENCE_MINUTES.source} />
              </div>
              <div className="space-y-2">
                <Label htmlFor="max-cost">End after spending (USD)</Label>
                <Input
                  id="max-cost"
                  type="number"
                  min={0.5}
                  step={0.5}
                  value={maxCost}
                  onChange={(e) => setMaxCost(e.target.value)}
                />
                <OverriddenNote source={settings.values.CALL_MAX_COST_USD.source} />
              </div>
            </div>
            <p className="text-xs text-gray-500">
              You get a warning about a minute before a call ends on its own. Spend is estimated during the call (hints plus
              transcription) and counts only when prices are set in <code>.env</code>.
            </p>
            <SaveButton saving={savingSettings} onClick={saveSettings}>
              Save settings
            </SaveButton>
          </div>
        </Section>

        <Section
          title="Workspace"
          description="Sign in to use your team's settings and playbook, and to send finished calls to the backend."
        >
          <BackendAccount onSettingsChanged={loadSettings} onAccountChanged={setAccount} />
        </Section>

        {account?.is_admin && (
          <Section
            title="Members"
            description="Everyone who can sign in to this workspace. New members get a one-time password to pass on."
          >
            <Members />
          </Section>
        )}

        <Section title="API keys" description="Keys are read from the .env file or environment variables and aren't shown here.">
          <ul className="space-y-2 text-sm">
            {[
              ['Deepgram (transcription)', settings.keys.deepgram],
              ['Anthropic (hints and summaries)', settings.keys.anthropic],
            ].map(([label, set]) => (
              <li key={String(label)} className="flex items-center gap-2">
                {set ? <CheckCircle2 className="h-4 w-4 text-green-600" /> : <XCircle className="h-4 w-4 text-red-600" />}
                <span className="text-gray-800">{label}</span>
                <span className="text-gray-500">{set ? 'Set' : 'Missing: calls will fail to start'}</span>
              </li>
            ))}
          </ul>
        </Section>

        <Section
          title="Playbook"
          description="What hints and summaries know about your company: services, prices, proof points, objections. Hints only state facts written here."
        >
          {settings.playbook.text == null ? (
            <p className="text-sm text-red-600">Couldn't read the playbook at {settings.playbook.path}.</p>
          ) : (
            <div className="space-y-3">
              <Textarea
                value={playbook}
                onChange={(e) => setPlaybook(e.target.value)}
                rows={20}
                spellCheck={false}
                className="font-mono text-xs leading-5"
              />
              <div className="flex items-center justify-between gap-4">
                <p className="truncate text-xs text-gray-400" title={settings.playbook.path}>
                  {settings.playbook.path}
                </p>
                <SaveButton saving={savingPlaybook} disabled={!playbookChanged} onClick={savePlaybook}>
                  Save playbook
                </SaveButton>
              </div>
            </div>
          )}
        </Section>
      </div>
    </div>
  );
}
