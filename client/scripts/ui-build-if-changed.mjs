// Builds the UI (client/ui -> client/ui/out) only when UI sources are newer than the last build.
// `tauri dev` runs this so the app loads prebuilt pages instantly instead of waiting 30+ s for
// the Next.js dev server to compile. For hot-reload UI work, use `npm run tauri:ui-dev`.

import { execSync } from 'node:child_process';
import { existsSync, readdirSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ui = join(dirname(fileURLToPath(import.meta.url)), '..', 'ui');
const builtMarker = join(ui, 'out', 'index.html');
const inputs = [
  'src',
  'public',
  'package.json',
  'package-lock.json',
  'next.config.js',
  'tailwind.config.js',
  'postcss.config.js',
  'tsconfig.json',
];

function newestMtime(path) {
  if (!existsSync(path)) return 0;
  const stat = statSync(path);
  if (!stat.isDirectory()) return stat.mtimeMs;
  return readdirSync(path).reduce((newest, name) => Math.max(newest, newestMtime(join(path, name))), stat.mtimeMs);
}

const lastBuild = existsSync(builtMarker) ? statSync(builtMarker).mtimeMs : 0;
const lastChange = Math.max(...inputs.map((input) => newestMtime(join(ui, input))));

if (lastBuild > lastChange) {
  console.log('UI is up to date, skipping build.');
} else {
  console.log(existsSync(builtMarker) ? 'UI changed, rebuilding...' : 'No UI build yet, building...');
  if (!existsSync(join(ui, 'node_modules'))) {
    execSync('npm install --no-fund --no-audit', { cwd: ui, stdio: 'inherit' });
  }
  execSync('npm run build', { cwd: ui, stdio: 'inherit' });
}
