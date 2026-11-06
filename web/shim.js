// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

// The map's page without a server: loaded before app.js, it starts the
// engine in a worker (worker.js) and hands it the page's requests, so that
// app.js stays as the server serves it. fetch("/api/...") and
// EventSource("/api/events") go to the worker; anything else (map tiles) to
// the network.
//
// The options of the command line come from the page's address:
// ?area=u09t&depth=2&strategy=a1 is `u09t --depth 2 --strategy a1`.

(() => {
  const worker = new Worker("/assets/worker.js", { type: "module" });
  const query = new URLSearchParams(location.search);
  const args = [];
  for (const [key, value] of query) {
    if (key === "area") continue;
    args.push("--" + key);
    if (value !== "") args.push(value);
  }
  worker.postMessage({ type: "start", area: query.get("area"), args });

  let next = 1;
  const pending = new Map();
  const streams = new Map();

  worker.onmessage = e => {
    const m = e.data;
    if (m.type === "reply") {
      const resolve = pending.get(m.id);
      pending.delete(m.id);
      if (resolve) {
        resolve(new Response(m.body, {
          status: m.status,
          headers: { "content-type": "application/json" },
        }));
      }
    } else if (m.type === "event") {
      const stream = streams.get(m.sid);
      if (stream) stream.deliver(m);
    } else if (m.type === "failed") {
      console.error("map engine:", m.message);
    }
  };

  const ours = url => url.origin === location.origin && url.pathname.startsWith("/api/");

  const networkFetch = window.fetch.bind(window);
  window.fetch = (input, init = {}) => {
    const url = new URL(typeof input === "string" ? input : input.url, location.href);
    if (!ours(url)) return networkFetch(input, init);
    const id = next++;
    return new Promise(resolve => {
      pending.set(id, resolve);
      worker.postMessage({
        type: "request",
        id,
        method: (init.method || "GET").toUpperCase(),
        path: url.pathname + url.search,
        body: typeof init.body === "string" ? init.body : "",
      });
    });
  };

  const CONNECTING = 0, OPEN = 1, CLOSED = 2;

  // The page's event stream, from the worker: hello, then the deltas. When
  // the engine closes it, an error as EventSource gives, readyState CLOSED:
  // the page opens another one.
  class EngineEvents extends EventTarget {
    constructor(url) {
      super();
      this.url = url;
      this.readyState = CONNECTING;
      this.onopen = this.onmessage = this.onerror = null;
      this.sid = next++;
      streams.set(this.sid, this);
      worker.postMessage({ type: "subscribe", sid: this.sid });
    }

    deliver(m) {
      if (this.readyState === CLOSED) return;
      if (m.event === "end") {
        this.forget();
        const error = new Event("error");
        this.dispatchEvent(error);
        if (this.onerror) this.onerror(error);
        return;
      }
      if (this.readyState === CONNECTING) {
        this.readyState = OPEN;
        const open = new Event("open");
        this.dispatchEvent(open);
        if (this.onopen) this.onopen(open);
      }
      const event = new MessageEvent(m.event, { data: m.data, lastEventId: String(m.id) });
      this.dispatchEvent(event);
      if (m.event === "message" && this.onmessage) this.onmessage(event);
    }

    forget() {
      this.readyState = CLOSED;
      streams.delete(this.sid);
    }

    close() {
      if (this.readyState === CLOSED) return;
      this.forget();
      worker.postMessage({ type: "unsubscribe", sid: this.sid });
    }
  }

  const NetworkEvents = window.EventSource;
  function EventSource(url, options) {
    const u = new URL(url, location.href);
    if (ours(u) && u.pathname === "/api/events") return new EngineEvents(url);
    return new NetworkEvents(url, options);
  }
  Object.assign(EventSource, { CONNECTING, OPEN, CLOSED });
  Object.assign(EngineEvents, { CONNECTING, OPEN, CLOSED });
  window.EventSource = EventSource;
})();
