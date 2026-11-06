// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

// The map's engine in the browser: the monitor (Rust, WebAssembly) runs in
// this worker, answers the page's /api/* requests and streams its events
// (see shim.js). Its state is kept in the browser's database (IndexedDB),
// as the binary keeps it in SQLite.

import init, { Engine } from "./pkg/bitchat_geohash.js";

const DB = "bitchat-geohash";
const TABLE = "state";
const DEFAULT_AREA = "u09t";

function database() {
  return new Promise((resolve, reject) => {
    const open = indexedDB.open(DB, 1);
    open.onupgradeneeded = () => open.result.createObjectStore(TABLE);
    open.onsuccess = () => resolve(open.result);
    open.onerror = () => reject(open.error);
  });
}

const db = database().catch(e => {
  console.warn("no browser database: nothing is kept", e);
  return null;
});

async function load(key) {
  const d = await db;
  if (!d) return undefined;
  return new Promise(resolve => {
    const get = d.transaction(TABLE).objectStore(TABLE).get(key);
    get.onsuccess = () => resolve(get.result);
    get.onerror = () => resolve(undefined);
  });
}

// Writes are queued in order by the database itself.
async function keep(key, value) {
  const d = await db;
  if (!d) return;
  d.transaction(TABLE, "readwrite").objectStore(TABLE).put(value, key);
}

let failed = null;
let ready = null;
const subscriptions = new Map();

async function start(area, args) {
  await init();
  area = area || (await load("area")) || DEFAULT_AREA;
  const snapshot = await load("snapshot");
  const engine = await Engine.start([area, ...args], snapshot, s => keep("snapshot", s));
  keep("area", engine.area());
  return engine;
}

function reply(id, status, body) {
  postMessage({ type: "reply", id, status, body });
}

onmessage = async e => {
  const m = e.data;
  if (m.type === "start") {
    ready = start(m.area, m.args).catch(err => {
      failed = String(err && err.message ? err.message : err);
      console.error("the map cannot start:", failed);
      postMessage({ type: "failed", message: failed });
      return null;
    });
    return;
  }
  const engine = await ready;
  if (m.type === "request") {
    if (!engine) {
      reply(m.id, 503, JSON.stringify({ error: "monitor_stopped", message: failed }));
      return;
    }
    const [status, body] = await engine.handle(m.method, m.path, m.body);
    reply(m.id, status, body);
    if (m.path === "/api/area" && status === 200) keep("area", engine.area());
  } else if (m.type === "subscribe") {
    if (!engine) {
      postMessage({ type: "event", sid: m.sid, event: "end", id: 0, data: "" });
      return;
    }
    subscriptions.set(m.sid, engine.subscribe((event, id, data) => {
      if (event === "end") subscriptions.delete(m.sid);
      postMessage({ type: "event", sid: m.sid, event, id, data });
    }));
  } else if (m.type === "unsubscribe") {
    const s = subscriptions.get(m.sid);
    if (s) {
      s.close();
      subscriptions.delete(m.sid);
    }
  }
};
