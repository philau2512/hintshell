#!/usr/bin/env node

const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawnSync } = require("child_process");

const IS_WIN = os.platform() === "win32";

function binName(base) {
  return IS_WIN ? `${base}.exe` : base;
}

function run(command, args, options = {}) {
  try {
    return spawnSync(command, args, {
      stdio: "ignore",
      timeout: 5000,
      windowsHide: true,
      ...options,
    });
  } catch (_) {
    return undefined;
  }
}

function stopRunningDaemon(reason) {
  console.log(`🛑 Stopping HintShell daemon (${reason})...`);
  if (process.env.HINTSHELL_SKIP_PROCESS_CONTROL === "1") {
    console.log("   HINTSHELL_SKIP_PROCESS_CONTROL=1 — skipped for isolated test.");
    return;
  }

  const home = os.homedir();
  const hintshellHome = process.env.HINTSHELL_HOME || path.join(home, ".hintshell");
  const candidates = [
    path.join(hintshellHome, "bin", binName("hintshell")),
    path.join(hintshellHome, "module", binName("hintshell")),
    path.join(__dirname, "..", "vendor", binName("hintshell")),
  ];

  for (const cli of candidates) {
    if (!fs.existsSync(cli)) continue;
    run(cli, ["stop"], { encoding: "utf8", timeout: 8000 });
    break;
  }

  if (IS_WIN) {
    run("taskkill", ["/F", "/IM", "hintshell-core.exe"]);
    run("taskkill", ["/F", "/IM", "hintshell.exe"]);
    run("powershell", [
      "-NoProfile",
      "-Command",
      "Get-Process -Name 'hintshell', 'hintshell-core' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue",
    ]);
  } else {
    run("pkill", ["-f", "hintshell-core"]);
  }

  console.log("   Daemon stop attempted (IPC + force kill).");
}

if (require.main === module) {
  stopRunningDaemon("before npm install");
}

module.exports = { stopRunningDaemon };