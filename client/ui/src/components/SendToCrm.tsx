'use client';

// Sends a finished call to the CRM, as an Opportunity record. The button always says where the
// call will be written before it writes, and where it landed afterwards, so nothing goes into a
// live CRM by surprise. Sending the same call twice updates one record instead of making two.
//
// A call started without a prospect — the demo call, or a call someone started in a hurry — can
// still be sent: the panel asks who it was with, searching the CRM the same way the start-call
// dialog does.

import { useEffect, useState } from 'react';
import { ArrowUpRight, Building2, CloudUpload, Loader2 } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { CrmProspectPicker, type CrmChoice } from '@/components/CrmProspectPicker';
import { summaryAsText } from '@/components/CallSummaryContent';
import { backend, type CallBrief, type CallSummaryContent, type CrmResult } from '@/lib/sidekick';

export function SendToCrm({
  callId,
  brief,
  summary,
  durationMinutes,
}: {
  /** The saved call's ID; the CRM record keeps it, so a re-send updates that record. */
  callId: string | null;
  brief: CallBrief | null;
  summary: CallSummaryContent;
  durationMinutes: number;
}) {
  const [crm, setCrm] = useState<{ configured: boolean; through: string } | null>(null);
  const [sending, setSending] = useState(false);
  const [sent, setSent] = useState<CrmResult | null>(null);
  // Who the call was with. Starts from the brief, and the rep can fill it in here if it's empty.
  const [prospect, setProspect] = useState('');
  const [company, setCompany] = useState('');
  const [choice, setChoice] = useState<CrmChoice | null>(null);

  useEffect(() => {
    backend
      .crmStatus()
      .then((status) => setCrm({ configured: status.configured, through: status.through }))
      .catch(() => setCrm({ configured: false, through: 'nothing' }));
  }, []);

  useEffect(() => {
    setSent(null);
    setChoice(null);
    setProspect(brief?.prospect.trim() ?? '');
    setCompany(brief?.company.trim() ?? '');
  }, [callId, brief]);

  if (crm?.configured === false || !callId) return null;

  const target = [prospect.trim(), company.trim()].filter(Boolean).join(' · ');
  // The call either knows its records from before it started, or from what's picked here.
  const personId = choice?.person.id ?? brief?.crm_person_id ?? null;
  const companyId = choice?.person.company_id ?? brief?.crm_company_id ?? null;
  const opportunityId = choice?.deal?.id ?? brief?.crm_opportunity_id ?? null;

  const choose = (picked: CrmChoice | null) => {
    setChoice(picked);
    if (picked) {
      setProspect(picked.person.name);
      if (picked.person.company_name) setCompany(picked.person.company_name);
    }
  };

  const send = async () => {
    setSending(true);
    try {
      const result = await backend.sendCallToCrm({
        call_id: callId,
        prospect: prospect.trim(),
        company: company.trim(),
        summary: summaryAsText(summary),
        details: summary,
        next_step: summary.next_step,
        sentiment: summary.sentiment,
        duration_minutes: durationMinutes,
        // Picked before the call or just now: the write lands on that deal, not on a name match.
        crm_person_id: personId,
        crm_company_id: companyId,
        crm_opportunity_id: opportunityId,
      });
      setSent(result);
      toast.success(`Opportunity ${result.action}: ${result.record_name || target}`);
    } catch (error) {
      toast.error(String(error));
    } finally {
      setSending(false);
    }
  };

  if (sent) {
    return (
      <div className="mt-6 flex flex-wrap items-center gap-3 rounded-lg bg-green-50 p-4 text-sm text-green-900">
        <Building2 className="h-4 w-4 shrink-0" aria-hidden="true" />
        <span>
          Opportunity {sent.action}: <strong>{sent.record_name || target}</strong>
          {sent.matched_person && ` · ${sent.matched_person}`}
          {sent.matched_company && ` · ${sent.matched_company}`}
        </span>
        <Button variant="outline" size="sm" className="ml-auto" asChild>
          <a href={sent.record_url} target="_blank" rel="noreferrer">
            Open in CRM
            <ArrowUpRight />
          </a>
        </Button>
        <Button variant="ghost" size="sm" onClick={send} disabled={sending}>
          {sending ? <Loader2 className="animate-spin" /> : null}
          Send again
        </Button>
      </div>
    );
  }

  // Which panel to show is decided by the call, not by what's in the fields right now: typing a
  // name must not swap the form out from under the rep mid-word.
  const named = Boolean(brief?.prospect.trim() || brief?.company.trim());

  // The usual case: the call already knows who it was with, so one button is the whole panel.
  if (named) {
    return (
      <div className="mt-6 flex flex-wrap items-center gap-3 rounded-lg bg-gray-50 p-4 text-sm text-gray-700">
        <Building2 className="h-4 w-4 shrink-0 text-gray-400" aria-hidden="true" />
        <span>
          Write this call to the CRM under <strong>{target}</strong>
          {opportunityId && choice?.deal && ` · ${choice.deal.name || 'their deal'}`}
        </span>
        <Button size="sm" className="ml-auto" onClick={send} disabled={sending || crm === null}>
          {sending ? <Loader2 className="animate-spin" /> : <CloudUpload />}
          {sending ? 'Sending...' : 'Send to CRM'}
        </Button>
      </div>
    );
  }

  // Nobody was named before the call. Ask here rather than hiding the panel: the summary is
  // written and worth keeping, and the CRM needs a name to file it under.
  return (
    <div className="mt-6 space-y-3 rounded-lg bg-gray-50 p-4 text-sm text-gray-700">
      <div className="flex items-center gap-2">
        <Building2 className="h-4 w-4 shrink-0 text-gray-400" aria-hidden="true" />
        <span>Who was this call with? Then it can go to the CRM.</span>
      </div>
      <CrmProspectPicker
        prospect={prospect}
        onProspectChange={setProspect}
        choice={choice}
        onChoose={choose}
        onKeyDown={(event) => {
          if (event.key === 'Enter') event.preventDefault();
        }}
        optional={false}
      />
      <div className="space-y-2">
        <Label htmlFor="crm-company">
          Company <span className="font-normal text-gray-400">(optional)</span>
        </Label>
        <Input
          id="crm-company"
          value={company}
          onChange={(event) => setCompany(event.target.value)}
          placeholder="Company"
        />
      </div>
      <div className="flex justify-end">
        <Button size="sm" onClick={send} disabled={sending || crm === null || !target}>
          {sending ? <Loader2 className="animate-spin" /> : <CloudUpload />}
          {sending ? 'Sending...' : 'Send to CRM'}
        </Button>
      </div>
    </div>
  );
}
