#!/usr/bin/env node

/**
 * npm postinstall script for HintShell
 * Downloads the correct platform-specific binary from GitHub Releases.
 *
 * Windows note: running hintshell-core locks the .exe. We stop the daemon
 * before extracting / before hintshell init copies into ~/.hintshell.
 */

const https = require("https");
const http = require("http");
const fs = require("fs");
const path = require("path");
const { fileURLToPath } = require("url");
const { execFileSync } = require("child_process");
const os = require("os");
const { stopRunningDaemon } = require("./npm-stop");

const REQUEST_TIMEOUT_MS = 30_000;
const MAX_REDIRECTS = 5;

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
  const overrideUrl = process.env.HINTSHELL_ASSET_URL;
  if (overrideUrl) {
    return overrideUrl
      .replace("{target}", target)
      .replace("{ext}", ext);
  }
  return `https://github.com/${REPO}/releases/download/${TAG}/hintshell-${target}${ext}`;
}

function downloadFile(url, dest, options = {}) {
  const requestTimeoutMs = options.requestTimeoutMs ?? REQUEST_TIMEOUT_MS;
  let parsedUrl;
  try {
    parsedUrl = new URL(url);
  } catch {
    return Promise.reject(new Error(`Download failed: invalid URL ${url}`));
  }
  if (parsedUrl.protocol === "file:") {
    return new Promise((resolve, reject) => {
      fs.copyFile(fileURLToPath(url), dest, (error) => (error ? reject(error) : resolve()));
    });
  }
  const maxRedirects = options.maxRedirects ?? MAX_REDIRECTS;

  return new Promise((resolve, reject) => {
    let settled = false;
    let activeRequest;
    let activeFile;
    let timeout;

    const finish = (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      activeRequest?.destroy();
      activeFile?.destroy();
      if (error) {
        try {
          fs.unlinkSync(dest);
        } catch (_) {
          /* no partial archive */
        }
        reject(error);
      } else {
        resolve();
      }
    };

    const follow = (nextUrl, redirects = 0) => {
      if (redirects > maxRedirects) {
        finish(new Error(`Download failed: exceeded ${maxRedirects} redirects`));
        return;
      }

      console.log(`   Fetching release asset${redirects ? ` (redirect ${redirects})` : ""}...`);
      const transport = new URL(nextUrl).protocol === "http:" ? http : https;
      activeRequest = transport.get(nextUrl, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          res.resume();
          follow(new URL(res.headers.location, nextUrl).toString(), redirects + 1);
          return;
        }
        if (res.statusCode !== 200) {
          res.resume();
          finish(new Error(`Download failed: HTTP ${res.statusCode} from ${nextUrl}`));
          return;
        }

        const totalBytes = Number(res.headers["content-length"] || 0);
        let downloadedBytes = 0;
        let lastReportedBytes = 0;
        activeFile = fs.createWriteStream(dest);
        res.on("data", (chunk) => {
          downloadedBytes += chunk.length;
          if (downloadedBytes - lastReportedBytes >= 5 * 1024 * 1024) {
            lastReportedBytes = downloadedBytes;
            const suffix = totalBytes
              ? `/${Math.ceil(totalBytes / 1024 / 1024)} MB`
              : " MB";
            console.log(`   Downloaded ${Math.floor(downloadedBytes / 1024 / 1024)}${suffix}`);
          }
        });
        res.on("error", finish);
        activeFile.on("error", finish);
        activeFile.on("finish", () => activeFile.close(() => finish()));
        res.pipe(activeFile);
      });

      activeRequest.on("error", finish);
      activeRequest.setTimeout(requestTimeoutMs, () => {
        finish(new Error(`Download timed out after ${requestTimeoutMs / 1000}s`));
      });
    };

    timeout = setTimeout(() => {
      finish(new Error(`Download timed out after ${requestTimeoutMs / 1000}s`));
    }, requestTimeoutMs);
    follow(url);
  });
}

function extractArchive(archivePath, destDir) {
  if (IS_WIN) {
    const command = [
      "$ErrorActionPreference = 'Stop'",
      `Expand-Archive -LiteralPath '${archivePath.replace(/'/g, "''")}' -DestinationPath '${destDir.replace(/'/g, "''")}' -Force`,
    ].join("; ");
    execFileSync("powershell", ["-NoProfile", "-Command", command], { stdio: "inherit" });
    return;
  }

  execFileSync("tar", ["xzf", archivePath, "-C", destDir], { stdio: "inherit" });
}

function binName(base) {
  return IS_WIN ? `${base}.exe` : base;
}

function runInit(vendorDir) {
  const cli = path.join(vendorDir, binName("hintshell"));
  if (!fs.existsSync(cli)) {
    console.log("⚠️  vendor CLI missing; skip auto-init. Run: hintshell init");
    return;
  }
  console.log("📦 Running hintshell init (copy into ~/.hintshell + hooks)...");
  try {
    execFileSync(cli, ["init"], { stdio: "inherit", windowsHide: true });
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

if (require.main === module) {
  main();
}

module.exports = { downloadFile, getDownloadUrl };
