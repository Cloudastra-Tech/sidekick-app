'use client';

// Main-window navigation built from shadcn/ui's Sidebar (`npx shadcn@latest add sidebar`).
// Collapses to a 3rem icon rail; the trigger in the footer, the rail edge, or Ctrl+B expands it.

import { useEffect, useState } from 'react';
import Link from 'next/link';
import { usePathname, useRouter } from 'next/navigation';
import {
  Building2,
  ClipboardCheck,
  History,
  Home,
  Info,
  LogOut,
  Mic,
  Moon,
  NotebookPen,
  Settings,
  Square,
  Sun,
  TrendingUp,
  Users,
} from 'lucide-react';
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
  SidebarTrigger,
} from '@/components/ui/sidebar';
import { Dialog, DialogContent, DialogTitle, DialogTrigger } from '@/components/ui/dialog';
import { useCall } from '@/contexts/CallContext';
import { backend, onBackend, type BackendStatus } from '@/lib/sidekick';
import { UpdateSidebarItem, UpdateVersionLine } from '@/components/UpdateCheck';
import { useTheme } from '@/lib/theme';

const NAV = [
  { href: '/', label: 'Live call', icon: Home },
  { href: '/summary/', label: 'Call summary', icon: NotebookPen },
  { href: '/history/', label: 'Call history', icon: History },
  { href: '/clients/', label: 'Clients', icon: Building2 },
  { href: '/team-calls/', label: 'Team calls', icon: Users, adminOnly: true },
  { href: '/performance/', label: 'Rep performance', icon: TrendingUp, adminOnly: true },
  { href: '/test-calls/', label: 'Test calls', icon: ClipboardCheck },
  { href: '/settings/', label: 'Settings', icon: Settings },
];

export function AppSidebar() {
  const pathname = usePathname();
  const router = useRouter();
  const { status } = useCall();
  const isLive = status === 'live';
  const { resolved: theme, setChoice: setTheme } = useTheme();
  const [account, setAccount] = useState<BackendStatus | null>(null);

  useEffect(() => {
    backend.backendStatus().then(setAccount).catch(() => {});
  }, []);
  const nextTheme = theme === 'dark' ? 'light' : 'dark';

  // The live call page asks for consent and starts the call. From another page, leave a
  // note for it to pick up once it has mounted; an event would fire before it listens.
  const startCall = () => {
    if (pathname === '/') {
      window.dispatchEvent(new CustomEvent('sidekick:start-call'));
    } else {
      sessionStorage.setItem('sidekick:start-call', '1');
      router.push('/');
    }
  };

  // The overlay's "Start coaching" button: open the start-call dialog from any page.
  useEffect(() => {
    const unlisten = onBackend('start-call-requested', () => startCall());
    return () => {
      unlisten.then((stop) => stop());
    };
  });

  return (
    <Sidebar collapsible="icon">
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg" asChild tooltip="Sidekick">
              <Link href="/">
                <svg className="!size-8 shrink-0" viewBox="0 0 64 64" fill="none" aria-hidden="true">
                  <rect width="64" height="64" rx="14" fill="#0D1117" />
                  <rect x="19" y="8" width="10" height="48" rx="5" fill="#F6F5F1" />
                  <rect x="35" y="20" width="10" height="24" rx="5" fill="#7651E6" />
                </svg>
                <div className="grid flex-1 text-left text-sm leading-tight transition-opacity duration-200 group-data-[collapsible=icon]:opacity-0">
                  <span className="truncate font-semibold">Sidekick</span>
                  <span className="truncate text-xs text-muted-foreground">Live call coach</span>
                </div>
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>

      <SidebarContent>
        <SidebarGroup>
          <SidebarGroupContent>
            <SidebarMenu>
              <SidebarMenuItem>
                <SidebarMenuButton
                  onClick={startCall}
                  disabled={isLive}
                  tooltip={isLive ? 'Call in progress' : 'Start call'}
                  className="bg-[#7651E6] text-white hover:bg-[#6541D6] hover:text-white active:bg-[#6541D6] active:text-white disabled:opacity-60"
                >
                  {isLive ? <Square /> : <Mic />}
                  <span>{isLive ? 'Call in progress' : 'Start call'}</span>
                </SidebarMenuButton>
              </SidebarMenuItem>

              {NAV.filter((item) => !item.adminOnly || account?.is_admin).map((item) => (
                <SidebarMenuItem key={item.href}>
                  <SidebarMenuButton asChild isActive={pathname === item.href} tooltip={item.label}>
                    <Link href={item.href}>
                      <item.icon />
                      <span>{item.label}</span>
                    </Link>
                  </SidebarMenuButton>
                </SidebarMenuItem>
              ))}
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>

      <SidebarFooter>
        <SidebarMenu>
          <SidebarMenuItem>
            <Dialog>
              <DialogTrigger asChild>
                <SidebarMenuButton tooltip="About">
                  <Info />
                  <span>About</span>
                </SidebarMenuButton>
              </DialogTrigger>
              <DialogContent>
                <DialogTitle>About Sidekick</DialogTitle>
                <div className="space-y-3 text-sm text-gray-600">
                  <p>Live coaching for sales calls: hints while the prospect talks, and a summary when the call ends.</p>
                  <div className="grid grid-cols-2 gap-2">
                    <div className="bg-gray-50 rounded p-3">
                      <h3 className="font-bold text-sm text-gray-900 mb-1">Shortcuts</h3>
                      <p className="text-xs">
                        Ctrl+B toggles this sidebar. Ctrl+Shift+H shows or hides the hints overlay. Ctrl+Shift+L makes
                        it click-through.
                      </p>
                    </div>
                    <div className="bg-gray-50 rounded p-3">
                      <h3 className="font-bold text-sm text-gray-900 mb-1">Open source</h3>
                      <p className="text-xs">Interface based on Meetily, Anarlog, and shadcn/ui (MIT License).</p>
                    </div>
                  </div>
                  <UpdateVersionLine />
                </div>
              </DialogContent>
            </Dialog>
          </SidebarMenuItem>
          <SidebarMenuItem>
            <SidebarMenuButton
              onClick={() => setTheme(nextTheme)}
              tooltip={nextTheme === 'dark' ? 'Dark theme' : 'Light theme'}
            >
              {nextTheme === 'dark' ? <Moon /> : <Sun />}
              <span>{nextTheme === 'dark' ? 'Dark theme' : 'Light theme'}</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
          <SidebarMenuItem>
            <UpdateSidebarItem />
          </SidebarMenuItem>
          {account?.signed_in && (
            <SidebarMenuItem>
              <SidebarMenuButton
                onClick={async () => {
                  await backend.backendLogout();
                  // Back to the sign-in screen the next time the window opens.
                  try {
                    localStorage.removeItem('sidekickSkipSignIn');
                  } catch {}
                  location.reload();
                }}
                tooltip={`${account.email ?? ''}${account.workspace ? ` · ${account.workspace}` : ''}`}
              >
                <LogOut />
                <span className="truncate">{account.email ?? 'Sign out'}</span>
              </SidebarMenuButton>
            </SidebarMenuItem>
          )}
          <SidebarMenuItem>
            <SidebarTrigger className="ml-0.5" />
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarFooter>
      <SidebarRail />
    </Sidebar>
  );
}
