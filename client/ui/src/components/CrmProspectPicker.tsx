'use client';

// Finds the prospect in the CRM before the call starts. Picking someone ties the call to their
// records, so the coach knows where the deal stands and the write-back afterwards lands on that
// deal instead of on whatever the names happen to match.
//
// It stays a plain text field when the CRM is unreachable or the person isn't in it: typing a
// name still starts the call, exactly as before.

import { useEffect, useRef, useState } from 'react';
import { Building2, Check, Loader2, Search, X } from 'lucide-react';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { backend, type CrmDeal, type CrmPerson } from '@/lib/sidekick';

/** What the dialog gets back when someone is picked. */
export interface CrmChoice {
  person: CrmPerson;
  deal: CrmDeal | null;
}

/** Long enough that a typist doesn't fire a search per keystroke, short enough to feel live. */
const SEARCH_AFTER_MS = 150;

/** How long a search's answer is shown again without asking the CRM. */
const CACHE_MS = 2 * 60 * 1000;
const searched = new Map<string, { at: number; people: CrmPerson[] }>();

/**
 * Something to show the moment the rep types: an earlier answer for this exact text, or the
 * answer for a shorter text filtered down. The CRM's answer replaces it a second later.
 */
function instant(query: string): CrmPerson[] | null {
  const key = query.toLowerCase();
  const exact = searched.get(key);
  if (exact && Date.now() - exact.at < CACHE_MS) return exact.people;
  for (let end = key.length - 1; end >= 2; end--) {
    const shorter = searched.get(key.slice(0, end));
    if (shorter && Date.now() - shorter.at < CACHE_MS) {
      const matches = shorter.people.filter((person) =>
        [person.name, person.company_name].some((text) => text?.toLowerCase().includes(key)),
      );
      return matches.length > 0 ? matches : null;
    }
  }
  return null;
}

/**
 * Everyone in the CRM, read once per few minutes and searched here as the rep types: asking the
 * CRM takes one to two seconds per search, this takes none.
 */
const LIST_FRESH_MS = 5 * 60 * 1000;
let list: { at: number; people: Promise<CrmPerson[]> } | null = null;

export function peopleList(): Promise<CrmPerson[]> {
  if (!list || Date.now() - list.at > LIST_FRESH_MS) {
    const people = backend.crmPeopleIndex();
    list = { at: Date.now(), people };
    // A failed read isn't kept, so the next dialog tries again.
    people.catch(() => {
      if (list?.people === people) list = null;
    });
  }
  return list.people;
}

const RESULTS_MAX = 8;

/**
 * Who matches every word typed, in their name, company or email. Names that start with what
 * was typed come first, then names with a word that does, then everything else.
 */
function searchLocally(people: CrmPerson[], query: string): CrmPerson[] {
  const typed = query.toLowerCase();
  const words = typed.split(/\s+/).filter(Boolean);
  const rank = (person: CrmPerson) => {
    const name = person.name.toLowerCase();
    if (name.startsWith(typed)) return 0;
    if (name.split(/[\s\-.]+/).some((part) => part.startsWith(words[0]))) return 1;
    return 2;
  };
  return people
    .filter((person) => {
      const text = [person.name, person.company_name, person.email].join(' ').toLowerCase();
      return words.every((word) => text.includes(word));
    })
    .map((person) => ({ person, rank: rank(person) }))
    .sort((a, b) => a.rank - b.rank || a.person.name.localeCompare(b.person.name))
    .slice(0, RESULTS_MAX)
    .map(({ person }) => person);
}

/** The last few people picked, offered before anything is typed. Kept on this computer only. */
const RECENT_KEY = 'sidekick:recent-prospects';
const RECENT_MAX = 5;

function loadRecent(): CrmPerson[] {
  try {
    const saved = JSON.parse(localStorage.getItem(RECENT_KEY) ?? '[]');
    return Array.isArray(saved) ? saved.slice(0, RECENT_MAX) : [];
  } catch {
    return [];
  }
}

function remember(person: CrmPerson) {
  try {
    const rest = loadRecent().filter((p) => p.id !== person.id);
    localStorage.setItem(RECENT_KEY, JSON.stringify([person, ...rest].slice(0, RECENT_MAX)));
  } catch {
    // Storage off: the list just stays empty.
  }
}

/**
 * What tells this person apart from someone of the same name. A CRM holds whichever of these it
 * happens to have, so a row shows the first two it has rather than a bare "No company".
 */
function details(person: CrmPerson): string {
  return [person.company_name, person.job_title, person.email, person.phone]
    .map((part) => part?.trim())
    .filter(Boolean)
    .slice(0, 2)
    .join(' · ');
}

/**
 * The line under the name in the results list. Where a CRM holds little about someone — and
 * several people of the same name with nothing on them is exactly when the rep needs help — the
 * deal they are on is the thing that tells them apart, so it is named rather than counted.
 */
function describe(person: CrmPerson): string {
  const deals = person.opportunities;
  const deal =
    deals.length === 0 ? '' : deals.length === 1 ? deals[0].name || '1 deal' : `${deals.length} deals`;
  return [details(person), deal].filter(Boolean).join(' · ') || 'Nothing else on record';
}

export function CrmProspectPicker({
  prospect,
  onProspectChange,
  choice,
  onChoose,
  onKeyDown,
  autoFocus,
  optional = true,
}: {
  prospect: string;
  onProspectChange: (name: string) => void;
  choice: CrmChoice | null;
  onChoose: (choice: CrmChoice | null) => void;
  onKeyDown: (event: React.KeyboardEvent) => void;
  autoFocus?: boolean;
  /** False where the call can't be filed without a name, as on the summary screen. */
  optional?: boolean;
}) {
  const [results, setResults] = useState<CrmPerson[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [unreachable, setUnreachable] = useState(false);
  const [focused, setFocused] = useState(false);
  const [recent, setRecent] = useState<CrmPerson[]>([]);
  const [everyone, setEveryone] = useState<CrmPerson[] | null>(null);
  const [loadingDeals, setLoadingDeals] = useState(false);
  // Only the latest search may write results; a slow earlier one is dropped.
  const latest = useRef(0);

  useEffect(() => {
    let live = true;
    peopleList()
      .then((people) => live && setEveryone(people))
      .catch(() => {}); // No list: searches go to the CRM as before.
    return () => {
      live = false;
    };
  }, []);

  useEffect(() => {
    if (choice) return;
    const query = prospect.trim();
    if (query.length < 2) {
      setResults(null);
      return;
    }
    const attempt = ++latest.current;
    if (everyone) {
      const found = searchLocally(everyone, query);
      setResults(found);
      setSearching(false);
      setUnreachable(false);
      // Someone added to the CRM in the last few minutes isn't in the list yet; only then ask it.
      if (found.length > 0 || query.length < 3) return;
    }
    const early = instant(query);
    if (early) setResults(early);
    const cached = searched.get(query.toLowerCase());
    if (cached && Date.now() - cached.at < CACHE_MS) {
      // A search still in flight for older text won't clear the spinner, so this does.
      setSearching(false);
      setUnreachable(false);
      return;
    }
    const timer = setTimeout(async () => {
      setSearching(true);
      try {
        const people = await backend.crmSearchPeople(query);
        searched.set(query.toLowerCase(), { at: Date.now(), people });
        if (attempt === latest.current) {
          setResults(people);
          setUnreachable(false);
        }
      } catch {
        // No CRM, or it didn't answer: the field carries on as a plain name box.
        if (attempt === latest.current) {
          setResults(null);
          setUnreachable(true);
        }
      } finally {
        if (attempt === latest.current) setSearching(false);
      }
    }, SEARCH_AFTER_MS);
    return () => clearTimeout(timer);
  }, [prospect, choice, everyone]);

  // Someone from the people list, or remembered from an earlier call, shows at once; their
  // deals are read after, since the list doesn't carry them and remembered ones may have moved.
  const pick = (person: CrmPerson, readDeals: boolean) => {
    latest.current++;
    setResults(null);
    remember(person);
    onChoose({ person, deal: person.opportunities[0] ?? null });
    if (!readDeals) return;
    const attempt = latest.current;
    setLoadingDeals(true);
    backend
      .crmPersonDeals(person.id)
      .then((opportunities) => {
        // Not if the rep has since cleared it, picked a deal, or picked someone else.
        if (attempt !== latest.current) return;
        const fresh = { ...person, opportunities };
        remember(fresh);
        onChoose({ person: fresh, deal: opportunities[0] ?? null });
      })
      .catch(() => {})
      .finally(() => {
        if (attempt === latest.current) setLoadingDeals(false);
      });
  };

  const clear = () => {
    latest.current++;
    setResults(null);
    setLoadingDeals(false);
    onChoose(null);
  };

  if (choice) {
    const { person, deal } = choice;
    return (
      <div className="space-y-2">
        <Label>Prospect</Label>
        <div className="rounded-md border border-blue-600/40 bg-blue-600/5 px-3 py-2">
          <div className="flex items-start gap-2">
            <Check className="mt-0.5 h-4 w-4 shrink-0 text-blue-600" aria-hidden="true" />
            <div className="min-w-0 flex-1">
              <p className="truncate text-sm font-medium text-gray-900">{person.name}</p>
              <p className="truncate text-xs text-gray-500">
                {details(person) || 'In your CRM'}
              </p>
            </div>
            <button
              type="button"
              onClick={clear}
              aria-label="Use a name instead"
              className="rounded p-1 text-gray-400 transition-colors hover:bg-gray-100 hover:text-gray-600"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
          {person.opportunities.length > 1 && (
            <div className="mt-2 space-y-1 border-t border-blue-600/20 pt-2">
              <p className="text-xs text-gray-500">Which deal is this call about?</p>
              {person.opportunities.map((option) => (
                <button
                  key={option.id}
                  type="button"
                  onClick={() => {
                    latest.current++;
                    onChoose({ person, deal: option });
                  }}
                  className={`flex w-full items-center gap-2 rounded px-2 py-1 text-left text-xs transition-colors ${
                    option.id === deal?.id
                      ? 'bg-blue-600/15 text-gray-900'
                      : 'text-gray-600 hover:bg-gray-100'
                  }`}
                >
                  <span className="truncate">{option.name || 'Untitled deal'}</span>
                  {option.stage && (
                    <span className="ml-auto shrink-0 text-[11px] uppercase tracking-wide text-gray-400">
                      {option.stage.replace(/_/g, ' ').toLowerCase()}
                    </span>
                  )}
                </button>
              ))}
            </div>
          )}
          {person.opportunities.length === 1 && deal && (
            <p className="mt-2 border-t border-blue-600/20 pt-2 text-xs text-gray-500">
              Deal: {deal.name || 'Untitled deal'}
            </p>
          )}
          {person.opportunities.length === 0 && loadingDeals && (
            <p className="mt-2 flex items-center gap-1.5 border-t border-blue-600/20 pt-2 text-xs text-gray-500">
              <Loader2 className="h-3 w-3 animate-spin" aria-hidden="true" />
              Looking up their deals…
            </p>
          )}
          {person.opportunities.length === 0 && !loadingDeals && (
            <p className="mt-2 border-t border-blue-600/20 pt-2 text-xs text-gray-500">
              No open deal yet — this call will start one.
            </p>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-2">
      <Label htmlFor="call-prospect">
        Prospect {optional && <span className="font-normal text-gray-400">(optional)</span>}
      </Label>
      <div className="relative">
        <Input
          id="call-prospect"
          value={prospect}
          onChange={(event) => onProspectChange(event.target.value)}
          onKeyDown={onKeyDown}
          onFocus={() => {
            setRecent(loadRecent());
            setFocused(true);
          }}
          onBlur={() => setFocused(false)}
          placeholder="Name, or search your CRM"
          autoFocus={autoFocus}
          autoComplete="off"
        />
        <span className="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 text-gray-300">
          {searching ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
          ) : (
            <Search className="h-3.5 w-3.5" aria-hidden="true" />
          )}
        </span>
      </div>
      {focused && prospect.trim().length < 2 && recent.length > 0 && (
        // mousedown would blur the field and hide the list before the click lands.
        <ul
          onMouseDown={(event) => event.preventDefault()}
          className="max-h-44 overflow-y-auto rounded-md border border-gray-200 bg-white py-1 shadow-sm"
        >
          <li className="px-3 pb-1 pt-0.5 text-[11px] uppercase tracking-wide text-gray-400">
            Recent
          </li>
          {recent.map((person) => (
            <li key={person.id}>
              <button
                type="button"
                onClick={() => pick(person, true)}
                className="flex w-full items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-gray-50"
              >
                <Building2 className="h-3.5 w-3.5 shrink-0 text-gray-300" aria-hidden="true" />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm text-gray-900">{person.name}</span>
                  <span className="block truncate text-xs text-gray-500">
                    {describe(person)}
                  </span>
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
      {results !== null && results.length > 0 && (
        <ul className="max-h-44 overflow-y-auto rounded-md border border-gray-200 bg-white py-1 shadow-sm">
          {results.map((person) => (
            <li key={person.id}>
              <button
                type="button"
                onClick={() => pick(person, everyone !== null)}
                className="flex w-full items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-gray-50"
              >
                <Building2 className="h-3.5 w-3.5 shrink-0 text-gray-300" aria-hidden="true" />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm text-gray-900">{person.name}</span>
                  <span className="block truncate text-xs text-gray-500">
                    {describe(person)}
                  </span>
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
      {results !== null && results.length === 0 && !searching && (
        <p className="text-xs text-gray-400">Not in the CRM — the call will create them.</p>
      )}
      {unreachable && <p className="text-xs text-gray-400">CRM search unavailable.</p>}
    </div>
  );
}
