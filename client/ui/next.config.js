// Based on Meetily's frontend config (MIT, see MEETILY-LICENSE.md): static export for Tauri.
/** @type {import('next').NextConfig} */
const nextConfig = {
  output: 'export',
  images: { unoptimized: true },
  // Emit route/index.html so window URLs like `overlay/` resolve the same in dev and in builds.
  trailingSlash: true,
};

module.exports = nextConfig;
