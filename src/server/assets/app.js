// Map page (lot 14): one rectangle per channel, coloured by activity,
// updated in place from the server's SSE stream (`hello`, then deltas).
// Strings from relays or relay lists only ever go through `textContent`.

import {
  ago, fillColour, isActive, levelCounts, levelLabel, maxValue, rank,
  rejectedTotal, scaleValue, PALETTE,
} from "./view.js";

const css = getComputedStyle(document.documentElement);
export const colour = s => css.getPropertyValue("--" + s).trim() || "#7a7a7a";

// Strings from relays and relay lists are text only, never HTML.
export function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

// Relays sharing coordinates (common in the lists) are drawn on a ring of
// fixed pixel radius around their point, so all stay visible at any zoom.
// Returns the icon and popup anchors of the `index`-th of `count` pins.
export function ringAnchors(index, count, size) {
  const a = 2 * Math.PI * index / count - Math.PI / 2;
  const radius = count > 1 ? 10 + 4 * count : 0;
  return {
    iconAnchor: [size / 2 - radius * Math.cos(a), size / 2 - radius * Math.sin(a)],
    popupAnchor: [radius * Math.cos(a), radius * Math.sin(a) - size / 2],
  };
}

// --- State -----------------------------------------------------------------

const state = {
  params: null,
  seq: null,
  cells: new Map(),   // geohash -> cell view
  relays: [],
  status: null,
  level: null,        // level shown
  scale: "participants",
  max: 1,             // scale maximum at the level shown
  link: "connecting",
};

// --- Map -------------------------------------------------------------------

const map = L.map("map", { zoomSnap: 0.5 }).setView([20, 0], 2);
const renderer = L.svg({ padding: 0.5 });
const layers = new Map();   // geohash -> L.Rectangle
const groups = new Map();   // level -> L.LayerGroup
let tiles = null;

// Hatching for "low" cells (k threshold), referenced as url(#low-hatch).
(function hatch() {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("width", "0");
  svg.setAttribute("height", "0");
  svg.setAttribute("aria-hidden", "true");
  svg.classList.add("defs");
  const pattern = document.createElementNS(ns, "pattern");
  pattern.id = "low-hatch";
  pattern.setAttribute("patternUnits", "userSpaceOnUse");
  pattern.setAttribute("width", "6");
  pattern.setAttribute("height", "6");
  pattern.setAttribute("patternTransform", "rotate(45)");
  const line = document.createElementNS(ns, "line");
  line.setAttribute("x1", "0");
  line.setAttribute("y1", "0");
  line.setAttribute("x2", "0");
  line.setAttribute("y2", "6");
  line.setAttribute("stroke", PALETTE[2]);
  line.setAttribute("stroke-width", "2");
  pattern.append(line);
  const defs = document.createElementNS(ns, "defs");
  defs.append(pattern);
  svg.append(defs);
  document.body.append(svg);
})();

function styleFor(cell) {
  if (!cell.watched) {
    return { color: colour("muted"), weight: 1, dashArray: "4 4", fill: false, opacity: 0.8 };
  }
  const value = scaleValue(cell, state.scale);
  if (value === "low") {
    return { color: PALETTE[2], weight: 1, dashArray: null, fill: true,
             fillColor: "url(#low-hatch)", fillOpacity: 0.8, opacity: 0.8 };
  }
  const fill = fillColour(value, state.max);
  if (!fill) {
    return { color: colour("line"), weight: 1, dashArray: null, fill: false, opacity: 0.45 };
  }
  return { color: fill, weight: 1, dashArray: null, fill: true, fillColor: fill,
           fillOpacity: 0.55, opacity: 0.9 };
}

function group(level) {
  let g = groups.get(level);
  if (!g) {
    g = L.layerGroup();
    groups.set(level, g);
    if (level === state.level) g.addTo(map);
  }
  return g;
}

function upsert(cell) {
  state.cells.set(cell.geohash, cell);
  let layer = layers.get(cell.geohash);
  if (!layer) {
    layer = L.rectangle(cell.bbox, { renderer, ...styleFor(cell) });
    layer.bindTooltip(() => tooltip(state.cells.get(cell.geohash)), {
      // "auto": left or right of the pointer, whichever side has room.
      sticky: true, direction: "auto", offset: [12, 0], className: "cell-tip",
    });
    layers.set(cell.geohash, layer);
    group(cell.level).addLayer(layer);
  } else {
    layer.setStyle(styleFor(cell));
    if (layer.isTooltipOpen()) layer.setTooltipContent(tooltip(cell));
  }
}

function remove(geohash) {
  const cell = state.cells.get(geohash);
  const layer = layers.get(geohash);
  if (cell && layer) group(cell.level).removeLayer(layer);
  layers.delete(geohash);
  state.cells.delete(geohash);
}

// Recolours the level shown when its maximum changed (the scale moved).
function rescale() {
  const shown = [...state.cells.values()].filter(c => c.level === state.level);
  const max = maxValue(shown, state.scale);
  if (max !== state.max) {
    state.max = max;
    for (const c of shown) layers.get(c.geohash)?.setStyle(styleFor(c));
  }
}

function showLevel(level) {
  if (state.level !== null) groups.get(state.level)?.remove();
  state.level = level;
  state.max = -1; // forces a recolour
  group(level).addTo(map);
  rescale();
  updatePanel();
}

// --- Tooltip ---------------------------------------------------------------

function tooltip(cell) {
  const box = el("div", "tip");
  const head = el("div", "tip-head");
  head.append(el("b", null, "#" + cell.geohash), el("span", "muted",
    ` · ${levelLabel(cell.level)}${cell.watched ? "" : " · retired"}`));
  box.append(head);
  const counts = el("div", "tip-counts");
  for (const [label, value] of [["messages", cell.messages], ["presences", cell.presences],
                                ["participants", cell.participants]]) {
    const item = el("span", "count");
    item.append(el("b", null, String(value)), el("span", "muted", " " + label));
    counts.append(item);
  }
  box.append(counts);
  const now = Date.now() / 1000;
  box.append(el("div", "muted",
    cell.last_seen ? "last event " + ago(now - cell.last_seen) : "no event yet"));

  const table = el("table", "relays");
  const tr = el("tr");
  for (const h of ["relay", "iOS", "Android", "status", "delivered"]) tr.append(el("th", null, h));
  table.append(tr);
  for (const r of cell.relays) {
    const row = el("tr");
    const status = r.status || "unprobed";
    const st = el("td");
    const dot = el("span", "dot");
    dot.style.background = colour(status);
    st.append(dot, document.createTextNode(status));
    row.append(el("td", "url", r.url.replace(/^wss?:\/\//, "")), el("td", null, rank(r.ios)),
               el("td", null, rank(r.android)), st, el("td", null, r.delivered ? "✓" : ""));
    table.append(row);
  }
  box.append(table);
  if (cell.divergence.length) {
    box.append(el("div", "warn",
      `${cell.divergence.length} of ${cell.relays.length} relays chosen by one app only: ` +
      "iOS and Android users may not see each other here"));
  }
  return box;
}

// --- Panel -----------------------------------------------------------------

// Built once; `updatePanel` only changes texts and options, so that an
// open drop-down list stays open across updates.
const panel = document.getElementById("panel");
const ui = {};

function buildPanel() {
  const head = el("div", "head");
  const toggle = el("button", "toggle", "▾");
  toggle.title = "fold the panel";
  toggle.setAttribute("aria-label", "fold the panel");
  toggle.addEventListener("click", () => {
    const folded = panel.classList.toggle("folded");
    toggle.textContent = folded ? "▸" : "▾";
  });
  head.append(el("h1", null, "BitChat geohash activity"), toggle);
  // On a phone the map comes first: the panel starts folded.
  if (window.innerWidth < 600) {
    panel.classList.add("folded");
    toggle.textContent = "▸";
  }

  ui.link = el("div", "link");
  ui.linkText = el("span");
  ui.start = el("span", "muted");
  ui.link.append(el("span", "dot"), ui.linkText, ui.start);
  ui.tabs = el("div", "warn",
    "too many tabs open on this server: close one, or enable TLS (HTTP/2)");

  const body = el("div", "body");
  ui.stats = el("div", "stats");
  ui.level = el("select");
  ui.level.setAttribute("aria-label", "level shown");
  ui.level.addEventListener("change", () => showLevel(Number(ui.level.value)));
  ui.scale = el("select");
  ui.scale.setAttribute("aria-label", "colour by");
  for (const s of ["participants", "messages"]) {
    const o = el("option", null, "colour by " + s);
    o.value = s;
    ui.scale.append(o);
  }
  ui.scale.addEventListener("change", () => {
    state.scale = ui.scale.value;
    state.max = -1;
    rescale();
    updatePanel();
  });
  const controls = el("div", "controls");
  controls.append(ui.level, ui.scale);

  const legend = el("div", "legend");
  const bar = el("div", "bar");
  bar.style.background = `linear-gradient(to right, ${PALETTE.join(", ")})`;
  ui.max = el("span");
  const ends = el("div", "ends");
  ends.append(el("span", null, "1"), ui.max);
  ui.low = el("span", "key low");
  const keys = el("div", "keys muted");
  keys.append(el("span", "key none", "no activity"), el("span", "key retired", "retired"), ui.low);
  legend.append(bar, ends, keys);

  body.append(ui.stats, controls, legend, el("div", "note muted",
    "Presences are not comparable between levels: the apps send heartbeats at levels 2, 4 and 5 only."));
  panel.append(head, ui.link, ui.tabs, body);
}

function updatePanel() {
  ui.link.className = "link " + state.link;
  ui.linkText.textContent = {
    live: "live", connecting: "connecting…", reconnecting: "reconnecting…", tabs: "no live stream",
  }[state.link];
  ui.start.textContent = state.params ? ` · #${state.params.start}` : "";
  ui.tabs.hidden = state.link !== "tabs";

  if (state.status) {
    const s = state.status;
    ui.stats.replaceChildren(
      el("div", null, `channels: ${s.watched} watched, ${s.retired} retired`),
      el("div", null, `relays: ${s.relays} listened, ${s.dropped_relays.length} set aside`),
      el("div", null, `connections: ${s.connections_up} / ${s.connections} open`),
      el("div", null, `events not counted: ${rejectedTotal(s)}`),
    );
  }

  const counts = levelCounts(state.cells.values());
  const options = [...ui.level.options];
  if (options.map(o => Number(o.value)).join() !== counts.map(([l]) => l).join()) {
    ui.level.replaceChildren(...counts.map(([level]) => {
      const o = el("option");
      o.value = String(level);
      return o;
    }));
  }
  counts.forEach(([level, active, all], i) => {
    ui.level.options[i].textContent = `${levelLabel(level)}: ${active} active / ${all}`;
  });
  if (state.level !== null) ui.level.value = String(state.level);
  ui.scale.value = state.scale;

  ui.max.textContent = `${Math.max(state.max, 1)} ${state.scale} (log scale)`;
  const k = state.params ? state.params.k : 0;
  ui.low.hidden = !(k > 0);
  ui.low.textContent = `fewer than ${k} participants`;
}

// --- Live stream -------------------------------------------------------------

let source = null;
let helloTimer = null;
let retryTimer = null;

function setLink(link) {
  state.link = link;
  updatePanel();
}

function onHello(d) {
  state.params = d.params;
  if (!tiles) {
    tiles = L.tileLayer(d.params.tiles, { maxZoom: 19, attribution: d.params.attribution }).addTo(map);
  }
  const seen = new Set(d.cells.map(c => c.geohash));
  for (const g of [...state.cells.keys()]) if (!seen.has(g)) remove(g);
  for (const c of d.cells) upsert(c);
  state.relays = d.relays;
  state.status = d.status;
  state.seq = d.seq;
  const levels = levelCounts(d.cells).map(([l]) => l);
  if (state.level === null || !levels.includes(state.level)) {
    if (levels.length) {
      showLevel(levels[0]); // the finest
      const bounds = L.latLngBounds(d.cells.map(c => c.bbox));
      if (bounds.isValid()) map.fitBounds(bounds, { padding: [20, 20] });
    }
  }
  rescale();
  setLink("live");
  updatePanel();
}

// Applies a delta, or starts over on a gap in the sequence.
function delta(e, apply) {
  const d = JSON.parse(e.data);
  if (state.seq === null || d.seq !== state.seq + 1) {
    connect();
    return;
  }
  state.seq = d.seq;
  apply(d);
}

function connect() {
  if (source) source.close();
  clearTimeout(helloTimer);
  clearTimeout(retryTimer);
  state.seq = null;
  source = new EventSource("/api/events");
  // In HTTP/1.1 a browser opens at most 6 connections per host: past that,
  // the stream waits forever.
  helloTimer = setTimeout(() => setLink("tabs"), 5000);
  source.addEventListener("hello", e => {
    clearTimeout(helloTimer);
    onHello(JSON.parse(e.data));
  });
  source.addEventListener("cells", e => delta(e, d => {
    for (const c of d.cells) upsert(c);
    rescale();
    updatePanel();
  }));
  source.addEventListener("removed", e => delta(e, d => {
    for (const g of d.cells) remove(g);
    updatePanel();
  }));
  source.addEventListener("relays", e => delta(e, d => { state.relays = d.relays; }));
  source.addEventListener("status", e => delta(e, d => {
    state.status = d.status;
    updatePanel();
  }));
  source.onerror = () => {
    if (source.readyState === EventSource.CLOSED) {
      // Refused (too many streams) or server gone: try again later.
      clearTimeout(helloTimer);
      setLink(state.params ? "reconnecting" : "tabs");
      retryTimer = setTimeout(connect, 5000);
    } else {
      setLink("reconnecting");
    }
  };
}

buildPanel();
updatePanel();
connect();
