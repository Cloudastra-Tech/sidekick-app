'use client';

// Call setup and consent before any recording starts. Built from Meetily's dialog and form
// styling (MIT, see MEETILY-LICENSE.md).

import { useEffect, useRef, useState } from 'react';
import Link from 'next/link';
import { Paperclip } from 'lucide-react';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from './ui/dialog';
import { Input } from './ui/input';
import { Label } from './ui/label';
import { Textarea } from './ui/textarea';
import { useCall } from '@/contexts/CallContext';
import { CrmProspectPicker, type CrmChoice } from './CrmProspectPicker';
import { backend, clientKey, type CallBrief, type CallHistory } from '@/lib/sidekick';

interface ConsentDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** `history` is still being read when the call starts; recording doesn't wait for it. */
  onConsented: (brief: CallBrief, history?: Promise<CallHistory>) => void;
}

/** The rep's name, remembered from the start-call dialog (also signs the follow-up email). */
export const REP_NAME_KEY = 'sidekick:consent-name';
const NAME_KEY = REP_NAME_KEY;

/** How long, from Confirm, the prospect's history gets before the opening hints go without it. */
const HISTORY_WAIT_MS = 4000;
/** Typing pause before earlier calls are looked up for the name typed so far. */
const PREFETCH_DEBOUNCE_MS = 400;

/** A read started while the dialog is open, and what it was read for. */
interface Read {
  key: string;
  promise: Promise<string>;
}

function Optional() {
  return <span className="text-gray-400 font-normal">(optional)</span>;
}

export function ConsentDialog({ open, onOpenChange, onConsented }: ConsentDialogProps) {
  const { acknowledgeConsent, detectedCall } = useCall();
  const [name, setName] = useState('');
  const [prospect, setProspect] = useState('');
  const [company, setCompany] = useState('');
  const [agenda, setAgenda] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  /** The CRM records this call is about, when the rep picked the prospect from the CRM. */
  const [crm, setCrm] = useState<CrmChoice | null>(null);
  // The prospect's history is read while the rep fills in the dialog, not after Confirm: by the
  // time they click, it has usually arrived, and the opening hints don't wait for it.
  const crmRead = useRef<Read | null>(null);
  const pastRead = useRef<Read | null>(null);
  const filesRead = useRef<Read | null>(null);
  /** What's kept about the client, shown so the rep knows the coach has it. */
  const [files, setFiles] = useState<{ count: number; words: number } | null>(null);
  const companyId = crm?.person.company_id ?? null;
  const filesKey = clientKey(companyId, company);

  const readCrm = (choice: CrmChoice) => {
    const key = `${choice.person.id}|${choice.deal?.id ?? ''}`;
    if (crmRead.current?.key !== key) {
      const promise = backend
        .crmPersonContext(choice.person.id, choice.deal?.id ?? null)
        .then((context) => context?.summary ?? '')
        .catch(() => {
          // A failed read isn't kept, so Confirm tries again.
          if (crmRead.current?.key === key) crmRead.current = null;
          return '';
        });
      crmRead.current = { key, promise };
    }
    return crmRead.current.promise;
  };

  const readClientFiles = (key: string) => {
    if (filesRead.current?.key !== key) {
      const promise = backend.clientContext(key).catch(() => {
        if (filesRead.current?.key === key) filesRead.current = null;
        return '';
      });
      filesRead.current = { key, promise };
    }
    return filesRead.current.promise;
  };

  const readPastCalls = (personId: string | null, orgId: string | null, name: string, org: string) => {
    const key = [personId ?? '', orgId ?? '', name.trim().toLowerCase(), org.trim().toLowerCase()].join('|');
    if (pastRead.current?.key !== key) {
      const promise = backend.previousCalls(personId, orgId, name, org).catch(() => {
        if (pastRead.current?.key === key) pastRead.current = null;
        return '';
      });
      pastRead.current = { key, promise };
    }
    return pastRead.current.promise;
  };

  // Your name carries over between calls; the call details start empty each time. Opening the
  // dialog also opens the backend connection the history reads and the first hint will use.
  useEffect(() => {
    if (!open) return;
    try {
      setName(localStorage.getItem(NAME_KEY) ?? '');
    } catch {}
    setProspect('');
    setCompany('');
    setAgenda('');
    setError(null);
    setCrm(null);
    crmRead.current = null;
    pastRead.current = null;
    filesRead.current = null;
    setFiles(null);
    backend.warmConnections().catch(() => {});
  }, [open]);

  // Earlier calls with whoever is named so far, read once the typing pauses.
  useEffect(() => {
    if (!open || (!crm && !prospect.trim() && !company.trim())) return;
    const timer = setTimeout(() => {
      readPastCalls(crm?.person.id ?? null, companyId, prospect, company);
    }, PREFETCH_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [open, crm, prospect, company]);

  // The client's files and notes, read once the company is known, for the coach and to show.
  useEffect(() => {
    setFiles(null);
    if (!open || !filesKey) return;
    let live = true;
    const timer = setTimeout(() => {
      readClientFiles(filesKey);
      backend
        .clientDocuments(filesKey)
        .then((found) => live && setFiles({ count: found.documents.length, words: found.words }))
        .catch(() => {});
    }, PREFETCH_DEBOUNCE_MS);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [open, filesKey]);

  // Picking someone from the CRM fills the company for them; clearing it leaves what they typed.
  const choose = (choice: CrmChoice | null) => {
    setCrm(choice);
    if (choice) {
      setProspect(choice.person.name);
      if (choice.person.company_name) setCompany(choice.person.company_name);
      readCrm(choice);
    }
  };

  const canConfirm = name.trim() !== '' && !saving;

  const confirm = async () => {
    if (!canConfirm) return;
    setSaving(true);
    setError(null);
    try {
      const label = [prospect.trim(), company.trim()].filter(Boolean).join(', ');
      await acknowledgeConsent(name, label);
      try {
        localStorage.setItem(NAME_KEY, name.trim());
      } catch {}

      // Their history goes into the coach's cached prompt: the CRM record and earlier calls
      // anyone in the workspace had with them. The call starts now, without waiting for it (the
      // rep is already talking); the opening hints wait for it, a few seconds at most, and go
      // without whatever didn't arrive.
      const inTime = (read: Promise<string>) =>
        Promise.race([read, new Promise<string>((resolve) => setTimeout(() => resolve(''), HISTORY_WAIT_MS))]);
      const history =
        crm || prospect.trim() || filesKey
          ? Promise.all([
              crm ? inTime(readCrm(crm)) : Promise.resolve(''),
              inTime(readPastCalls(crm?.person.id ?? null, companyId, prospect, company)),
              filesKey ? inTime(readClientFiles(filesKey)) : Promise.resolve(''),
            ]).then(([crm_context, past_calls, client_files]) => ({ crm_context, past_calls, client_files }))
          : undefined;

      onOpenChange(false);
      onConsented(
        {
          prospect,
          company,
          agenda,
          crm_person_id: crm?.person.id ?? null,
          crm_company_id: crm?.person.company_id ?? null,
          crm_opportunity_id: crm?.deal?.id ?? null,
        },
        history,
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  // Enter starts the call from any single-line field; the agenda keeps Enter for new lines.
  const submitOnEnter = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      confirm();
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[480px]">
        <DialogHeader>
          <DialogTitle>Start a call</DialogTitle>
          <DialogDescription>
            The call will be transcribed, and the transcript is saved for your team to review. Confirm that
            everyone on the call has been told and agrees before it starts.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4 py-2">
          {detectedCall && (
            <p className="rounded-md bg-gray-50 px-3 py-2 text-xs text-gray-600">
              Call app: {detectedCall.app}.{' '}
              {detectedCall.app_audio_only
                ? `Only ${detectedCall.app}'s audio is captured for the prospect.`
                : 'All system audio is captured for the prospect, so mute other sounds during the call.'}
            </p>
          )}
          <div className="space-y-2">
            <Label htmlFor="consent-name">Consent confirmed by</Label>
            <Input
              id="consent-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={submitOnEnter}
              placeholder="Your name"
              autoFocus={!name}
            />
          </div>
          {/* The prospect search gets its own row: its results list needs the width. */}
          <CrmProspectPicker
            prospect={prospect}
            onProspectChange={setProspect}
            choice={crm}
            onChoose={choose}
            onKeyDown={submitOnEnter}
            autoFocus={!!name}
          />
          <div className="space-y-2">
            <Label htmlFor="call-company">
              Company <Optional />
            </Label>
            <Input
              id="call-company"
              value={company}
              onChange={(e) => setCompany(e.target.value)}
              onKeyDown={submitOnEnter}
              placeholder="Company"
            />
            {filesKey && files && (
              <p className="flex items-center gap-1.5 text-xs text-gray-500">
                <Paperclip className="h-3 w-3 shrink-0" aria-hidden="true" />
                {files.count === 0
                  ? 'No files or notes about them yet.'
                  : `${files.count} ${files.count === 1 ? 'file or note' : 'files and notes'} about them go to the coach (${files.words.toLocaleString()} words).`}
                <Link
                  href={`/clients/?key=${encodeURIComponent(filesKey)}&name=${encodeURIComponent(company.trim())}`}
                  onClick={() => onOpenChange(false)}
                  className="ml-auto shrink-0 text-blue-600 hover:underline"
                >
                  {files.count === 0 ? 'Add' : 'Manage'}
                </Link>
              </p>
            )}
          </div>
          <div className="space-y-2">
            <Label htmlFor="call-agenda">
              Agenda / goals for this meeting <Optional />
            </Label>
            <Textarea
              id="call-agenda"
              value={agenda}
              onChange={(e) => setAgenda(e.target.value)}
              placeholder="What you want from this call, what to cover, what you already know"
              rows={3}
              maxLength={2000}
            />
            <p className="text-xs text-gray-400">
              Opening hints, live hints and the summary use these, with the CRM and earlier calls.
            </p>
          </div>
          {error && <p className="text-sm text-red-600">{error}</p>}
        </div>
        <DialogFooter>
          <button
            onClick={() => onOpenChange(false)}
            className="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 hover:bg-gray-200 rounded-md transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={confirm}
            disabled={!canConfirm}
            className="px-4 py-2 text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 disabled:bg-blue-300 rounded-md transition-colors"
          >
            Confirm consent and start
          </button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
