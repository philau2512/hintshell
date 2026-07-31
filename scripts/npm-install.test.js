const assert = require("node:assert/strict");
const fs = require("node:fs");
const http = require("node:http");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const { downloadFile, getDownloadUrl } = require("./npm-install");

function tempFile(name) {
  return path.join(os.tmpdir(), `hintshell-${process.pid}-${Date.now()}-${name}`);
}

function listen(handler) {
  const server = http.createServer(handler);
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => resolve(server));
  });
}

test("Windows stop script targets the live wrapper and daemon", () => {
  const script = fs.readFileSync(path.join(__dirname, "npm-stop.js"), "utf8");
  assert.match(script, /hintshell-core\.exe/);
  assert.match(script, /hintshell\.exe/);
  assert.match(script, /Get-Process -Name 'hintshell', 'hintshell-core'/);
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