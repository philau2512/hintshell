#!/usr/bin/env node

/**
 * npm postinstall script for HintShell
 * Downloads the correct platform-specific binary from GitHub Releases.
 *
 * Windows note: running hintshell-core locks the .exe. We stop the daemon
 * before extracting / before hintshell init copies into ~/.hintshell.
 *
 * Extraction never relies on a single unpacker, because each one can be
 * unavailable on a real machine (see windowsExtractStrategies).
 */

const https = require("https");
const http = require("http");
const fs = require("fs");
const path = require("path");
const { fileURLToPath } = require("url");
const { execFileSync, spawnSync } = require("child_process");
const os = require("os");
const { stopRunningDaemon, waitForDaemonExit } = require("./npm-stop");

const REQUEST_TIMEOUT_MS = 30_000;
const MAX_REDIRECTS = 5;
const EXTRACT_TIMEOUT_MS = 120_000;
const LOCK_WAIT_MS = 8_000;

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

function formatMb(bytes) {
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

// GitHub answers with content-length, so a short body is a broken asset and
// must never reach the unpacker as a partial archive.
function sizeMismatchError(totalBytes, downloadedBytes) {
  if (!(totalBytes > 0) || downloadedBytes === totalBytes) {
    return null;
  }
  return new Error(`Download failed: incomplete release asset (${downloadedBytes} of ${totalBytes} bytes)`);
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
        const reportProgress = () => {
          const incomplete = sizeMismatchError(totalBytes, downloadedBytes);
          if (incomplete) {
            return finish(incomplete);
          }
          if (downloadedBytes >= 1024 * 1024) {
            console.log(`   Download complete: ${formatMb(downloadedBytes)}`);
          }
          finish();
        };
        activeFile = fs.createWriteStream(dest);
        res.on("data", (chunk) => {
          downloadedBytes += chunk.length;
          if (downloadedBytes - lastReportedBytes >= 5 * 1024 * 1024) {
            lastReportedBytes = downloadedBytes;
            const suffix = totalBytes
              ? `/${Math.ceil(totalBytes / 1024 / 1024)} MB`
              : " MB";
            // Milestone lines only: a 7 MB asset prints once, so the lack of a
            // final "7/7 MB" line is why this looked like a truncated download.
            console.log(`   Downloaded ${Math.floor(downloadedBytes / 1024 / 1024)}${suffix}`);
          }
        });
        res.on("error", finish);
        activeFile.on("error", finish);
        activeFile.on("finish", () => activeFile.close(() => reportProgress()));
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

function psQuote(value) {
  return `'${String(value).replace(/'/g, "''")}'`;
}

// PowerShell writes the reason across lines and puts the error id last, and it
// echoes the offending source line, so a bare first line is not enough to tell a
// module failure from a locked or corrupt archive.
const ERROR_DETAIL_LINES = 3;
const ERROR_DETAIL_MAX_CHARS = 400;
const ERROR_ECHO_LINE = /^(?:At(?: |$)|char:\d|[+~\-]|GAC\s|(?:True|False)\s|C:.*\.psm1[:\d])/i;
const ANSI_ESCAPE = /\x1b\[[0-9;?]*[A-Za-z]/g;

function summarizeError(text, fallback) {
  const lines = String(text || "")
    .replace(ANSI_ESCAPE, "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  if (lines.length === 0) {
    return fallback;
  }

  const message = lines.filter((line) => !ERROR_ECHO_LINE.test(line)).slice(0, ERROR_DETAIL_LINES);
  const errorId = lines.find((line) => /FullyQualifiedErrorId/i.test(line));
  const summary = (errorId ? message.concat(errorId) : message).join(" | ");

  return summary.length > ERROR_DETAIL_MAX_CHARS
    ? `${summary.slice(0, ERROR_DETAIL_MAX_CHARS - 3)}...`
    : summary;
}

// Windows ships bsdtar in System32, which reads zip; the `tar` first on PATH in
// Git Bash is GNU tar, which cannot. Only the absolute path is safe here.
function systemTarPath() {
  const root = process.env.SystemRoot || process.env.WINDIR || "C:\\Windows";
  return [path.join(root, "System32", "tar.exe"), path.join(root, "SysWOW64", "tar.exe")].find(
    (candidate) => fs.existsSync(candidate)
  );
}

// Entry-by-entry extraction through .NET: needs no PowerShell module at all,
// so a broken or policy-blocked Microsoft.PowerShell.Archive cannot stop it.
function dotnetZipScript(archivePath, destDir) {
  return [
    "$ErrorActionPreference = 'Stop'",
    "try { Add-Type -AssemblyName System.IO.Compression.FileSystem } catch { }",
    `$zip = [System.IO.Compression.ZipFile]::OpenRead(${psQuote(archivePath)})`,
    "try {",
    "  foreach ($entry in $zip.Entries) {",
    "    if ($entry.Name -eq '') { continue }",
    `    $target = Join-Path ${psQuote(destDir)} $entry.FullName`,
    "    $parent = Split-Path -Parent $target",
    "    if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }",
    "    [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $target, $true)",
    "  }",
    "} finally { $zip.Dispose() }",
  ].join("\n");
}

function powershellArgs(script) {
  return ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script];
}

/**
 * Unpackers ordered by how little of the local machine they trust. A machine
 * where one is broken (missing module, no pwsh, blocked by execution policy)
 * still gets the binaries instead of a failed install.
 */
function windowsExtractStrategies(archivePath, destDir) {
  const strategies = [];
  const systemTar = systemTarPath();
  if (systemTar) {
    strategies.push({
      label: "System32 tar.exe",
      command: systemTar,
      args: ["-xf", archivePath, "-C", destDir],
    });
  }

  const expandScript = [
    "$ErrorActionPreference = 'Stop'",
    `Expand-Archive -LiteralPath ${psQuote(archivePath)} -DestinationPath ${psQuote(destDir)} -Force`,
  ].join("; ");

  strategies.push(
    {
      label: ".NET ZipFile via Windows PowerShell",
      command: "powershell",
      args: powershellArgs(dotnetZipScript(archivePath, destDir)),
    },
    {
      label: "PowerShell 7 Expand-Archive",
      command: "pwsh",
      args: powershellArgs(expandScript),
    },
    {
      label: "Windows PowerShell Expand-Archive",
      command: "powershell",
      args: powershellArgs(expandScript),
    }
  );

  return strategies;
}

function runStrategy(strategy) {
  const result = spawnSync(strategy.command, strategy.args, {
    encoding: "utf8",
    timeout: EXTRACT_TIMEOUT_MS,
    windowsHide: true,
  });

  if (result.error) {
    const unavailable = result.error.code === "ENOENT";
    return {
      ok: false,
      skipped: unavailable,
      detail: unavailable ? `${strategy.command} is not installed` : result.error.message,
    };
  }
  if (result.status === 0) {
    return { ok: true };
  }
  return {
    ok: false,
    detail: summarizeError(
      `${result.stderr || ""}\n${result.stdout || ""}`,
      `exit code ${result.status}`
    ),
  };
}

function missingRequiredFiles(destDir) {
  return [binName("hintshell"), binName("hintshell-core")].filter(
    (bin) => !fs.existsSync(path.join(destDir, bin))
  );
}

function isFileLockError(detail) {
  return /being used by another process|sharing violation|cannot remove item|RemoveFileSystemItemIOError|Access is denied|EBUSY|EPERM/i.test(
    String(detail || "")
  );
}

/** @returns {null | Array<{label: string, detail: string}>} failures, or null on success */
function extractWithStrategies(strategies, destDir) {
  const failures = [];

  for (const strategy of strategies) {
    console.log(`   Extracting with ${strategy.label}...`);
    const outcome = runStrategy(strategy);

    if (!outcome.ok) {
      if (!outcome.skipped) {
        console.log(`   ⚠️  ${strategy.label} failed: ${outcome.detail}`);
      }
      failures.push({ label: strategy.label, detail: outcome.detail });
      continue;
    }

    const missing = missingRequiredFiles(destDir);
    if (missing.length === 0) {
      return null;
    }
    // An unpacker can exit 0 on a partial run, so the vendor dir decides success.
    const detail = `left no ${missing.join(", ")} in vendor/`;
    console.log(`   ⚠️  ${strategy.label} ${detail}`);
    failures.push({ label: strategy.label, detail });
  }

  return failures;
}

function describeExtractionFailures(failures) {
  const lines = failures.map((failure) => `${failure.label}: ${failure.detail}`);
  return `Could not unpack the release archive.\n   ${lines.join("\n   ")}`;
}

function extractArchive(archivePath, destDir) {
  const strategies = IS_WIN
    ? windowsExtractStrategies(archivePath, destDir)
    : [{ label: "tar", command: "tar", args: ["xzf", archivePath, "-C", destDir] }];

  let failures = extractWithStrategies(strategies, destDir);
  if (!failures) {
    return;
  }

  if (failures.some((failure) => isFileLockError(failure.detail))) {
    console.log("   Vendor binaries still locked; waiting for the daemon to release them...");
    if (waitForDaemonExit(LOCK_WAIT_MS)) {
      failures = extractWithStrategies(strategies, destDir);
      if (!failures) {
        return;
      }
    } else {
      failures = failures.concat({ label: "daemon wait", detail: "hintshell-core.exe is still running" });
    }
  }

  throw new Error(describeExtractionFailures(failures));
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

function stopDaemonAndReleaseLocks(reason) {
  // CRITICAL: a running hintshell-core locks vendor/*.exe on Windows
  stopRunningDaemon(reason);
  if (!waitForDaemonExit(LOCK_WAIT_MS)) {
    console.log("   ⚠️  hintshell-core.exe is still running; extraction may hit a file lock.");
  }
}

function installRemedies(detail) {
  // Match only the failure signatures themselves: the module name also appears
  // in stack paths of unrelated errors, and strategy labels always name Expand-Archive.
  const archiveModuleError =
    /CouldNotAutoloadMatchingModule|command was found in the module|could not be loaded|running scripts is disabled|execution policy/i;

  const lines = [];

  if (isFileLockError(detail)) {
    lines.push("   Tip: something still locks the binaries — hs stop (or taskkill /F /IM hintshell-core.exe), then retry.");
  }
  if (archiveModuleError.test(detail)) {
    lines.push("   Tip: this machine cannot load PowerShell's Archive module. Inspect it with:");
    lines.push("         Get-Module -ListAvailable Microsoft.PowerShell.Archive | fl Name,Version,ModuleBase");
    lines.push("         Get-ExecutionPolicy -List");
  }
  if (/Download failed|timed out/i.test(detail)) {
    lines.push("   Tip: retry, or point HINTSHELL_ASSET_URL at a mirror if GitHub is unreachable here.");
  }

  lines.push("   Then: npm i -g hintshell@latest && hintshell init");
  lines.push(`   Manual download: https://github.com/${REPO}/releases`);
  return lines;
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
  stopDaemonAndReleaseLocks("before npm extract");

  fs.mkdirSync(installDir, { recursive: true });

  try {
    await downloadFile(url, archivePath);
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
      stopDaemonAndReleaseLocks("before hintshell init");
      runInit(installDir);
    } else {
      console.log(`   HINTSHELL_SKIP_INIT=1 — skipped auto init.`);
      console.log(`   Run 'hintshell init' to configure your shell.`);
    }

    console.log(`✅ HintShell install/update finished.`);
  } catch (err) {
    console.error(`❌ Installation failed: ${err.message}`);
    for (const line of installRemedies(err.message)) {
      console.error(line);
    }
    process.exit(1);
  }
}

if (require.main === module) {
  main();
}

module.exports = {
  downloadFile,
  getDownloadUrl,
  sizeMismatchError,
  summarizeError,
  extractArchive,
  windowsExtractStrategies,
  systemTarPath,
  missingRequiredFiles,
  installRemedies,
};
