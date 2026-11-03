'use client';

// The sign-in screen, shown before the app when a workspace backend is configured.
//
// Layout follows shadcn/ui's `login-05` block (MIT): a centred column, the mark above the
// heading, and a field group. Sidekick changes: our own mark and copy, a password field (the
// block is email-only), and no Apple or Google buttons, since the backend only does email and
// password today. A skip link keeps the app usable on a machine that shouldn't sign in.

import { useState } from 'react';
import { Loader2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Field, FieldDescription, FieldGroup, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { backend, type BackendStatus } from '@/lib/sidekick';

export function SignIn({
  status,
  onSignedIn,
  onSkip,
}: {
  status: BackendStatus;
  onSignedIn: () => void;
  onSkip: () => void;
}) {
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await backend.backendLogin(email.trim(), password);
      onSignedIn();
    } catch (problem) {
      setError(String(problem).replace(/^Error:\s*/, ''));
      setBusy(false);
    }
  };

  return (
    <div className="flex min-h-svh flex-col items-center justify-center gap-6 bg-gray-50 p-6 md:p-10">
      <div className="w-full max-w-sm">
        <form onSubmit={submit}>
          <FieldGroup>
            <div className="flex flex-col items-center gap-3 text-center">
              <svg className="size-12" viewBox="0 0 64 64" fill="none" aria-hidden="true">
                <rect width="64" height="64" rx="14" fill="#0D1117" />
                <rect x="19" y="8" width="10" height="48" rx="5" fill="#F6F5F1" />
                <rect x="35" y="20" width="10" height="24" rx="5" fill="#7651E6" />
              </svg>
              <h1 className="text-xl font-bold">Sign in to Sidekick</h1>
              <FieldDescription>
                Your workspace&apos;s playbook and settings arrive with your account, and your calls
                go to your team.
              </FieldDescription>
            </div>

            <Field>
              <FieldLabel htmlFor="email">Email</FieldLabel>
              <Input
                id="email"
                type="email"
                autoComplete="username"
                placeholder="you@company.com"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                required
                autoFocus
              />
            </Field>

            <Field>
              <FieldLabel htmlFor="password">Password</FieldLabel>
              <Input
                id="password"
                type="password"
                autoComplete="current-password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                required
              />
            </Field>

            {error && (
              <p role="alert" className="text-sm text-red-600">
                {error}
              </p>
            )}

            <Field>
              <Button type="submit" disabled={busy || !email || !password}>
                {busy && <Loader2 className="animate-spin" />}
                {busy ? 'Signing in...' : 'Sign in'}
              </Button>
            </Field>

            <FieldDescription className="text-center">
              No account yet? Ask your workspace admin to create one.
            </FieldDescription>
          </FieldGroup>
        </form>

        <div className="mt-6 text-center">
          <button
            type="button"
            onClick={onSkip}
            className="text-xs text-gray-500 underline-offset-4 hover:text-gray-800 hover:underline"
          >
            Use this computer without signing in
          </button>
          <p className="mt-1 text-[11px] text-gray-400">
            Calls and settings stay on this machine. {status.url}
          </p>
        </div>
      </div>
    </div>
  );
}
