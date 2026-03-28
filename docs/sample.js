// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

const {
  createMessageConnection,
  StreamMessageReader,
  StreamMessageWriter,
} = require("vscode-jsonrpc/node");
const path = require("path");
const { spawn } = require("child_process");
const { PassThrough } = require("stream");

const RET_EXE = path.join(
  __dirname,
  "..",
  "target",
  "debug",
  process.platform === "win32" ? "ret.exe" : "ret"
);

const installations = [];
const managers = [];

async function start() {
  const readable = new PassThrough();
  const writable = new PassThrough();
  const proc = spawn(RET_EXE, ["server"], {
    env: process.env,
  });

  proc.stdout.pipe(readable, { end: false });
  proc.stderr.on("data", (data) => console.error(data.toString()));
  writable.pipe(proc.stdin, { end: false });

  const connection = createMessageConnection(
    new StreamMessageReader(readable),
    new StreamMessageWriter(writable)
  );
  connection.onError((ex) => console.error("Connection Error:", ex));
  connection.onClose(() => proc.kill());
  handleNotifications(connection);
  connection.listen();
  return connection;
}

/**
 * @param {import("vscode-jsonrpc").MessageConnection} connection
 */
function handleNotifications(connection) {
  connection.onNotification("manager", (manager) => {
    managers.push(manager);
    console.log(`Discovered Manager (${manager.tool}) ${manager.executable}`);
  });

  connection.onNotification("installation", (installation) => {
    installations.push(installation);
  });

  connection.onNotification("telemetry", (payload) => {
    console.log(`Telemetry: ${payload.event}`);
  });
}

/**
 * @param {import("vscode-jsonrpc").MessageConnection} connection
 */
async function configure(connection) {
  const cacheDirectory = path.join(process.cwd(), "temp", "ret-cache");

  await connection.sendRequest("configure", {
    workspaceDirectories: [process.cwd()],
    environmentDirectories: [],
    cacheDirectory,
  });
}

/**
 * @param {import("vscode-jsonrpc").MessageConnection} connection
 * @param {undefined | { searchKind?: string } | { searchPaths?: string[] }} search
 */
async function refresh(connection, search) {
  installations.length = 0;
  managers.length = 0;

  const { duration } = await connection.sendRequest("refresh", search);
  const count = installations.length;
  const scope = search
    ? ` (in ${JSON.stringify(search)})`
    : " (using configured search roots)";

  console.log(`Found ${count} installations in ${duration}ms${scope}`);
}

/**
 * @param {import("vscode-jsonrpc").MessageConnection} connection
 * @param {string} searchPath
 */
async function find(connection, searchPath) {
  const result = await connection.sendRequest("find", { searchPath });
  const count = Array.isArray(result) ? result.length : 0;

  console.log(`Find returned ${count} installation(s)`);
  return result;
}

/**
 * @param {import("vscode-jsonrpc").MessageConnection} connection
 * @param {string} executable
 */
async function resolve(connection, executable) {
  try {
    const result = await connection.sendRequest("resolve", { executable });
    console.log(
      `Resolved (${result.kind}, ${result.version}) ${result.executable}`
    );
    return result;
  } catch (ex) {
    console.error(`Failed to resolve ${executable}`, ex.message);
  }
}

/**
 * @param {import("vscode-jsonrpc").MessageConnection} connection
 */
async function condaInfo(connection) {
  const info = await connection.sendRequest("condaInfo", null);
  console.log(`Conda executable available: ${info.canSpawnConda}`);
  return info;
}

/**
 * @param {import("vscode-jsonrpc").MessageConnection} connection
 */
async function clearCache(connection) {
  await connection.sendRequest("clearCache", null);
}

async function main() {
  const connection = await start();

  await configure(connection);

  await refresh(connection);

  // Search explicit paths for this refresh only.
  await refresh(connection, {
    searchPaths: [
      "/Users/user_name/projects/demo",
      "/Users/user_name/.rig/R/4.4.1",
      "/usr/local/bin/R",
    ],
  });

  // Limit refresh to a specific installation kind.
  await refresh(connection, { searchKind: "Rig" });

  // Search a single path directly.
  await find(connection, process.cwd());

  // Resolve either an executable or an installation home directory.
  await resolve(connection, "/usr/local/bin/R");
  await resolve(connection, "/Library/Frameworks/R.framework/Resources");

  await condaInfo(connection);
  await clearCache(connection);

  connection.end();
  process.exit(0);
}

main();
