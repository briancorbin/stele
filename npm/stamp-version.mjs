#!/usr/bin/env node
// Stamp one version across every npm package (the cli launcher + all platform
// packages) and keep the cli's optionalDependencies pinned to the same version.
// Usage: node npm/stamp-version.mjs 0.0.2
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const version = process.argv[2];
if (!version) {
  console.error("usage: stamp-version.mjs <version>");
  process.exit(1);
}

const here = dirname(fileURLToPath(import.meta.url));
const platforms = [
  "darwin-arm64",
  "darwin-x64",
  "linux-x64",
  "linux-arm64",
  "win32-x64",
];

function patch(pkgDir, fn) {
  const path = join(here, pkgDir, "package.json");
  const json = JSON.parse(readFileSync(path, "utf8"));
  fn(json);
  writeFileSync(path, JSON.stringify(json, null, 2) + "\n");
}

for (const plat of platforms) {
  patch(plat, (j) => {
    j.version = version;
  });
}

patch("cli", (j) => {
  j.version = version;
  j.optionalDependencies = Object.fromEntries(
    platforms.map((p) => [`@stelegen/cli-${p}`, version]),
  );
});

console.log(`stamped all @stelegen packages to ${version}`);
