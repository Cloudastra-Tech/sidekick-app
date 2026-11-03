// Root document shared by the main window and the hint overlay. The app frame (sidebar,
// providers, toasts) lives in (main)/layout.tsx so the overlay window renders without it.

import '@/styles/google-sans.css';
import './theme.css';
import './globals.css';
import 'sonner/dist/styles.css';

export const metadata = { title: 'Sidekick' };

// Applies the saved theme before first paint, so a dark window never flashes white. The hint
// overlay is dark glass in every theme, so its page stays `color-scheme: dark`: a light scheme
// there made the engine paint the transparent window's background opaque white whenever the app
// was set to Light and Windows to dark, and the white glass hid the hint text.
const THEME_BOOT = `try{var c=localStorage.getItem('sidekick:theme')||'system';var d=c==='dark'||(c==='system'&&matchMedia('(prefers-color-scheme: dark)').matches);document.documentElement.classList.toggle('dark',d);if(location.pathname.indexOf('/overlay')===0)document.documentElement.style.colorScheme='dark'}catch(e){}`;

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: THEME_BOOT }} />
      </head>
      <body className="font-sans antialiased">{children}</body>
    </html>
  );
}
