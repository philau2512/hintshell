#!/usr/bin/env node

/**
 * npm postinstall script for HintShell
 * Downloads the correct platform-specific binary from GitHub Releases.
 */

const https = require("https");
const fs = require("fs");
const path = require("path");
const { execSync } = require("child_process");
const os = require("os");

const REPO = "philau2512/hintshell";
const VERSION = require("../package.json").version;
const TAG = `v${VERSION}`;

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
        file.on("finish", () => { file.close(); resolve(); });
        file.on("error", reject);
      }).on("error", reject);
    };
    follow(url);
  });
}

function extractArchive(archivePath, destDir) {
  const platform = os.platform();
  if (platform === "win32") {
    // Windows 10+ has tar (bsdtar) built-in which fully supports zip files.
    // It avoids execution policy and auto-load module issues from PowerShell's Expand-Archive.
    execSync(`tar -xf "${archivePath}" -C "${destDir}"`, { stdio: "inherit" });
  } else {
    execSync(`tar xzf "${archivePath}" -C "${destDir}"`, { stdio: "inherit" });
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

  console.log(`📦 Installing HintShell for ${platformKey}...`);
  console.log(`   Downloading from: ${url}`);

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

    // Cleanup
    fs.unlinkSync(archivePath);

    console.log(`✅ HintShell installed successfully!`);
    console.log(`   Run 'hintshell init' to configure your shell.`);
  } catch (err) {
    console.error(`❌ Installation failed: ${err.message}`);
    console.error(`   You can manually download from: https://github.com/${REPO}/releases`);
    process.exit(1);
  }
}

main();
