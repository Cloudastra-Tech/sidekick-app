'use client';

// Updates, the way an editor does it: a quiet dot in the sidebar rather than a banner over the
// call, and the details on hover. Nothing moves the layout, because an update landing mid-call
// shouldn't push the transcript around.
//
// The check repeats while the app is open (and when the window comes back to the front), so a
// build published this afternoon shows up without anyone restarting.
//
// Installers come from the workspace the user signed in to, and each build is signed with the
// release key the app carries, so a build that wasn't ours can't install. Never during a call.

import { createContext, useCallback, useContext, useEffect, useMemo, useState } from 'react';
import { Download, Loader2, RefreshCw, Sparkles } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { HoverCard, HoverCardContent, HoverCardTrigger } from '@/components/ui/hover-card';
import { SidebarMenuButton } from '@/components/ui/sidebar';
import { backend, type UpdateCheck as UpdateCheckResult } from '@/lib/sidekick';

/** How often to ask the workspace for a newer build while the app is open. */
const CHECK_EVERY_MS = 15 * 60 * 1000;

interface UpdateState {
  version: string | null;
  found: UpdateCheckResult | null;
  checking: boolean;
  installing: boolean;
  check: (quiet: boolean) => Promise<void>;
  install: () => Promise<void>;
  dismiss: () => void;
}

const UpdateContext = createContext<UpdateState | null>(null);

export function UpdateProvider({ children }: { children: React.ReactNode }) {
  const [version, setVersion] = useState<string | null>(null);
  const [found, setFound] = useState<UpdateCheckResult | null>(null);
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);

  useEffect(() => {
    backend
      .appVersion()
      .then(({ version }) => setVersion(version))
      .catch(() => {});
  }, []);

  const check = useCallback(async (quiet: boolean) => {
    setChecking(true);
    try {
      const result = await backend.checkForUpdate();
      setVersion(result.current_version);
      setFound(result.available ? result : null);
      if (!result.available && !quiet) toast.success('Sidekick is up to date');
    } catch (error) {
      // Signed out, or the workspace is unreachable: not worth interrupting anyone over.
      if (!quiet) toast.error(String(error).replace(/^Error:\s*/, ''));
    } finally {
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    check(true);
    const timer = setInterval(() => check(true), CHECK_EVERY_MS);
    const onFocus = () => check(true);
    window.addEventListener('focus', onFocus);
    return () => {
      clearInterval(timer);
      window.removeEventListener('focus', onFocus);
    };
  }, [check]);

  const install = useCallback(async () => {
    setInstalling(true);
    try {
      await backend.installUpdate();
    } catch (error) {
      toast.error(String(error).replace(/^Error:\s*/, ''));
      setInstalling(false);
    }
  }, []);

  const value = useMemo(
    () => ({
      version,
      found,
      checking,
      installing,
      check,
      install,
      dismiss: () => setFound(null),
    }),
    [version, found, checking, installing, check, install],
  );

  return <UpdateContext.Provider value={value}>{children}</UpdateContext.Provider>;
}

function useUpdate(): UpdateState {
  const state = useContext(UpdateContext);
  if (!state) throw new Error('UpdateProvider is missing above this component');
  return state;
}

/** The sidebar row: only there when an update is waiting. Hovering shows what it is. */
export function UpdateSidebarItem() {
  const { found, installing, install, dismiss } = useUpdate();
  if (!found) return null;

  return (
    <HoverCard openDelay={80} closeDelay={150}>
      <HoverCardTrigger asChild>
        <SidebarMenuButton
          onClick={install}
          disabled={installing}
          tooltip={`Sidekick ${found.version} is ready to install`}
        >
          {/* Quiet: a small icon and a violet dot, so it reads as a notice rather than an alarm. */}
          <span className="relative flex shrink-0">
            {installing ? (
              <Loader2 className="!size-3.5 animate-spin" />
            ) : (
              <Sparkles className="!size-3.5 text-muted-foreground" />
            )}
            {!installing && (
              <span className="absolute -right-1 -top-1 h-1.5 w-1.5 rounded-full bg-[#7651E6] ring-2 ring-sidebar" />
            )}
          </span>
          <span className="truncate">{installing ? 'Installing...' : 'Update ready'}</span>
        </SidebarMenuButton>
      </HoverCardTrigger>
      <HoverCardContent side="right" align="end" className="w-72">
        <p className="text-sm font-semibold">Sidekick {found.version}</p>
        <p className="mt-0.5 text-xs text-muted-foreground">You&apos;re on {found.current_version}</p>
        {found.notes && <p className="mt-2 text-xs leading-relaxed">{found.notes}</p>}
        <div className="mt-3 flex items-center gap-2">
          <Button size="sm" onClick={install} disabled={installing}>
            {installing ? <Loader2 className="animate-spin" /> : <Download />}
            {installing ? 'Installing...' : 'Install and restart'}
          </Button>
          <Button variant="ghost" size="sm" onClick={dismiss} disabled={installing}>
            Later
          </Button>
        </div>
        <p className="mt-2 text-[11px] text-muted-foreground">
          Installs when you say so, and never during a call.
        </p>
      </HoverCardContent>
    </HoverCard>
  );
}

/** Version and a manual check, for the About dialog. */
export function UpdateVersionLine() {
  const { version, checking, check } = useUpdate();
  return (
    <div className="flex items-center gap-2 text-xs text-gray-500">
      <span>Version {version ?? '...'}</span>
      <Button
        variant="ghost"
        size="sm"
        className="h-7 px-2"
        onClick={() => check(false)}
        disabled={checking}
      >
        <RefreshCw className={checking ? 'animate-spin' : undefined} />
        Check for updates
      </Button>
    </div>
  );
}
