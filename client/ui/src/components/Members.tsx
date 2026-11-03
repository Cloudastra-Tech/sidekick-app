'use client';

// Who is in the workspace, for admins. Adding someone returns a one-time password that is shown
// once, here, for the admin to pass on: there is no email sender yet, and a password that can be
// re-read later is one that can be found later.
//
// A member with calls is never deleted, only deactivated, so their call history keeps its owner.

import { useCallback, useEffect, useState } from 'react';
import { Check, Copy, KeyRound, Loader2, Trash2, UserPlus } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Switch } from '@/components/ui/switch';
import { backend, type Member, type NewMember } from '@/lib/sidekick';

function OneTimePassword({ created, onDone }: { created: NewMember; onDone: () => void }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    await navigator.clipboard.writeText(created.temporary_password);
    setCopied(true);
    toast.success('Password copied; send it to them directly');
  };

  return (
    <div className="space-y-2 rounded-lg bg-blue-50 p-4 text-sm text-blue-900">
      <p className="font-semibold">
        One-time password for {created.member.email}
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <code className="rounded bg-white px-2 py-1 font-mono text-base tracking-wide text-gray-900">
          {created.temporary_password}
        </code>
        <Button variant="outline" size="sm" onClick={copy}>
          {copied ? <Check /> : <Copy />}
          {copied ? 'Copied' : 'Copy'}
        </Button>
        <Button variant="ghost" size="sm" className="ml-auto" onClick={onDone}>
          Done
        </Button>
      </div>
      <p className="text-xs text-blue-800">
        Send it to them yourself. It won&apos;t be shown again, and they should change it after
        signing in.
      </p>
    </div>
  );
}

export function Members() {
  const [members, setMembers] = useState<Member[] | null>(null);
  // Your own row: the backend refuses self-demotion, so the control is disabled rather than
  // offered and then rejected.
  const [me, setMe] = useState<string | null>(null);
  const [email, setEmail] = useState('');
  const [name, setName] = useState('');
  const [asAdmin, setAsAdmin] = useState(false);
  const [busy, setBusy] = useState(false);
  const [created, setCreated] = useState<NewMember | null>(null);

  const load = useCallback(() => {
    backend
      .listMembers()
      .then(setMembers)
      .catch((error) => toast.error(String(error)));
  }, []);

  useEffect(load, [load]);

  useEffect(() => {
    backend
      .backendStatus()
      .then((status) => setMe(status.email))
      .catch(() => {});
  }, []);

  const act = async (work: () => Promise<unknown>, done?: string) => {
    setBusy(true);
    try {
      await work();
      if (done) toast.success(done);
      load();
    } catch (error) {
      toast.error(String(error).replace(/^Error:\s*/, ''));
    } finally {
      setBusy(false);
    }
  };

  const add = async (event: React.FormEvent) => {
    event.preventDefault();
    await act(async () => {
      const result = await backend.addMember(email.trim(), name.trim(), asAdmin);
      setCreated(result);
      setEmail('');
      setName('');
      setAsAdmin(false);
    });
  };

  return (
    <div className="space-y-4">
      {created && <OneTimePassword created={created} onDone={() => setCreated(null)} />}

      <form onSubmit={add} className="flex flex-wrap items-end gap-3 rounded-lg bg-gray-50 p-4">
        <div className="space-y-1.5">
          <Label htmlFor="member-email">Email</Label>
          <Input
            id="member-email"
            type="email"
            placeholder="rep@company.com"
            className="w-64"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            required
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="member-name">Name</Label>
          <Input
            id="member-name"
            placeholder="Optional"
            className="w-48"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </div>
        <label className="flex items-center gap-2 pb-2 text-sm text-gray-700">
          <Switch checked={asAdmin} onCheckedChange={setAsAdmin} />
          Admin
        </label>
        <Button type="submit" size="sm" disabled={busy || !email}>
          {busy ? <Loader2 className="animate-spin" /> : <UserPlus />}
          Add member
        </Button>
      </form>

      {members === null ? (
        <p className="text-sm text-gray-500">Loading members...</p>
      ) : (
        <ul className="divide-y divide-gray-200 rounded-lg border border-gray-200">
          {members.map((member) => (
            <li key={member.id} className="flex flex-wrap items-center gap-3 p-3 text-sm">
              <div className="min-w-0">
                <p className="truncate font-medium text-gray-900">
                  {member.name || member.email}
                  {member.email === me && (
                    <span className="ml-2 rounded-full bg-gray-100 px-2 py-0.5 text-[11px] text-gray-600">
                      You
                    </span>
                  )}
                  {!member.is_active && (
                    <span className="ml-2 rounded-full bg-amber-100 px-2 py-0.5 text-[11px] text-amber-800">
                      Deactivated
                    </span>
                  )}
                </p>
                <p className="truncate text-xs text-gray-500">
                  {member.name ? `${member.email} · ` : ''}
                  {member.calls === 1 ? '1 call' : `${member.calls} calls`}
                </p>
              </div>

              <div className="ml-auto flex items-center gap-2">
                <label
                  className="flex items-center gap-2 text-xs text-gray-600"
                  title={
                    member.email === me
                      ? "You can't change your own role"
                      : 'Admins manage members, workspace settings and the playbook'
                  }
                >
                  <Switch
                    checked={member.is_admin}
                    disabled={busy || member.email === me}
                    onCheckedChange={(next) =>
                      act(
                        () => backend.updateMember(member.id, { is_admin: next }),
                        next ? `${member.email} is now an admin` : `${member.email} is now a member`,
                      )
                    }
                  />
                  Admin
                </label>
                <label className="flex items-center gap-2 text-xs text-gray-600">
                  <Switch
                    checked={member.is_active}
                    disabled={busy || member.email === me}
                    onCheckedChange={(next) =>
                      act(
                        () => backend.updateMember(member.id, { is_active: next }),
                        next ? `${member.email} can sign in again` : `${member.email} deactivated`,
                      )
                    }
                  />
                  Can sign in
                </label>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={busy}
                  title="Give them a new one-time password"
                  onClick={() =>
                    act(async () => setCreated(await backend.resetMemberPassword(member.id)))
                  }
                >
                  <KeyRound />
                  Reset password
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={busy}
                  title={member.calls ? 'Members with calls are deactivated instead' : 'Remove'}
                  onClick={() =>
                    act(() => backend.removeMember(member.id), `${member.email} removed`)
                  }
                >
                  <Trash2 />
                </Button>
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
