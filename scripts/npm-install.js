#!/usr/bin/env node

/**
 * npm postinstall script for HintShell
 * Downloads the correct platform-specific binary from GitHub Releases.
 *
 * Windows note: running hintshell-core locks the .exe. We stop the daemon
 * before extracting / before hintshell init copies into ~/.hintshell.
 */

const https = require("https");
const fs = require("fs");
const path = require("path");
const { execSync, spawnSync } = require("child_process");
const os = require("os");

const REPO = "philau2512/hintshell";
const VERSION = require("../package.json").version;
const TAG = `v${VERSION}`;
const IS_WIN = os.platform() === "win32";

const PLATFORM_MAP = {
  "win32-x64": "x86_64-pc-windows-msvc",
  "linux-x64": "x86_64-unknown-linux-gnu",
  "linux-arm64": "aarch64-unknown-linux-gnu",
  "darwin-x64": "x86_64-apple-darwin",
  "darwin-arm64": "aarch64-apple-darwin",
};

const EXT_MAP = {
  win32: ".zip",
  linux: ".tar.gz",
  darwin: ".tar.gz",
};

function getPlatformKey() {
  const platform = os.platform();
  const arch = os.arch();
  return `${platform}-${arch}`;
}

function getDownloadUrl(target, ext) {
  return `https://github.com/${REPO}/releases/download/${TAG}/hintshell-${target}${ext}`;
}

function downloadFile(url, dest) {
  return new Promise((resolve, reject) => {
    const follow = (url) => {
      https.get(url, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          follow(res.headers.location);
          return;
        }
        if (res.statusCode !== 200) {
          reject(new Error(`Download failed: HTTP ${res.statusCode} from ${url}`));
          return;
        }
        const file = fs.createWriteStream(dest);
        res.pipe(file);
        file.on("finish", () => {
          file.close();
          resolve();
        });
        file.on("error", reject);
      }).on("error", reject);
    };
    follow(url);
  });
}

function extractArchive(archivePath, destDir) {
  if (os.platform() === "win32") {
    // Windows 10+ has tar (bsdtar) built-in which fully supports zip files.
    execSync(`tar -xf "${archivePath}" -C "${destDir}"`, { stdio: "inherit" });
  } else {
    execSync(`tar xzf "${archivePath}" -C "${destDir}"`, { stdio: "inherit" });
  }
}

function binName(base) {
  return IS_WIN ? `${base}.exe` : base;
}

/**
 * Stop running daemon so Windows can overwrite vendor/ and ~/.hintshell binaries.
 * Best-effort: never throw.
 */
function stopRunningDaemon(reason) {
  console.log(`🛑 Stopping HintShell daemon (${reason})...`);
  const home = os.homedir();
  const candidates = [
    path.join(home, ".hintshell", "bin", binName("hintshell")),
    path.join(home, ".hintshell", "module", binName("hintshell")),
    path.join(__dirname, "..", "vendor", binName("hintshell")),
  ];

  for (const cli of candidates) {
    if (!fs.existsSync(cli)) continue;
    try {
      spawnSync(cli, ["stop"], {
        encoding: "utf8",
        timeout: 8000,
        windowsHide: true,
      });
      break;
    } catch (_) {
      /* try next */
    }
  }

  // Always force-kill leftovers (Windows file lock on hintshell-core.exe)
  try {
    if (IS_WIN) {
      spawnSync("taskkill", ["/F", "/IM", "hintshell-core.exe"], {
        stdio: "ignore",
        windowsHide: true,
        timeout: 5000,
      });
      spawnSync(
        "powershell",
        [
          "-NoProfile",
          "-Command",
          "Get-Process -Name 'hintshell-core' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue",
        ],
        { stdio: "ignore", windowsHide: true, timeout: 5000 }
      );
    } else {
      spawnSync("pkill", ["-f", "hintshell-core"], {
        stdio: "ignore",
        timeout: 5000,
      });
    }
  } catch (_) {
    /* ignore */
  }

  // Brief wait so OS releases file handles
  const until = Date.now() + 400;
  while (Date.now() < until) {
    /* spin — avoid Atomics/SharedArrayBuffer quirks */
  }

  console.log("   Daemon stop attempted (IPC + force kill).");
}

function runInit(vendorDir) {
  const cli = path.join(vendorDir, binName("hintshell"));
  if (!fs.existsSync(cli)) {
    console.log("⚠️  vendor CLI missing; skip auto-init. Run: hintshell init");
    return;
  }
  console.log("📦 Running hintshell init (copy into ~/.hintshell + hooks)...");
  try {
    execSync(`"${cli}" init`, { stdio: "inherit", windowsHide: true });
  } catch (err) {
    console.error("⚠️  hintshell init failed. Run manually: hintshell init");
    if (err && err.message) console.error(`   ${err.message}`);
  }
}

async function main() {
  const platformKey = getPlatformKey();
  const target = PLATFORM_MAP[platformKey];

  if (!target) {
    console.error(`❌ Unsupported platform: ${platformKey}`);
    console.error(`   Supported: ${Object.keys(PLATFORM_MAP).join(", ")}`);
    process.exit(1);
  }

  const ext = EXT_MAP[os.platform()];
  const url = getDownloadUrl(target, ext);
  const installDir = path.join(__dirname, "..", "vendor");
  const archivePath = path.join(os.tmpdir(), `hintshell-${target}${ext}`);

  console.log(`📦 Installing HintShell ${VERSION} for ${platformKey}...`);
  console.log(`   Downloading from: ${url}`);

  // CRITICAL: release Windows locks before replacing vendor/*.exe
  stopRunningDaemon("before npm extract");

  fs.mkdirSync(installDir, { recursive: true });

  try {
    await downloadFile(url, archivePath);
    console.log(`   Extracting...`);
    extractArchive(archivePath, installDir);

    // Make binaries executable on Unix
    if (os.platform() !== "win32") {
      const bins = ["hintshell", "hintshell-core", "hs"];
      for (const bin of bins) {
        const binPath = path.join(installDir, bin);
        if (fs.existsSync(binPath)) {
          fs.chmodSync(binPath, 0o755);
        }
      }
    }

    try {
      fs.unlinkSync(archivePath);
    } catch (_) {
      /* ignore */
    }

    console.log(`✅ HintShell package binaries ready in vendor/`);

    // Copy into ~/.hintshell so profile/hooks use the new build (not stale bin/)
    // Skip when HINTSHELL_SKIP_INIT=1 (CI / packaging)
    if (process.env.HINTSHELL_SKIP_INIT !== "1") {
      stopRunningDaemon("before hintshell init");
      runInit(installDir);
    } else {
      console.log(`   HINTSHELL_SKIP_INIT=1 — skipped auto init.`);
      console.log(`   Run 'hintshell init' to configure your shell.`);
    }

    console.log(`✅ HintShell install/update finished.`);
  } catch (err) {
    console.error(`❌ Installation failed: ${err.message}`);
    console.error(
      `   Tip: stop daemon first:  hs stop   (or taskkill /F /IM hintshell-core.exe)`
    );
    console.error(`   Then: npm i -g hintshell@latest && hintshell init`);
    console.error(`   Manual download: https://github.com/${REPO}/releases`);
    process.exit(1);
  }
}

main();