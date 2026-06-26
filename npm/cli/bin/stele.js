#!/usr/bin/env node
"use strict";

// Thin launcher: resolve the right native `stele` binary for this platform and
// exec it, forwarding all args + stdio. The binary itself ships in a per-platform
// optional dependency (@stelegen/cli-<platform>-<arch>), so npm only downloads
// the one that matches the install machine.
const { spawnSync } = require("node:child_process");
const path = require("node:path");
const fs = require("node:fs");

function findBinary() {
  if (process.env.STELE_BINARY) return process.env.STELE_BINARY;
  const { platform, arch } = process;
  const exe = platform === "win32" ? "stele.exe" : "stele";
  // 1) Published per-platform package (the esbuild/swc model).
  try {
    return require.resolve(`@stelegen/cli-${platform}-${arch}/${exe}`);
  } catch {
    /* not installed — fall through */
  }
  // 2) Local/dev fallback: a binary vendored inside this package (not published).
  const local = path.join(__dirname, "..", "vendor", exe);
  if (fs.existsSync(local)) return local;
  return null;
}

const bin = findBinary();
if (!bin) {
  console.error(
    `stele: no prebuilt binary for ${process.platform}-${process.arch}. ` +
      `Set STELE_BINARY, or install @stelegen/cli-${process.platform}-${process.arch}.`,
  );
  process.exit(1);
}

const result = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(`stele: failed to launch ${bin}: ${result.error.message}`);
  process.exit(1);
}
process.exit(result.status ?? 1);
