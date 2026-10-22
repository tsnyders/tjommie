import type { NextConfig } from "next";

// A static export that the desktop app serves from app://tjommie. trailingSlash gives every page its own
// folder with an index.html, so the app protocol maps paths without guessing. The workspace packages ship
// TypeScript source, so Next compiles them.
const nextConfig: NextConfig = {
  output: "export",
  trailingSlash: true,
  transpilePackages: ["@tjommie/protocol", "@tjommie/client"],
  devIndicators: false,
};

export default nextConfig;
