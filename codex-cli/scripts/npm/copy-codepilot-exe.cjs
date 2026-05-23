#!/usr/bin/env node

const { copyFileSync, existsSync } = require("node:fs");
const { spawnSync } = require("node:child_process");
const path = require("node:path");

const packageRoot = path.resolve(__dirname, "..");
const targetRoot = path.join(packageRoot, "vendor", "x86_64-pc-windows-msvc");
const binaryDir = existsSync(path.join(targetRoot, "bin", "codex.exe"))
  ? path.join(targetRoot, "bin")
  : path.join(targetRoot, "codex");
const source = path.join(binaryDir, "codex.exe");
const destination = path.join(binaryDir, "codepilot.exe");

if (!existsSync(source)) {
  throw new Error(`Cannot create codepilot.exe because codex.exe is missing: ${source}`);
}

copyFileSync(source, destination);

linkCodexAppToCodepilot(destination);

function linkCodexAppToCodepilot(binaryPath) {
  if (process.platform !== "win32") {
    return;
  }

  const command = `[Environment]::SetEnvironmentVariable('CODEX_CLI_PATH', ${quotePowerShellString(binaryPath)}, 'User')`;
  const encodedCommand = Buffer.from(command, "utf16le").toString("base64");
  const result = spawnSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-NonInteractive",
      "-ExecutionPolicy",
      "Bypass",
      "-EncodedCommand",
      encodedCommand,
    ],
    { encoding: "utf8", windowsHide: true },
  );

  if (result.error || result.status !== 0) {
    const reason = result.error?.message || result.stderr?.trim() || `exit code ${result.status}`;
    console.warn(
      `Codepilot installed, but failed to link the Codex app to ${binaryPath}: ${reason}`,
    );
    console.warn(`To link manually, run: ${command}`);
    return;
  }

  console.log(`Linked Codex app to Codepilot CLI: ${binaryPath}`);
  console.log("Restart the Codex app for this change to take effect.");
}

function quotePowerShellString(value) {
  return `'${value.replace(/'/g, "''")}'`;
}
