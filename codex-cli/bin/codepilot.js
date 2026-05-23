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
const packageBinaryPath = (vendorRoot) =>
  path.join(vendorRoot, TARGET_TRIPLE, "bin", codepilotBinaryName);
const legacyBinaryPath = (vendorRoot) =>
  path.join(vendorRoot, TARGET_TRIPLE, "codex", codepilotBinaryName);

function resolveNativePackage(vendorRoot) {
  const packageRoot = path.join(vendorRoot, TARGET_TRIPLE);
  const binaryPath = packageBinaryPath(vendorRoot);
  if (existsSync(binaryPath)) {
    return {
      binaryPath,
      pathDir: path.join(packageRoot, "codex-path"),
    };
  }

  const legacyPath = legacyBinaryPath(vendorRoot);
  if (existsSync(legacyPath)) {
    return {
      binaryPath: legacyPath,
      pathDir: path.join(packageRoot, "path"),
    };
  }

  return null;
}

let nativePackage;
try {
  const packageJsonPath = require.resolve(`${PLATFORM_PACKAGE}/package.json`);
  nativePackage = resolveNativePackage(
    path.join(path.dirname(packageJsonPath), "vendor"),
  );
} catch {
  nativePackage = resolveNativePackage(localVendorRoot);
}

if (!nativePackage) {
  const updateCommand = installCommand();
  throw new Error(
    `Missing optional dependency ${PLATFORM_PACKAGE}. Reinstall Codepilot: ${updateCommand}`,
  );
}

const { binaryPath, pathDir } = nativePackage;

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
