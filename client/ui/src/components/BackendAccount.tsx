'use client';

// Signing in to the Sidekick backend, from Settings. Signing in pulls the workspace's settings
// and playbook, so a fresh install is configured by logging in rather than by editing files.
// With no backend URL set, this section stays hidden and the app works entirely on its own.

import { useEffect, useState } from 'react';
import { CloudOff, LogIn, LogOut, RefreshCw } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { backend, type BackendStatus } from '@/lib/sidekick';

export function BackendAccount({
  onSettingsChanged,
  onAccountChanged,
}: {
  onSettingsChanged: () => void;
  /** Lets the Settings page show the Members section once an admin signs in. */
  onAccountChanged?: (status: BackendStatus) => void;
}) {
  const [status, setStatus] = useState<BackendStatus | null>(null);
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    backend
      .backendStatus()
      .then((next) => {
        setStatus(next);
        onAccountChanged?.(next);
      })
      .catch(() => setStatus(null));
  }, [onAccountChanged]);

  if (!status?.url) return null;

  const signIn = async () => {
    setBusy(true);
    try {
      const result = await backend.backendLogin(email, password);
      setStatus(result.status);
      onAccountChanged?.(result.status);
      setPassword('');
      toast.success(`Signed in to ${result.status.workspace || 'the workspace'}; settings updated`);
      onSettingsChanged();
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(false);
    }
  };

  const signOut = async () => {
    await backend.backendLogout();
    const next = await backend.backendStatus();
    setStatus(next);
    onAccountChanged?.(next);
    toast.success('Signed out; this computer keeps its own settings');
  };

  const sync = async () => {
    setBusy(true);
    try {
      const result = await backend.backendSyncSettings();
      toast.success(`Settings updated from the workspace (playbook v${result.playbook_version})`);
      onSettingsChanged();
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(false);
    }
  };

  if (status.signed_in) {
    return (
      <div className="flex flex-wrap items-center gap-3 rounded-lg bg-gray-50 p-4 text-sm">
        <span className="text-gray-700">
          Signed in as <strong>{status.email}</strong>
          {status.workspace && ` · ${status.workspace}`}
        </span>
        <span className="text-xs text-gray-400">{status.url}</span>
        <Button variant="outline" size="sm" className="ml-auto" onClick={sync} disabled={busy}>
          <RefreshCw className={busy ? 'animate-spin' : undefined} />
          Update settings now
        </Button>
        <Button variant="ghost" size="sm" onClick={signOut} disabled={busy}>
          <LogOut />
          Sign out
        </Button>
      </div>
    );
  }

  return (
    <form
      className="space-y-3 rounded-lg bg-gray-50 p-4"
      onSubmit={(event) => {
        event.preventDefault();
        signIn();
      }}
    >
      <p className="flex items-center gap-2 text-sm text-gray-600">
        <CloudOff className="h-4 w-4 text-gray-400" />
        Not signed in. Calls and settings stay on this computer.
      </p>
      <div className="grid gap-3 sm:grid-cols-2 max-w-lg">
        <div className="space-y-1.5">
          <Label htmlFor="backend-email">Email</Label>
          <Input
            id="backend-email"
            type="email"
            autoComplete="username"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="backend-password">Password</Label>
          <Input
            id="backend-password"
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
        </div>
      </div>
      <Button type="submit" size="sm" disabled={busy || !email || !password}>
        <LogIn />
        {busy ? 'Signing in...' : 'Sign in'}
      </Button>
      <p className="text-xs text-gray-500">{status.url}</p>
    </form>
  );
}
