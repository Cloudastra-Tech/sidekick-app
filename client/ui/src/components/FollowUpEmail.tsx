'use client';

// Drafts the follow-up email after a call, from its summary (Claude, on request: calls that need
// no email don't pay for one). The draft is editable here; copy it, or open it in the mail app.

import { useState } from 'react';
import { Copy, Loader2, Mail } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { REP_NAME_KEY } from '@/components/ConsentDialog';
import { backend, type CallBrief, type CallSummaryContent } from '@/lib/sidekick';

function repName(): string | null {
  try {
    return localStorage.getItem(REP_NAME_KEY);
  } catch {
    return null;
  }
}

export function FollowUpEmail({ summary, brief }: { summary: CallSummaryContent; brief: CallBrief | null }) {
  const [drafting, setDrafting] = useState(false);
  const [subject, setSubject] = useState<string | null>(null);
  const [body, setBody] = useState('');

  const draft = async () => {
    setDrafting(true);
    try {
      const email = await backend.draftFollowUp(summary, brief, repName());
      setSubject(email.subject);
      setBody(email.body);
    } catch (e) {
      toast.error(`Couldn't draft the follow-up: ${e}`);
    } finally {
      setDrafting(false);
    }
  };

  const copy = async () => {
    await navigator.clipboard.writeText(`Subject: ${subject}\n\n${body}`);
    toast.success('Follow-up copied');
  };

  const mailto = `mailto:?subject=${encodeURIComponent(subject ?? '')}&body=${encodeURIComponent(body)}`;

  return (
    <section className="mt-8 rounded-lg border border-gray-200 p-4">
      <div className="flex items-center justify-between gap-2">
        <h3 className="flex items-center gap-2 text-sm font-semibold text-gray-800">
          <Mail className="w-4 h-4" />
          Follow-up email
        </h3>
        <Button variant="outline" size="sm" onClick={draft} disabled={drafting}>
          {drafting && <Loader2 className="animate-spin" />}
          {subject == null ? 'Draft follow-up email' : 'Draft again'}
        </Button>
      </div>
      {subject != null && (
        <div className="mt-3 space-y-2">
          <Input value={subject} onChange={(e) => setSubject(e.target.value)} aria-label="Subject" />
          <textarea
            value={body}
            onChange={(e) => setBody(e.target.value)}
            rows={10}
            aria-label="Email"
            className="w-full rounded-md border border-gray-200 p-3 text-sm focus:outline-none focus:ring-2 focus:ring-gray-300"
          />
          <div className="flex gap-2">
            <Button variant="outline" size="sm" onClick={copy}>
              <Copy />
              Copy
            </Button>
            <Button variant="outline" size="sm" asChild>
              <a href={mailto}>
                <Mail />
                Open in mail app
              </a>
            </Button>
          </div>
          <p className="text-xs text-gray-400">Written from the call summary. Check it before sending.</p>
        </div>
      )}
    </section>
  );
}
