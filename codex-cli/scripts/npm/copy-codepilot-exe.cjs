#!/usr/bin/env node

const { copyFileSync, existsSync } = require("node:fs");
const path = require("node:path");

const packageRoot = path.resolve(__dirname, "..");
const binaryDir = path.join(
  packageRoot,
  "vendor",
  "x86_64-pc-windows-msvc",
  "codex",
);
const source = path.join(binaryDir, "codex.exe");
const destination = path.join(binaryDir, "codepilot.exe");

if (!existsSync(source)) {
  throw new Error(`Cannot create codepilot.exe because codex.exe is missing: ${source}`);
}

copyFileSync(source, destination);