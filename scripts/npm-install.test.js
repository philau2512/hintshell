const assert = require("node:assert/strict");
const fs = require("node:fs");
const http = require("node:http");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { execFileSync, spawnSync } = require("node:child_process");

const {
  downloadFile,
  getDownloadUrl,
  extractArchive,
  installRemedies,
  missingRequiredFiles,
  sizeMismatchError,
  summarizeError,
  systemTarPath,
  windowsExtractStrategies,
} = require("./npm-install");

const IS_WIN = process.platform === "win32";
// The packer needs bsdtar (GNU tar cannot write zip), which Windows ships in System32.
const PACK_TAR = systemTarPath() || "tar";

function tempFile(name) {
  return path.join(os.tmpdir(), `hintshell-${process.pid}-${Date.now()}-${name}`);
}

function tempDir(prefix) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `hintshell-${prefix}-${process.pid}-`));
}

function withPsModulePath(moduleDir, body) {
  const previous = process.env.PSModulePath;
  process.env.PSModulePath = `${moduleDir}${path.delimiter}${previous || ""}`;
  try {
    return body();
  } finally {
    if (previous === undefined) {
      delete process.env.PSModulePath;
    } else {
      process.env.PSModulePath = previous;
    }
  }
}

// Releases pack dist/* so entries sit at the archive root and only module/ nests.
function createReleaseArchive(dir, { binaries = true } = {}) {
  const staging = path.join(dir, "dist");
  const entries = { "default-commands.json": "[]" };
  if (binaries) {
    entries[`hintshell${IS_WIN ? ".exe" : ""}`] = "cli";
    entries[`hintshell-core${IS_WIN ? ".exe" : ""}`] = "core";
  }
  entries[path.join("module", "HintShellModule.psm1")] = "module";

  for (const [name, content] of Object.entries(entries)) {
    const target = path.join(staging, name);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, content);
  }

  const archivePath = path.join(dir, "release.zip");
  const names = Object.keys(entries).map((name) => name.split(path.sep).join("/"));
  execFileSync(PACK_TAR, ["-a", "-c", "-f", archivePath, "-C", staging, ...names], {
    stdio: "ignore",
  });
  return archivePath;
}

// A module copy whose manifest advertises Expand-Archive but whose body cannot load:
// the exact machine state that used to fail the whole install.
function installBrokenArchiveModule(dir) {
  const moduleDir = path.join(dir, "Microsoft.PowerShell.Archive");
  fs.mkdirSync(moduleDir, { recursive: true });
  fs.writeFileSync(
    path.join(moduleDir, "Microsoft.PowerShell.Archive.psd1"),
    [
      "@{",
      "ModuleVersion = '1.0.1'",
      "RootModule = 'Microsoft.PowerShell.Archive.psm1'",
      "FunctionsToExport = @('Expand-Archive')",
      "}",
    ].join("\n")
  );
  fs.writeFileSync(path.join(moduleDir, "Microsoft.PowerShell.Archive.psm1"), "throw 'broken by test'\n");
  return moduleDir;
}

function listen(handler) {
  const server = http.createServer(handler);
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => resolve(server));
  });
}

test("Windows stop script stops only live wrapper processes", () => {
  const script = fs.readFileSync(path.join(__dirname, "npm-stop.js"), "utf8");
  assert.match(script, /hintshell-core\.exe/);
  assert.match(script, /Name = 'hintshell\.exe'/);
  assert.match(script, /CommandLine -match/);
  assert.match(script, /CommandLine -match .*bash/);
  assert.doesNotMatch(script, /taskkill", \["\/F", "\/IM", "hintshell\.exe"\]/);
  assert.doesNotMatch(script, /Get-Process -Name 'hintshell', 'hintshell-core'/);
});

test("PowerShell update checks npm version before lifecycle work and retains module helpers", () => {
  const module = fs.readFileSync(
    path.join(__dirname, "..", "integrations", "powershell", "HintShellModule", "HintShellModule.psm1"),
    "utf8"
  );
  assert.match(module, /function script:Get-HSInstalledVersion/);
  assert.match(module, /function script:Get-HSLatestNpmVersion/);
  assert.match(module, /npm view hintshell@latest version --fetch-timeout=30000 --fetch-retries=1/);
  assert.match(module, /already the latest version/);
  assert.match(module, /if \(\$installedVersion -and \$latestVersion -and \$installedVersion -eq \$latestVersion\)[\s\S]*?return[\s\S]*?Stopping daemon/);
  assert.doesNotMatch(module, /Remove-Module HintShellModule -Force/);
  assert.doesNotMatch(module, /Reloading module \+ starting daemon/);
});

test("isolated tests skip global process control", () => {
  const script = fs.readFileSync(path.join(__dirname, "npm-stop.js"), "utf8");
  assert.match(script, /HINTSHELL_SKIP_PROCESS_CONTROL/);
});

test("uses the local asset URL override for isolated installer tests", () => {
  const previous = process.env.HINTSHELL_ASSET_URL;
  process.env.HINTSHELL_ASSET_URL = "http://127.0.0.1:8123/hintshell-{target}{ext}";
  try {
    assert.equal(
      getDownloadUrl("x86_64-pc-windows-msvc", ".zip"),
      "http://127.0.0.1:8123/hintshell-x86_64-pc-windows-msvc.zip"
    );
  } finally {
    if (previous === undefined) {
      delete process.env.HINTSHELL_ASSET_URL;
    } else {
      process.env.HINTSHELL_ASSET_URL = previous;
    }
  }
});

test("downloads a release asset after a redirect", async () => {
  const server = await listen((request, response) => {
    if (request.url === "/release") {
      response.writeHead(302, { location: "/asset" });
      response.end();
      return;
    }
    response.writeHead(200, { "content-length": "5" });
    response.end("ready");
  });
  const destination = tempFile("redirect.zip");

  try {
    const address = server.address();
    await downloadFile(`http://127.0.0.1:${address.port}/release`, destination, {
      requestTimeoutMs: 1_000,
    });
    assert.equal(fs.readFileSync(destination, "utf8"), "ready");
  } finally {
    server.close();
    fs.rmSync(destination, { force: true });
  }
});

test("copies an isolated local file asset", async () => {
  const source = tempFile("fixture.zip");
  const destination = tempFile("local-copy.zip");
  fs.writeFileSync(source, "fixture");

  try {
    await downloadFile(new URL(`file:///${source.replace(/\\/g, "/")}`).toString(), destination);
    assert.equal(fs.readFileSync(destination, "utf8"), "fixture");
  } finally {
    fs.rmSync(source, { force: true });
    fs.rmSync(destination, { force: true });
  }
});

test("rejects a stalled download and removes its partial archive", async () => {
  const server = await listen(() => {});
  const destination = tempFile("timeout.zip");

  try {
    const address = server.address();
    await assert.rejects(
      downloadFile(`http://127.0.0.1:${address.port}/stalled`, destination, {
        requestTimeoutMs: 50,
      }),
      /timed out/
    );
    assert.equal(fs.existsSync(destination), false);
  } finally {
    server.close();
    fs.rmSync(destination, { force: true });
  }
});

test("rejects a truncated body before the unpacker ever sees it", async () => {
  const server = await listen((request, response) => {
    response.writeHead(200, { "content-length": "20" });
    response.flushHeaders();
    response.socket.write("short");
    response.socket.end();
  });
  const destination = tempFile("truncated.zip");

  try {
    const address = server.address();
    // Either the HTTP layer (premature close) or the byte-count backstop below
    // it has to refuse the asset, and neither may leave a partial archive.
    await assert.rejects(
      downloadFile(`http://127.0.0.1:${address.port}/truncated`, destination, {
        requestTimeoutMs: 1_000,
      }),
      /incomplete|premature|aborted/i
    );
    assert.equal(fs.existsSync(destination), false);
  } finally {
    server.close();
    fs.rmSync(destination, { force: true });
  }
});

test("compares the byte count with content-length when both are known", () => {
  assert.equal(sizeMismatchError(100, 100), null);
  assert.equal(sizeMismatchError(0, 5_000_000), null);
  assert.match(
    sizeMismatchError(100, 60).message,
    /incomplete release asset \(60 of 100 bytes\)/
  );
});

test("Windows extraction does not depend on PowerShell's Archive module", () => {
  const installer = fs.readFileSync(path.join(__dirname, "npm-install.js"), "utf8");
  // The single-shot call that failed on machines where the module cannot load.
  assert.doesNotMatch(installer, /execFileSync\("powershell", \["-NoProfile", "-Command"/);
  assert.match(installer, /waitForDaemonExit/);
});

test("keeps one unpacker per machine weakness", { skip: !IS_WIN && "Windows-only extractor chain" }, () => {
  const strategies = windowsExtractStrategies("release.zip", "vendor");

  assert.ok(strategies.length >= 3, `expected several extractors, got ${strategies.length}`);
  assert.match(strategies[0].command, /[\\/]System32[\\/]tar\.exe$/i);

  // Git Bash puts GNU tar first on PATH and it cannot read zip, so PATH must not decide.
  assert.ok(!strategies.some((strategy) => strategy.command === "tar"));

  // At least one path needs no PowerShell module at all.
  const scripts = strategies.map((strategy) => strategy.args[strategy.args.length - 1]);
  assert.ok(scripts.some((script) => /ZipFile/.test(script) && !/Expand-Archive/.test(script)));

  for (const strategy of strategies.filter((s) => /powershell|pwsh/i.test(s.command))) {
    assert.ok(strategy.args.includes("-NoProfile"), `${strategy.label} must not load profiles`);
    assert.ok(strategy.args.includes("-ExecutionPolicy"), `${strategy.label} must not inherit policy`);
  }
});

test("installs over a broken Microsoft.PowerShell.Archive module", { skip: !IS_WIN && "Windows-only extractor chain" }, () => {
  const dir = tempDir("broken-module");
  const archivePath = createReleaseArchive(dir);
  const vendorDir = path.join(dir, "vendor");
  fs.mkdirSync(vendorDir);

  try {
    const moduleDir = installBrokenArchiveModule(dir);

    withPsModulePath(moduleDir, () => {
      // Proof the fixture really breaks the cmdlet the old installer relied on.
      const legacy = windowsExtractStrategies(archivePath, vendorDir).at(-1);
      assert.match(legacy.label, /Windows PowerShell Expand-Archive/);
      const result = spawnSync(legacy.command, legacy.args, { encoding: "utf8" });
      assert.notEqual(result.status, 0, "Expand-Archive was expected to fail on this fixture");
      assert.match(`${result.stderr}`, /Microsoft\.PowerShell\.Archive/);

      // The shipped chain must still place the binaries.
      extractArchive(archivePath, vendorDir);
      assert.deepEqual(missingRequiredFiles(vendorDir), []);
      assert.equal(fs.existsSync(path.join(vendorDir, "module", "HintShellModule.psm1")), true);
    });
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("fails when an unpacker exits 0 but leaves no binaries", { skip: !IS_WIN && "Windows-only extractor chain" }, () => {
  const dir = tempDir("partial");
  const archivePath = createReleaseArchive(dir, { binaries: false });
  const vendorDir = path.join(dir, "vendor");
  fs.mkdirSync(vendorDir);

  try {
    assert.throws(
      () => extractArchive(archivePath, vendorDir),
      (error) => {
        assert.match(error.message, /Could not unpack the release archive/);
        assert.match(error.message, /tar\.exe/);
        assert.match(error.message, /left no .* in vendor\//);
        return true;
      }
    );
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("routes each failure to the remedy that fits", () => {
  const moduleRemedy = installRemedies(
    "Windows PowerShell Expand-Archive: Expand-Archive : The 'Expand-Archive' command was found in " +
      "the module 'Microsoft.PowerShell.Archive', but the module could not be loaded. FullyQualifiedErrorId : CouldNotAutoloadMatchingModule"
  ).join("\n");
  assert.match(moduleRemedy, /Archive module/);
  assert.match(moduleRemedy, /Get-ExecutionPolicy/);
  assert.doesNotMatch(moduleRemedy, /taskkill/, "a module failure must not chase the daemon");

  const lockRemedy = installRemedies(
    "System32 tar.exe: vendor\\hintshell-core.exe: The process cannot access the file because it is being used by another process"
  ).join("\n");
  assert.match(lockRemedy, /taskkill/);
  assert.doesNotMatch(lockRemedy, /Archive module/);

  const networkRemedy = installRemedies("Download failed: incomplete release asset (60 of 100 bytes)").join("\n");
  assert.match(networkRemedy, /HINTSHELL_ASSET_URL/);
  assert.match(networkRemedy, /Manual download/);
});

test("keeps the error signature and drops the source echoes", () => {
  const autoload = [
    "Expand-Archive : The 'Expand-Archive' command was found in the module 'Microsoft.PowerShell.Archive', but the module",
    "could not be loaded. For more information, run 'Import-Module Microsoft.PowerShell.Archive'.",
    "At line:1 char:34",
    "+ $ErrorActionPreference = 'Stop'; Expand-Archive -LiteralPath 'C:\\User ...",
    "+                                  ~~~~~~~~~~~~~~",
    "    + FullyQualifiedErrorId : CouldNotAutoloadMatchingModule",
  ].join("\r\n");

  const summary = summarizeError(autoload, "exit code 1");
  assert.match(summary, /CouldNotAutoloadMatchingModule/);
  assert.match(summary, /could not be loaded/);
  assert.doesNotMatch(summary, /~{5}/);
  assert.ok(summary.length <= 400, summary.length);

  // PowerShell 7 colourises the record even when its stderr is redirected.
  const coloured = summarizeError("\x1b[31;1mOperationStopped: \x1b[31;1m\x1b[0m", "exit code 1");
  assert.equal(coloured, "OperationStopped:");
});

test("does not blame the Archive module for a corrupt asset", () => {
  const corrupt = summarizeError(
    [
      'New-Object : Exception calling ".ctor" with "3" argument(s): "End of Central Directory record could not be found."',
      "At C:\\Windows\\system32\\WindowsPowerShell\\v1.0\\Modules\\Microsoft.PowerShell.Archive\\Microsoft.PowerShell.Archive.psm1:934 char:23",
      "+ ... ipArchive = New-Object -TypeName System.IO.Compression.ZipArchive -Ar ...",
      "+                 ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~",
      "    + CategoryInfo          : NotSpecified: (:) [], ParentContainsErrorRecordException",
      "    + FullyQualifiedErrorId : InvalidDataException",
    ].join("\r\n"),
    "exit code 1"
  );

  assert.doesNotMatch(corrupt, /Microsoft\.PowerShell\.Archive/);
  const remedy = installRemedies(`Windows PowerShell Expand-Archive: ${corrupt}`).join("\n");
  assert.doesNotMatch(remedy, /Archive module/);
  assert.doesNotMatch(remedy, /taskkill/);
  assert.match(remedy, /Manual download/);
});
