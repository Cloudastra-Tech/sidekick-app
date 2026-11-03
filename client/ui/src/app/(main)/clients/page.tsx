'use client';

// Files and notes about a client company, kept on the workspace server. Whoever in the team takes
// the next call with that company gets them in their coach's prompt, read when the call starts.
// Card styling follows the other main-window screens (Meetily, MIT, see MEETILY-LICENSE.md).

import { useEffect, useMemo, useRef, useState } from 'react';
import { Building2, FileText, Loader2, Pencil, Search, StickyNote, Trash2, Upload, X } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { peopleList } from '@/components/CrmProspectPicker';
import { backend, clientKey, type ClientDocument, type ClientDocuments } from '@/lib/sidekick';

/** What the server can read; anything else is refused there with a reason. */
const ACCEPT = '.pdf,.docx,.html,.htm,.txt,.md,.csv';
const MAX_BYTES = 10 * 1024 * 1024;
const COMPANIES_SHOWN = 8;

interface Client {
  key: string;
  name: string;
}

interface Company {
  id: string;
  name: string;
}

/** Each company once, from the CRM people list the start dialog already loads. */
function companiesOf(people: { company_id: string | null; company_name: string }[]): Company[] {
  const seen = new Map<string, string>();
  for (const person of people) {
    if (person.company_id && person.company_name && !seen.has(person.company_id)) {
      seen.set(person.company_id, person.company_name);
    }
  }
  return [...seen].map(([id, name]) => ({ id, name })).sort((a, b) => a.name.localeCompare(b.name));
}

function when(iso: string): string {
  return new Date(iso).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
}

/** A note's title in the list: its first line. */
function noteTitle(text: string): string {
  const line = text.split('\n').find((l) => l.trim()) ?? 'Note';
  return line.length > 80 ? `${line.slice(0, 80)}…` : line;
}

function ClientPicker({ onPick }: { onPick: (client: Client) => void }) {
  const [query, setQuery] = useState('');
  const [companies, setCompanies] = useState<Company[] | null>(null);

  useEffect(() => {
    peopleList()
      .then((people) => setCompanies(companiesOf(people)))
      .catch(() => setCompanies([])); // No CRM list: a typed name still works.
  }, []);

  const typed = query.trim();
  const matches = useMemo(() => {
    if (!companies || typed.length < 1) return [];
    const q = typed.toLowerCase();
    return companies
      .filter((c) => c.name.toLowerCase().includes(q))
      .sort((a, b) => Number(!a.name.toLowerCase().startsWith(q)) - Number(!b.name.toLowerCase().startsWith(q)))
      .slice(0, COMPANIES_SHOWN);
  }, [companies, typed]);

  return (
    <div className="space-y-2">
      <div className="relative">
        <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-gray-400" />
        <Input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Search a company in your CRM, or type a name"
          className="pl-9"
          autoFocus
        />
      </div>
      {typed && (
        <ul className="overflow-hidden rounded-md border border-gray-200 bg-white">
          {matches.map((company) => (
            <li key={company.id}>
              <button
                type="button"
                onClick={() => onPick({ key: `crm:${company.id}`, name: company.name })}
                className="flex w-full items-center gap-2 px-3 py-2 text-left text-sm text-gray-900 hover:bg-gray-50"
              >
                <Building2 className="h-4 w-4 shrink-0 text-gray-400" aria-hidden="true" />
                {company.name}
              </button>
            </li>
          ))}
          <li>
            <button
              type="button"
              onClick={() => onPick({ key: clientKey(null, typed)!, name: typed })}
              className="flex w-full items-center gap-2 px-3 py-2 text-left text-sm text-gray-600 hover:bg-gray-50"
            >
              <Pencil className="h-4 w-4 shrink-0 text-gray-400" aria-hidden="true" />
              Use “{typed}” (not in the CRM)
            </button>
          </li>
        </ul>
      )}
      {companies === null && <p className="text-xs text-gray-400">Loading companies from the CRM…</p>}
    </div>
  );
}

function DocumentRow({
  document,
  onChanged,
  onRemoved,
}: {
  document: ClientDocument;
  onChanged: () => void;
  onRemoved: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(document.text);
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);

  const save = async () => {
    setBusy(true);
    try {
      await backend.clientUpdateNote(document.id, text);
      setEditing(false);
      onChanged();
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    try {
      await backend.clientRemove(document.id);
      onRemoved();
    } catch (error) {
      toast.error(String(error));
      setBusy(false);
    }
  };

  const Icon = document.kind === 'file' ? FileText : StickyNote;
  return (
    <li className="px-4 py-3">
      <div className="flex items-start gap-3">
        <Icon className="mt-0.5 h-4 w-4 shrink-0 text-gray-400" aria-hidden="true" />
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium text-gray-900">
            {document.kind === 'file' ? document.filename : noteTitle(document.text)}
          </p>
          <p className="text-xs text-gray-500">
            {document.words.toLocaleString()} words · {document.created_by || 'someone'} · {when(document.created_at)}
          </p>
        </div>
        {document.kind === 'note' && !editing && (
          <Button variant="ghost" size="sm" onClick={() => setEditing(true)} disabled={busy}>
            <Pencil />
            Edit
          </Button>
        )}
        {confirming ? (
          <>
            <Button variant="destructive" size="sm" onClick={remove} disabled={busy}>
              {busy ? <Loader2 className="animate-spin" /> : <Trash2 />}
              Remove
            </Button>
            <Button variant="ghost" size="sm" onClick={() => setConfirming(false)} disabled={busy}>
              Keep
            </Button>
          </>
        ) : (
          <Button variant="ghost" size="sm" onClick={() => setConfirming(true)} aria-label="Remove">
            <Trash2 />
          </Button>
        )}
      </div>
      {editing && (
        <div className="mt-3 space-y-2 pl-7">
          <Textarea value={text} onChange={(event) => setText(event.target.value)} rows={5} />
          <div className="flex gap-2">
            <Button size="sm" onClick={save} disabled={busy || !text.trim()}>
              {busy && <Loader2 className="animate-spin" />}
              Save
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setText(document.text);
                setEditing(false);
              }}
            >
              Cancel
            </Button>
          </div>
        </div>
      )}
    </li>
  );
}

export default function ClientsPage() {
  const [client, setClient] = useState<Client | null>(null);
  const [docs, setDocs] = useState<ClientDocuments | null>(null);
  const [uploading, setUploading] = useState<string[]>([]);
  const [note, setNote] = useState('');
  const [savingNote, setSavingNote] = useState(false);
  const [dragging, setDragging] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);

  // Opened from the start-call dialog with the client already chosen.
  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const key = params.get('key');
    if (key) setClient({ key, name: params.get('name') || key.replace(/^(crm|name):/, '') });
  }, []);

  const reload = (key: string) =>
    backend
      .clientDocuments(key)
      .then(setDocs)
      .catch((error) => toast.error(String(error)));

  useEffect(() => {
    setDocs(null);
    if (client) reload(client.key);
  }, [client]);

  const upload = async (files: File[]) => {
    if (!client) return;
    for (const file of files) {
      if (file.size > MAX_BYTES) {
        toast.error(`${file.name} is over 10 MB`);
        continue;
      }
      setUploading((names) => [...names, file.name]);
      try {
        await backend.clientUpload(client.key, client.name, file);
        toast.success(`Added ${file.name}`);
      } catch (error) {
        toast.error(`${file.name}: ${error}`);
      } finally {
        setUploading((names) => names.filter((name) => name !== file.name));
      }
    }
    reload(client.key);
  };

  const addNote = async () => {
    if (!client || !note.trim()) return;
    setSavingNote(true);
    try {
      await backend.clientAddNote(client.key, client.name, note);
      setNote('');
      reload(client.key);
    } catch (error) {
      toast.error(String(error));
    } finally {
      setSavingNote(false);
    }
  };

  const used = docs ? Math.min(100, (docs.words / docs.word_limit) * 100) : 0;

  return (
    <div className="h-screen overflow-y-auto bg-gray-50">
      <div className="mx-auto max-w-3xl space-y-6 p-8">
        <div>
          <h1 className="text-2xl font-semibold text-gray-900">Clients</h1>
          <p className="mt-1 text-sm text-gray-500">
            Files and notes about a client company: their requirements, proposals, org chart, anything worth knowing.
            Whoever takes the next call with them gets these in their coach.
          </p>
        </div>

        <section className="rounded-lg border border-gray-200 bg-white p-6">
          {client ? (
            <div className="flex items-center gap-3">
              <Building2 className="h-5 w-5 text-gray-400" aria-hidden="true" />
              <div className="min-w-0 flex-1">
                <p className="truncate text-base font-semibold text-gray-900">{client.name}</p>
                <p className="text-xs text-gray-500">
                  {client.key.startsWith('crm:') ? 'In your CRM' : 'Not in the CRM: matched by name'}
                </p>
              </div>
              <Button variant="outline" size="sm" onClick={() => setClient(null)}>
                <X />
                Change
              </Button>
            </div>
          ) : (
            <ClientPicker onPick={setClient} />
          )}
        </section>

        {client && (
          <>
            <section className="space-y-4 rounded-lg border border-gray-200 bg-white p-6">
              <div
                onDragOver={(event) => {
                  event.preventDefault();
                  setDragging(true);
                }}
                onDragLeave={() => setDragging(false)}
                onDrop={(event) => {
                  event.preventDefault();
                  setDragging(false);
                  upload([...event.dataTransfer.files]);
                }}
                className={`flex flex-col items-center gap-2 rounded-md border-2 border-dashed px-4 py-6 text-center transition-colors ${
                  dragging ? 'border-blue-500 bg-blue-50' : 'border-gray-200'
                }`}
              >
                <Upload className="h-5 w-5 text-gray-400" aria-hidden="true" />
                <p className="text-sm text-gray-600">Drop files here, or</p>
                <Button size="sm" variant="outline" onClick={() => fileInput.current?.click()}>
                  Choose files
                </Button>
                <p className="text-xs text-gray-400">PDF, Word, HTML, text, Markdown or CSV · up to 10 MB each</p>
                <input
                  ref={fileInput}
                  type="file"
                  accept={ACCEPT}
                  multiple
                  hidden
                  onChange={(event) => {
                    upload([...(event.target.files ?? [])]);
                    event.target.value = '';
                  }}
                />
              </div>
              {uploading.map((name) => (
                <p key={name} className="flex items-center gap-2 text-xs text-gray-500">
                  <Loader2 className="h-3 w-3 animate-spin" aria-hidden="true" />
                  Reading {name}…
                </p>
              ))}

              <div className="space-y-2">
                <Textarea
                  value={note}
                  onChange={(event) => setNote(event.target.value)}
                  placeholder="Or type a note: who decides, what they care about, what was promised…"
                  rows={3}
                />
                <Button size="sm" onClick={addNote} disabled={savingNote || !note.trim()}>
                  {savingNote && <Loader2 className="animate-spin" />}
                  Add note
                </Button>
              </div>
            </section>

            <section className="rounded-lg border border-gray-200 bg-white">
              <div className="space-y-2 border-b border-gray-100 px-4 py-3">
                <div className="flex items-baseline justify-between text-sm">
                  <span className="font-medium text-gray-900">What the coach gets</span>
                  {docs && (
                    <span className="text-xs text-gray-500">
                      {docs.words.toLocaleString()} / {docs.word_limit.toLocaleString()} words
                    </span>
                  )}
                </div>
                <div className="h-1.5 overflow-hidden rounded-full bg-gray-100">
                  <div
                    className={`h-full rounded-full ${used > 90 ? 'bg-amber-500' : 'bg-blue-600'}`}
                    style={{ width: `${used}%` }}
                  />
                </div>
              </div>
              {docs === null ? (
                <p className="flex items-center gap-2 px-4 py-6 text-sm text-gray-500">
                  <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
                  Loading…
                </p>
              ) : docs.documents.length === 0 ? (
                <p className="px-4 py-6 text-sm text-gray-500">Nothing yet. Add a file or a note above.</p>
              ) : (
                <ul className="divide-y divide-gray-100">
                  {docs.documents.map((document) => (
                    <DocumentRow
                      key={`${document.id}-${document.updated_at}`}
                      document={document}
                      onChanged={() => reload(client.key)}
                      onRemoved={() => reload(client.key)}
                    />
                  ))}
                </ul>
              )}
            </section>
          </>
        )}
      </div>
    </div>
  );
}
