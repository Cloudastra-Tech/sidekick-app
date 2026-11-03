'use client';

// App frame for the main window: shadcn/ui sidebar plus content, toasts at the bottom.

import { useCallback, useEffect, useState } from 'react';
import { Loader2 } from 'lucide-react';
import { Toaster } from 'sonner';
import { AppSidebar } from '@/components/AppSidebar';
import { SidebarInset, SidebarProvider } from '@/components/ui/sidebar';
import { TooltipProvider } from '@/components/ui/tooltip';
import { CallProvider } from '@/contexts/CallContext';
import { SignIn } from '@/components/SignIn';
import { UpdateProvider } from '@/components/UpdateCheck';
import { backend, type BackendStatus } from '@/lib/sidekick';
import { PageTransition } from '@/components/PageTransition';
import { useTheme } from '@/lib/theme';

/** Remembers a deliberate "use this computer only", so the gate doesn't ask on every start. */
const SKIP_KEY = 'sidekickSkipSignIn';

export default function MainLayout({ children }: { children: React.ReactNode }) {
  // The static export pre-renders every button, but none respond until React has loaded.
  // Show a loading screen until then instead of a page that looks ready but ignores clicks.
  const [ready, setReady] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [account, setAccount] = useState<BackendStatus | null>(null);
  const [skipped, setSkipped] = useState(false);
  const { resolved: theme } = useTheme();
  useEffect(() => {
    // shadcn's sidebar saves its state in the `sidebar_state` cookie but only reads it
    // server-side; with a static export, read it here so the choice survives restarts.
    setSidebarOpen(/(?:^|; )sidebar_state=true/.test(document.cookie));
    try {
      setSkipped(localStorage.getItem(SKIP_KEY) === 'true');
    } catch {}
    // A backend that can't be reached shouldn't lock anyone out of their own calls.
    backend
      .backendStatus()
      .then((status) => {
        setAccount(status);
        // Roles can change between sign-ins; ask the backend who we are now.
        if (status.signed_in) backend.backendRefreshAccount().then(setAccount).catch(() => {});
      })
      .catch(() => setAccount({ url: null, signed_in: false, email: null, workspace: null, is_admin: false }))
      .finally(() => setReady(true));
  }, []);

  const refreshAccount = useCallback(() => {
    backend.backendStatus().then(setAccount).catch(() => {});
  }, []);

  const needsSignIn = ready && account?.url && !account.signed_in && !skipped;

  return (
    <>
      {ready && needsSignIn && account ? (
        <SignIn
          status={account}
          onSignedIn={refreshAccount}
          onSkip={() => {
            try {
              localStorage.setItem(SKIP_KEY, 'true');
            } catch {}
            setSkipped(true);
          }}
        />
      ) : ready ? (
        <CallProvider>
          <TooltipProvider>
            <UpdateProvider>
            {/* Starts as the compact icon rail until the user expands it. */}
            <SidebarProvider defaultOpen={sidebarOpen}>
              {/* Asks the workspace for a newer build on start, every 15 minutes, and whenever the
                  window comes back to the front; the sidebar shows it. */}
              <AppSidebar />
              <SidebarInset className="min-w-0 overflow-hidden">
                <PageTransition>{children}</PageTransition>
              </SidebarInset>
            </SidebarProvider>
            </UpdateProvider>
          </TooltipProvider>
        </CallProvider>
      ) : (
        <div className="flex h-screen flex-col items-center justify-center gap-3 bg-gray-50 text-gray-500">
          <Loader2 className="h-8 w-8 animate-spin text-gray-400" />
          <p className="text-sm">Starting Sidekick...</p>
        </div>
      )}
      <Toaster position="bottom-center" richColors closeButton theme={theme} />
    </>
  );
}
