// Map page (lot 14): one rectangle per channel, coloured by activity,
// updated in place from the server's SSE stream (`hello`, then deltas).
// Lot 15: click to descend or reinstate, breadcrumb, view kept in the URL.
// D21: hovering a cell previews the level below (listened to for a while).
// Lot 16: relay layer (pins, lines from the hovered cell) and mesh layer
// (activity of the channels inside each mesh).
// Strings from relays or relay lists only ever go through `textContent`.

import {
  ago, ancestors, appsLabel, belowSummary, cellsMeeting, childLevel, childrenOf, fillColour,
  formatHash, levelCounts, levelLabel, maxValue, parseHash, rank, refusalText, rejectedTotal,
  ringAnchors, ringed, scaleValue, PALETTE,
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
  focus: null,        // cell descended into (breadcrumb), or null
  home: null,         // level shown before the first descent
  wanted: parseHash(location.hash), // level and scale asked by the URL
  hover: null,        // cell under the pointer, whose level below is shown
  show: "channels",   // "channels": own activity; "meshes": sum of the channels inside
  meshes: [],         // active meshes, every precision
  meshLevel: null,    // mesh precision shown
  meshMax: 1,
  showRelays: false,  // relay layer
};
if (state.wanted.scale) state.scale = state.wanted.scale;
if (state.wanted.show) state.show = state.wanted.show;
if (state.wanted.relays) state.showRelays = true;

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

// Cells without colour still get a transparent fill: in SVG, the inside of
// an unfilled shape takes no hover nor click, only its outline does.
const CLEAR = { fill: true, fillColor: "#000", fillOpacity: 0 };

function styleFor(cell, max = state.max) {
  if (!cell.watched) {
    return { ...CLEAR, color: colour("muted"), weight: 1, dashArray: "4 4", opacity: 0.8 };
  }
  const value = scaleValue(cell, state.scale);
  if (value === "low") {
    return { color: PALETTE[2], weight: 1, dashArray: null, fill: true,
             fillColor: "url(#low-hatch)", fillOpacity: 0.8, opacity: 0.8 };
  }
  const fill = fillColour(value, max);
  if (!fill) {
    return { ...CLEAR, color: colour("line"), weight: 1, dashArray: null, opacity: 0.45 };
  }
  return { color: fill, weight: 1, dashArray: null, fill: true, fillColor: fill,
           fillOpacity: 0.55, opacity: 0.9 };
}

function group(level) {
  let g = groups.get(level);
  if (!g) {
    g = L.layerGroup();
    groups.set(level, g);
    if (level === state.level && state.show === "channels") g.addTo(map);
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
    layer.on("click", e => {
      layer.closeTooltip();
      endHover(cell.geohash);
      openMenu(cell.geohash, e.latlng);
    });
    layer.on("mouseover", () => startHover(cell.geohash));
    layer.on("mouseout", () => endHover(cell.geohash));
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
  if (state.show === "channels") group(level).addTo(map);
  else group(level);
  rescale();
  saveView();
  updatePanel();
}

// Shows a coarser or finer level from the drop-down list or the URL. Going
// up, the map zooms out to the cells of that level in view, whole.
function changeLevel(level) {
  const up = state.level !== null && level < state.level;
  showLevel(level);
  if (!up) return;
  const b = map.getBounds();
  const inView = cellsMeeting(state.cells.values(), level,
    [[b.getSouth(), b.getWest()], [b.getNorth(), b.getEast()]]);
  if (inView.length) map.fitBounds(L.latLngBounds(inView.map(c => c.bbox)), { padding: [20, 20] });
}

// Level, scale and layers in the URL, to share a view between tabs.
function saveView() {
  const level = state.show === "meshes" ? state.meshLevel : state.level;
  if (level !== null) {
    history.replaceState(null, "", formatHash(level, state.scale,
      { show: state.show, relays: state.showRelays }));
  }
}

window.addEventListener("hashchange", () => {
  const wanted = parseHash(location.hash);
  if (wanted.scale && wanted.scale !== state.scale) {
    state.scale = wanted.scale;
    state.max = -1;
    rescale();
    drawMeshes();
  }
  if ((wanted.relays ?? false) !== state.showRelays) setRelays(wanted.relays ?? false);
  const show = wanted.show || "channels";
  if (show === "meshes") {
    if (wanted.level && state.params?.mesh_levels.includes(wanted.level)) state.meshLevel = wanted.level;
    if (show !== state.show) setShow(show);
    else drawMeshes();
  } else {
    if (show !== state.show) setShow(show);
    if (wanted.level && wanted.level !== state.level) changeLevel(wanted.level);
  }
  saveView();
  updatePanel();
});

// --- Mesh layer (lot 16) ----------------------------------------------------

// A mesh is a geohash prefix (level 3 included): its rectangle sums the
// channels inside it, apart from the mesh's own channel.
const meshLayer = L.layerGroup();
const meshRects = new Map();   // "level:geohash" -> L.Rectangle

// Switches between the channels' own activity and the meshes.
function setShow(show) {
  state.show = show;
  const cells = state.level !== null ? groups.get(state.level) : null;
  if (show === "meshes") {
    cells?.remove();
    endHover(state.hover);
    if (state.meshLevel === null) {
      // The mesh level just above the channels shown.
      const levels = state.params ? state.params.mesh_levels : [];
      state.meshLevel = [...levels].reverse().find(l => l < (state.level ?? 99)) ?? levels[0] ?? null;
    }
    meshLayer.addTo(map);
  } else {
    meshLayer.remove();
    cells?.addTo(map);
  }
  drawMeshes();
  saveView();
  updatePanel();
}

function meshStyle(m, max) {
  return styleFor({ ...m, watched: true }, max);
}

// Draws the meshes of the level shown, updating rectangles in place so that
// an open tooltip stays open.
function drawMeshes() {
  const shown = state.show === "meshes"
    ? state.meshes.filter(m => m.level === state.meshLevel) : [];
  const max = maxValue(shown, state.scale);
  state.meshMax = max;
  const keep = new Set();
  for (const m of shown) {
    const key = `${m.level}:${m.geohash}`;
    keep.add(key);
    let r = meshRects.get(key);
    if (!r) {
      r = L.rectangle(m.bbox, { renderer, ...meshStyle(m, max) });
      r.bindTooltip(() => meshTip(r.mesh), {
        sticky: true, direction: "auto", offset: [12, 0], className: "cell-tip",
      });
      // The mesh's own channel, if watched, has the usual menu.
      r.on("click", e => {
        if (state.cells.has(r.mesh.geohash)) {
          r.closeTooltip();
          openMenu(r.mesh.geohash, e.latlng);
        }
      });
      meshRects.set(key, r);
      meshLayer.addLayer(r);
    } else {
      r.setStyle(meshStyle(m, max));
    }
    r.mesh = m;
    if (r.isTooltipOpen()) r.setTooltipContent(meshTip(m));
  }
  for (const [key, r] of meshRects) {
    if (!keep.has(key)) {
      meshLayer.removeLayer(r);
      meshRects.delete(key);
    }
  }
}

function meshTip(m) {
  const box = el("div", "tip");
  const head = el("div", "tip-head");
  head.append(el("b", null, "#" + m.geohash), el("span", "muted",
    ` · mesh, level ${m.level} · channels inside`));
  box.append(head);
  const counts = el("div", "tip-counts");
  for (const [label, value] of [["active channels", m.active_channels], ["messages", m.messages],
                                ["presences", m.presences], ["participants", m.participants]]) {
    const item = el("span", "count");
    item.append(el("b", null, String(value)), el("span", "muted", " " + label));
    counts.append(item);
  }
  box.append(counts);
  box.append(el("div", "tip-below muted",
    "Participants are distinct keys: the app signs with one key per channel, so one person " +
    "writing in two channels counts twice. The mesh's own channel is not included."));
  return box;
}

// --- Relay layer (lot 16) ---------------------------------------------------

// One pin per relay of the plan: colour = status, round = chosen by both
// apps, square = by one app only, faded = not listened to (set aside, or
// only retired channels). Relays at one point sit on a ring.
const relayLayer = L.layerGroup();
const pins = new Map();        // url -> { marker, shape }
const links = L.layerGroup().addTo(map);
const PIN = 16;

function relayByUrl(url) {
  return state.relays.find(r => r.url === url);
}

function setRelays(on) {
  state.showRelays = on;
  if (on) relayLayer.addTo(map);
  else relayLayer.remove();
  drawRelays();
  drawBelow();
  saveView();
  updatePanel();
}

function drawRelays() {
  if (!state.showRelays) {
    relayLayer.clearLayers();
    pins.clear();
    return;
  }
  const keep = new Set();
  for (const [r, i, n] of ringed(state.relays)) {
    keep.add(r.url);
    // Position, place on the ring and shape: a change recreates the pin.
    const shape = `${r.lat},${r.lon},${i},${n},${r.ios && r.android}`;
    let pin = pins.get(r.url);
    if (!pin || pin.shape !== shape) {
      if (pin) relayLayer.removeLayer(pin.marker);
      const icon = L.divIcon({
        className: "pin" + (r.ios && r.android ? "" : " one"),
        iconSize: [PIN, PIN],
        ...ringAnchors(i, n, PIN),
      });
      const marker = L.marker([r.lat, r.lon], { icon, keyboard: false, riseOnHover: true });
      const url = r.url;
      marker.bindTooltip(() => el("span", null, url.replace(/^wss?:\/\//, "")), { direction: "top" });
      marker.bindPopup(() => relayPopup(relayByUrl(url) || r), { maxWidth: 320 });
      marker.on("add", () => paintPin(marker, relayByUrl(url) || r));
      pin = { marker, shape };
      pins.set(r.url, pin);
      relayLayer.addLayer(marker);
    } else {
      paintPin(pin.marker, r);
      if (pin.marker.isPopupOpen()) pin.marker.setPopupContent(relayPopup(r));
    }
  }
  for (const [url, pin] of pins) {
    if (!keep.has(url)) {
      relayLayer.removeLayer(pin.marker);
      pins.delete(url);
    }
  }
}

function paintPin(marker, r) {
  const e = marker.getElement();
  if (!e) return;
  e.style.background = colour(r.status || "unprobed");
  e.classList.toggle("idle", !r.listened);
}

function relayPopup(r) {
  const box = el("div", "tip");
  box.append(el("b", null, r.url));
  const status = el("div");
  const dot = el("span", "dot");
  dot.style.background = colour(r.status || "unprobed");
  status.append(dot, document.createTextNode(r.status || "unprobed"));
  box.append(status,
    el("div", null, `chosen by ${appsLabel(r)} · ${r.channels} watched channels`),
    el("div", "muted", r.listened ? "listened to" : "not listened to"));
  r.connections.forEach((c, i) => {
    const text = c.state === "waiting" ? `waiting, retry in ${c.retry_in_secs} s`
      : c.state === "abandoned" ? `abandoned${c.auth ? " (AUTH)" : ""}: ${c.reason}` : c.state;
    box.append(el("div", "muted", `connection ${i + 1}: ${text}`));
  });
  return box;
}

// Lines from the hovered cell to its relays: solid when both apps choose
// the relay, dashed when only one does.
function drawLinks() {
  links.clearLayers();
  const cell = state.hover && state.cells.get(state.hover);
  if (!cell || !state.showRelays) return;
  const centre = L.latLngBounds(cell.bbox).getCenter();
  for (const cr of cell.relays) {
    const r = relayByUrl(cr.url);
    if (!r) continue;
    const both = cr.ios !== null && cr.android.length > 0;
    L.polyline([centre, [r.lat, r.lon]], {
      interactive: false, weight: 2, opacity: 0.8,
      color: colour(r.status || "unprobed"), dashArray: both ? null : "6 6",
    }).addTo(links);
  }
}

// --- Preview of the level below (D21) --------------------------------------

// Hovering a watched cell for a moment asks the server to listen to the
// channels below it for a while; they are drawn inside the cell, on their
// own colour scale, without taking the pointer.
const HOVER_DELAY = 400;       // ms on a cell before asking
const ASK_AGAIN = 60 * 1000;   // ms before asking again for the same cell
const overlay = L.layerGroup().addTo(map);
const asked = new Map();       // geohash -> time of the latest request
let hoverTimer = null;

function startHover(geohash) {
  const cell = state.cells.get(geohash);
  clearTimeout(hoverTimer);
  if (!cell) return;
  state.hover = geohash;
  drawBelow();
  if (!cell.watched || childLevel(cell.level) === null) return;
  hoverTimer = setTimeout(() => {
    if (state.hover !== geohash) return;
    if (Date.now() - (asked.get(geohash) || 0) < ASK_AGAIN) return;
    asked.set(geohash, Date.now());
    // A refusal (cap reached) only shows as an empty level below.
    command("/api/preview", { geohash });
  }, HOVER_DELAY);
}

function endHover(geohash) {
  if (!geohash || state.hover !== geohash) return;
  clearTimeout(hoverTimer);
  state.hover = null;
  overlay.clearLayers();
  links.clearLayers();
}

// Draws the channels below the hovered cell and the lines to its relays,
// and refreshes its tooltip.
function drawBelow() {
  overlay.clearLayers();
  drawLinks();
  const g = state.hover;
  if (!g) return;
  const children = childrenOf(state.cells.values(), g);
  const max = maxValue(children, state.scale);
  for (const c of children) {
    L.rectangle(c.bbox, { renderer, interactive: false, ...styleFor(c, max), weight: 1, opacity: 0.8 })
      .addTo(overlay);
  }
  const layer = layers.get(g);
  if (layer && layer.isTooltipOpen()) layer.setTooltipContent(tooltip(state.cells.get(g)));
}

// --- Commands (lot 15) -----------------------------------------------------

async function command(path, body) {
  try {
    const resp = await fetch(path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    let data = null;
    try { data = await resp.json(); } catch { /* not JSON */ }
    return { ok: resp.ok, data };
  } catch (e) {
    return { ok: false, data: { message: "server unreachable" } };
  }
}

// Bounds of a geohash: its own cell if drawn, else the cells inside it.
function boundsOf(geohash) {
  const own = state.cells.get(geohash);
  if (own) return L.latLngBounds(own.bbox);
  const inside = [...state.cells.values()].filter(c => c.geohash.startsWith(geohash));
  return inside.length ? L.latLngBounds(inside.map(c => c.bbox)) : null;
}

// Shows the channels below `geohash`, framed on it.
function focusOn(geohash) {
  if (state.focus === null) state.home = state.level;
  state.focus = geohash;
  showLevel(childLevel(geohash.length) ?? geohash.length);
  const bounds = boundsOf(geohash);
  if (bounds && bounds.isValid()) map.fitBounds(bounds, { padding: [20, 20] });
}

// Shows the level of `geohash`, framed on it: going back up from a cell.
function upTo(geohash) {
  if (state.focus === null) state.home = state.level;
  const start = state.params ? state.params.start.length : 0;
  const parents = ancestors(geohash).slice(0, -1).filter(g => g.length > start);
  state.focus = parents.length ? parents[parents.length - 1] : null;
  showLevel(geohash.length);
  const bounds = boundsOf(geohash);
  if (bounds && bounds.isValid()) map.fitBounds(bounds, { padding: [20, 20] });
}

// Back to the whole area, at the level shown before the descents.
function focusAll() {
  state.focus = null;
  const levels = levelCounts(state.cells.values()).map(([l]) => l);
  if (levels.includes(state.home)) showLevel(state.home);
  else if (levels.length) showLevel(levels[0]);
  const bounds = L.latLngBounds([...state.cells.values()].map(c => c.bbox));
  if (bounds.isValid()) map.fitBounds(bounds, { padding: [20, 20] });
}

function openMenu(geohash, latlng) {
  const cell = state.cells.get(geohash);
  if (!cell) return;
  const box = el("div", "menu");
  const head = el("div", "menu-head");
  head.append(el("b", null, "#" + geohash), el("span", "muted", " · " + levelLabel(cell.level)));
  const result = el("div", "result muted");
  box.append(head);
  const button = (label, action) => {
    const b = el("button", null, label);
    b.addEventListener("click", async () => {
      b.disabled = true;
      result.className = "result muted";
      result.textContent = "…";
      await action();
      b.disabled = false;
    });
    box.append(b);
  };
  if (cell.watched) {
    const child = childLevel(cell.level);
    if (child === null) {
      box.append(el("div", "muted", "finest channel level: nothing below"));
    } else {
      button(`Descend to ${levelLabel(child)}`, async () => {
        const { ok, data } = await command("/api/descend", { geohash });
        if (!ok) {
          result.className = "result warn";
          result.textContent = refusalText(data);
          return;
        }
        const n = data.added.length;
        result.textContent = n ? `${n} channels added` : "already watched";
        focusOn(geohash);
        map.closePopup();
      });
    }
  } else {
    box.append(el("div", "muted", "retired: no event for a while, no longer listened to"));
    button("Reinstate", async () => {
      const { ok, data } = await command("/api/reinstate", { channels: [geohash] });
      result.className = ok ? "result muted" : "result warn";
      result.textContent = ok ? "listening again" : refusalText(data);
    });
  }
  // Going back up: one button per coarser level drawn, nearest first.
  const levels = levelCounts(state.cells.values()).map(([l]) => l);
  const ups = ancestors(geohash).slice(0, -1).reverse().filter(g => levels.includes(g.length));
  for (const g of ups) {
    const b = el("button", "up", `Up to #${g} · ${levelLabel(g.length)}`);
    b.addEventListener("click", () => {
      map.closePopup();
      upTo(g);
    });
    box.append(b);
  }
  box.append(result);
  L.popup({ className: "cell-menu", maxWidth: 280 }).setLatLng(latlng).setContent(box).openOn(map);
}

// --- Tooltip ---------------------------------------------------------------

function tooltip(cell) {
  const box = el("div", "tip");
  const head = el("div", "tip-head");
  head.append(el("b", null, "#" + cell.geohash), el("span", "muted",
    ` · ${levelLabel(cell.level)}${cell.watched ? "" : " · retired"}` +
    (cell.preview ? " · preview" : "")));
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
  const below = childLevel(cell.level);
  if (cell.watched && below !== null) {
    const s = belowSummary(childrenOf(state.cells.values(), cell.geohash));
    box.append(el("div", "tip-below", s.total
      ? `${levelLabel(below)} below: ${s.active} active / ${s.total}, ` +
        `${s.messages} messages, ${s.presences} presences` +
        (s.preview ? " (preview: listened to for a few minutes, new events only)" : "")
      : `${levelLabel(below)} below: listening…`));
  }
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
  ui.crumbs = el("nav", "crumbs");
  ui.crumbs.setAttribute("aria-label", "levels");
  ui.stats = el("div", "stats");
  ui.reinstateAll = el("button", "reinstate-all", "Reinstate all retired channels");
  ui.reinstateAll.addEventListener("click", async () => {
    const n = state.status ? state.status.retired : 0;
    if (!confirm(`Listen again to the ${n} retired channels?`)) return;
    ui.reinstateAll.disabled = true;
    const { ok, data } = await command("/api/reinstate", { all: true });
    ui.reinstateAll.disabled = false;
    if (!ok) alert(refusalText(data));
  });
  ui.level = el("select");
  ui.level.setAttribute("aria-label", "level shown");
  ui.level.addEventListener("change", () => {
    if (state.show === "meshes") {
      state.meshLevel = Number(ui.level.value);
      drawMeshes();
      saveView();
      updatePanel();
    } else {
      changeLevel(Number(ui.level.value));
    }
  });
  ui.show = el("select");
  ui.show.setAttribute("aria-label", "what the rectangles show");
  for (const [value, label] of [["channels", "each channel's activity"],
                                ["meshes", "sum of the channels inside (meshes)"]]) {
    const o = el("option", null, label);
    o.value = value;
    ui.show.append(o);
  }
  ui.show.addEventListener("change", () => setShow(ui.show.value));
  ui.relays = el("input");
  ui.relays.type = "checkbox";
  ui.relays.addEventListener("change", () => setRelays(ui.relays.checked));
  const relaysLabel = el("label", "check");
  relaysLabel.append(ui.relays, document.createTextNode(" relays"));
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
    drawMeshes();
    saveView();
    updatePanel();
  });
  const controls = el("div", "controls");
  controls.append(ui.show, ui.level, ui.scale, relaysLabel);

  const legend = el("div", "legend");
  const bar = el("div", "bar");
  bar.style.background = `linear-gradient(to right, ${PALETTE.join(", ")})`;
  ui.max = el("span");
  const ends = el("div", "ends");
  ends.append(el("span", null, "1"), ui.max);
  ui.low = el("span", "key low");
  const keys = el("div", "keys muted");
  keys.append(el("span", "key none", "no activity"), el("span", "key retired", "retired"), ui.low);
  ui.relayKeys = el("div", "keys muted");
  ui.relayKeys.append(el("span", "key-pin both", "both apps"), el("span", "key-pin one", "one app"),
    el("span", "key-pin idle", "not listened to"),
    el("span", null, "hover a cell: lines to its relays, dashed for one app"));
  legend.append(bar, ends, keys, ui.relayKeys);

  body.append(ui.crumbs, ui.stats, ui.reinstateAll, controls, legend, el("div", "note muted",
    "Presences are not comparable between levels: the apps send heartbeats at levels 2, 4 and 5 only."));
  panel.append(head, ui.link, ui.tabs, body);
}

function updatePanel() {
  ui.link.className = "link " + state.link;
  ui.linkText.textContent = {
    live: "live", connecting: "connecting…", reconnecting: "reconnecting…", tabs: "no live stream",
    paused: "paused while the tab is hidden",
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

  ui.reinstateAll.hidden = !(state.status && state.status.retired > 0);
  updateCrumbs();

  const meshes = state.show === "meshes";
  // Channel levels, finest first, or mesh levels with their active meshes.
  const counts = meshes
    ? [...(state.params ? state.params.mesh_levels : [])].reverse()
      .map(l => [l, state.meshes.filter(m => m.level === l).length])
    : levelCounts(state.cells.values());
  const options = [...ui.level.options];
  if (options.map(o => Number(o.value)).join() !== counts.map(([l]) => l).join()) {
    ui.level.replaceChildren(...counts.map(([level]) => {
      const o = el("option");
      o.value = String(level);
      return o;
    }));
  }
  counts.forEach(([level, active, all], i) => {
    ui.level.options[i].textContent = meshes
      ? `meshes of level ${level}: ${active} active`
      : `${levelLabel(level)}: ${active} active / ${all}`;
  });
  const level = meshes ? state.meshLevel : state.level;
  if (level !== null) ui.level.value = String(level);
  ui.scale.value = state.scale;
  ui.show.value = state.show;
  ui.relays.checked = state.showRelays;
  ui.relayKeys.hidden = !state.showRelays;

  const max = meshes ? state.meshMax : state.max;
  ui.max.textContent = `${Math.max(max, 1)} ${state.scale} (log scale)`;
  const k = state.params ? state.params.k : 0;
  ui.low.hidden = !(k > 0);
  ui.low.textContent = `fewer than ${k} participants`;
}

// "#u09 › #u09t › #u09tv": click to go back up (display only: a descent
// is not undone).
function updateCrumbs() {
  ui.crumbs.hidden = !state.focus || !state.params;
  if (ui.crumbs.hidden) return;
  const start = state.params.start;
  const items = [["#" + start + " (all)", focusAll]];
  for (const g of ancestors(state.focus)) {
    if (g.length > start.length) items.push(["#" + g, () => focusOn(g)]);
  }
  ui.crumbs.replaceChildren();
  items.forEach(([label, go], i) => {
    if (i) ui.crumbs.append(el("span", "muted", " › "));
    const last = i === items.length - 1;
    const a = el(last ? "b" : "button", last ? null : "crumb", label);
    if (!last) a.addEventListener("click", go);
    ui.crumbs.append(a);
  });
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
    tiles = L.tileLayer(d.params.tiles, {
      maxZoom: 19,
      attribution: d.params.attribution,
      // The page sends no Referer (Referrer-Policy: no-referrer), but the
      // OpenStreetMap tile policy requires one: browsers that honour the
      // page policy strictly (Firefox) get "Access blocked" (403). Tiles get
      // the page origin only, never the path nor the #level part.
      referrerPolicy: "strict-origin-when-cross-origin",
    }).addTo(map);
  }
  const seen = new Set(d.cells.map(c => c.geohash));
  for (const g of [...state.cells.keys()]) if (!seen.has(g)) remove(g);
  for (const c of d.cells) upsert(c);
  state.relays = d.relays;
  state.meshes = d.meshes;
  state.status = d.status;
  state.seq = d.seq;
  const levels = levelCounts(d.cells).map(([l]) => l);
  if (state.level === null || !levels.includes(state.level)) {
    if (levels.length) {
      // The level asked by the URL, else the finest.
      showLevel(levels.includes(state.wanted.level) ? state.wanted.level : levels[0]);
      const bounds = L.latLngBounds(d.cells.map(c => c.bbox));
      if (bounds.isValid()) map.fitBounds(bounds, { padding: [20, 20] });
    }
  }
  rescale();
  if (state.show === "meshes" && state.wanted.level && d.params.mesh_levels.includes(state.wanted.level)
      && state.meshLevel === null) {
    state.meshLevel = state.wanted.level;
  }
  if (state.show === "meshes" && !map.hasLayer(meshLayer)) setShow("meshes");
  drawMeshes();
  if (state.showRelays && !map.hasLayer(relayLayer)) setRelays(true);
  else drawRelays();
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
    drawBelow();
    updatePanel();
  }));
  source.addEventListener("removed", e => delta(e, d => {
    for (const g of d.cells) remove(g);
    // An ended preview may empty the level shown: show the nearest one.
    const levels = levelCounts(state.cells.values()).map(([l]) => l);
    if (levels.length && !levels.includes(state.level)) {
      showLevel(levels.find(l => l < state.level) ?? levels[0]);
    }
    drawBelow();
    updatePanel();
  }));
  source.addEventListener("relays", e => delta(e, d => {
    state.relays = d.relays;
    drawRelays();
    drawLinks();
  }));
  source.addEventListener("meshes", e => delta(e, d => {
    state.meshes = d.meshes;
    drawMeshes();
    updatePanel();
  }));
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

// A hidden tab lets go of its stream after a while, and takes it again
// (with a new `hello`) when shown: in HTTP/1.1 every open stream holds one
// of the 6 connections per host, and past them even the page of a new tab
// cannot load.
const PAUSE_AFTER = 10 * 1000;
let pauseTimer = null;

function pause() {
  clearTimeout(helloTimer);
  clearTimeout(retryTimer);
  if (source) source.close();
  source = null;
  setLink("paused");
}

document.addEventListener("visibilitychange", () => {
  clearTimeout(pauseTimer);
  if (document.hidden) pauseTimer = setTimeout(pause, PAUSE_AFTER);
  else if (!source) connect();
});

buildPanel();
updatePanel();
connect();
