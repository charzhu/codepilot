#!/usr/bin/env node
// Unified npm entry point for the Codepilot CLI.

import { spawn } from "node:child_process";
import { existsSync, realpathSync } from "fs";
import { createRequire } from "node:module";
import path from "path";
import { fileURLToPath } from "url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const require = createRequire(import.meta.url);

const TARGET_TRIPLE = "x86_64-pc-windows-msvc";
const PLATFORM_PACKAGE = "@charzhu/codepilot-win32-x64";

const { platform, arch } = process;
if (platform !== "win32" || arch !== "x64") {
  throw new Error(
    `Unsupported platform: ${platform} (${arch}). @charzhu/codepilot currently publishes a Windows x64 binary only.`,
  );
}

const codepilotBinaryName = "codepilot.exe";
const localVendorRoot = path.join(__dirname, "..", "vendor");
const localBinaryPath = path.join(
  localVendorRoot,
  TARGET_TRIPLE,
  "codex",
  codepilotBinaryName,
);

let vendorRoot;
try {
  const packageJsonPath = require.resolve(`${PLATFORM_PACKAGE}/package.json`);
  vendorRoot = path.join(path.dirname(packageJsonPath), "vendor");
} catch {
  if (existsSync(localBinaryPath)) {
    vendorRoot = localVendorRoot;
  } else {
    const updateCommand = installCommand();
    throw new Error(
      `Missing optional dependency ${PLATFORM_PACKAGE}. Reinstall Codepilot: ${updateCommand}`,
    );
  }
}

if (!vendorRoot) {
  const updateCommand = installCommand();
  throw new Error(
    `Missing optional dependency ${PLATFORM_PACKAGE}. Reinstall Codepilot: ${updateCommand}`,
  );
}

const archRoot = path.join(vendorRoot, TARGET_TRIPLE);
const binaryPath = path.join(archRoot, "codex", codepilotBinaryName);

function getUpdatedPath(newDirs) {
  const existingPath = process.env.PATH || "";
  const updatedPath = [
    ...newDirs,
    ...existingPath.split(";").filter(Boolean),
  ].join(";");
  return updatedPath;
}

function detectPackageManager() {
  const userAgent = process.env.npm_config_user_agent || "";
  if (/\bbun\//.test(userAgent)) {
    return "bun";
  }

  const execPath = process.env.npm_execpath || "";
  if (execPath.includes("bun")) {
    return "bun";
  }

  if (
    __dirname.includes(".bun/install/global") ||
    __dirname.includes(".bun\\install\\global")
  ) {
    return "bun";
  }

  return userAgent ? "npm" : null;
}

function installCommand() {
  return detectPackageManager() === "bun"
    ? "bun install -g @charzhu/codepilot@latest"
    : "npm install -g @charzhu/codepilot@latest";
}

const additionalDirs = [];
const pathDir = path.join(archRoot, "path");
if (existsSync(pathDir)) {
  additionalDirs.push(pathDir);
}
const updatedPath = getUpdatedPath(additionalDirs);

const env = { ...process.env, PATH: updatedPath };
// Keep the upstream CODEX_* names because Rust install-context detection uses them.
const packageManagerEnvVar =
  detectPackageManager() === "bun"
    ? "CODEX_MANAGED_BY_BUN"
    : "CODEX_MANAGED_BY_NPM";
env[packageManagerEnvVar] = "1";
env.CODEX_MANAGED_PACKAGE_ROOT = realpathSync(path.join(__dirname, ".."));

const child = spawn(binaryPath, process.argv.slice(2), {
  stdio: "inherit",
  env,
});

child.on("error", (err) => {
  console.error(err);
  process.exit(1);
});

const forwardSignal = (signal) => {
  if (child.killed) {
    return;
  }
  try {
    child.kill(signal);
  } catch {
    /* ignore */
  }
};

["SIGINT", "SIGTERM", "SIGHUP"].forEach((sig) => {
  process.on(sig, () => forwardSignal(sig));
});

const childResult = await new Promise((resolve) => {
  child.on("exit", (code, signal) => {
    if (signal) {
      resolve({ type: "signal", signal });
    } else {
      resolve({ type: "code", exitCode: code ?? 1 });
    }
  });
});

if (childResult.type === "signal") {
  process.kill(process.pid, childResult.signal);
} else {
  process.exit(childResult.exitCode);
}
