// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

// Pure helpers of the map page: colour scale, labels, counts. No DOM, so
// that node can check them (see PLAN_ET2.md, lot 14).

// Viridis, readable by colour-blind people (D16).
export const PALETTE = [
  "#440154", "#482878", "#3e4989", "#31688e", "#26828e",
  "#1f9e89", "#35b779", "#6ece58", "#b5de2b", "#fde725",
];

export const LEVEL_NAMES = { 2: "region", 3: "mesh", 4: "province", 5: "city", 6: "neighbourhood", 7: "block" };

export const levelLabel = level => `level ${level} (${LEVEL_NAMES[level] || "?"})`;

// The value a cell is coloured by: a number, "low" (hidden below k), or 0.
export function scaleValue(cell, scale) {
  return scale === "messages" ? cell.messages : cell.participants;
}

// Log scale: 1 and `max` are the two ends; 0 has no colour.
export function fillColour(value, max) {
  if (typeof value !== "number" || value <= 0) return null;
  const t = max > 1 ? Math.log(value) / Math.log(max) : 1;
  const i = Math.round(Math.min(1, Math.max(0, t)) * (PALETTE.length - 1));
  return PALETTE[i];
}

// Largest numeric value among `cells` (at least 1).
export function maxValue(cells, scale) {
  let max = 1;
  for (const c of cells) {
    const v = scaleValue(c, scale);
    if (typeof v === "number" && v > max) max = v;
  }
  return max;
}

// A cell had activity in the window.
export const isActive = c => c.messages > 0 || c.presences > 0;

// `seconds` ago, in words.
export function ago(seconds) {
  if (seconds < 0) seconds = 0;
  if (seconds < 60) return `${Math.round(seconds)} s ago`;
  if (seconds < 3600) return `${Math.round(seconds / 60)} min ago`;
  if (seconds < 86400) return `${Math.round(seconds / 3600)} h ago`;
  return `${Math.round(seconds / 86400)} d ago`;
}

// "#1", "#2+3" (Android picked the server twice) or "–".
export function rank(r) {
  if (Array.isArray(r)) return r.length ? "#" + r.join("+") : "–";
  return r ? "#" + r : "–";
}

// Per level: [level, active cells, all cells], finest first.
export function levelCounts(cells) {
  const by = new Map();
  for (const c of cells) {
    const e = by.get(c.level) || [c.level, 0, 0];
    e[2] += 1;
    if (isActive(c)) e[1] += 1;
    by.set(c.level, e);
  }
  return [...by.values()].sort((a, b) => b[0] - a[0]);
}

// Total of the rejection counters.
export const rejectedTotal = status =>
  Object.values(status.rejected || {}).reduce((a, b) => a + b, 0);

// Channel levels of the app; 3 is only a mesh.
export const CHANNEL_LEVELS = [2, 4, 5, 6, 7];

// The channel level below `level`, or null under the finest.
export function childLevel(level) {
  return CHANNEL_LEVELS.find(l => l > level) ?? null;
}

// The channels from the coarsest down to `geohash` itself, among the
// channel levels: the breadcrumb of a descent. "u09tv" -> ["u0", "u09t", "u09tv"].
export function ancestors(geohash) {
  return CHANNEL_LEVELS.filter(l => l <= geohash.length).map(l => geohash.slice(0, l));
}

// `#level=5&scale=messages&show=meshes&relays=1` -> { level: 5, scale:
// "messages", show: "meshes", relays: true }; unknown or bad values are left out.
export function parseHash(hash) {
  const out = {};
  for (const part of hash.replace(/^#/, "").split("&")) {
    const [k, v] = part.split("=");
    if (k === "level" && /^[2-7]$/.test(v || "")) out.level = Number(v);
    if (k === "scale" && (v === "participants" || v === "messages")) out.scale = v;
    if (k === "show" && (v === "channels" || v === "meshes")) out.show = v;
    if (k === "relays" && (v === "0" || v === "1")) out.relays = v === "1";
  }
  return out;
}

// The defaults (channels, no relays) stay out of the URL.
export const formatHash = (level, scale, { show = "channels", relays = false } = {}) =>
  `#level=${level}&scale=${scale}` + (show === "meshes" ? "&show=meshes" : "") +
  (relays ? "&relays=1" : "");

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

// Relays grouped by coordinates: [[relay, index, count], ...], so that
// relays at one point go on one ring.
export function ringed(relays) {
  const by = new Map();
  for (const r of relays) {
    const key = `${r.lat},${r.lon}`;
    if (!by.has(key)) by.set(key, []);
    by.get(key).push(r);
  }
  const out = [];
  for (const group of by.values()) group.forEach((r, i) => out.push([r, i, group.length]));
  return out;
}

// "iOS and Android", "iOS only", "Android only".
export const appsLabel = r =>
  r.ios && r.android ? "iOS and Android" : r.ios ? "iOS only" : "Android only";

// A refusal from the server, in words.
// What the area field holds, as the server compares it with the watched
// area: `#U09T` and `u09t` are the cell `u09t`; a place stays as typed.
export function typedArea(text) {
  const t = text.trim();
  const g = t.replace(/^#\s*/, "").toLowerCase();
  return /^[0-9b-hjkmnp-z]{1,12}$/.test(g) ? g : t;
}

export function refusalText(body) {
  switch (body && body.error) {
    case "too_many_channels":
      return `refused: ${body.count} channels would be watched, more than the maximum of ${body.max}`;
    case "no_finer_level":
      return "no channel level below this one";
    case "outside_area":
      return `refused: outside the watched area #${body.start}`;
    case "invalid_geohash":
      return "not a geohash: 2 to 7 characters among 0-9 and b-z, without a, i, l, o";
    case "area_fixed":
      return "this map watches a fixed area";
    case "monitor_stopped":
      return "the area is changing: try again in a moment";
    case "place_not_found":
      return "no place found: try a town, an address, or #geohash";
    case "geocode":
      return "the place lookup failed: " + ((body && body.message) || "unknown error");
    default:
      return "refused: " + ((body && body.message) || "unknown error");
  }
}

// The cells of the channel level just below `geohash` (D21).
export function childrenOf(cells, geohash) {
  const level = childLevel(geohash.length);
  if (level === null) return [];
  return [...cells].filter(c => c.level === level && c.geohash.startsWith(geohash));
}

// Activity below a cell: active channels, all channels, messages and
// presences summed. Participants are not summed: one person can be in two.
export function belowSummary(children) {
  const out = { active: 0, total: children.length, messages: 0, presences: 0, preview: false };
  for (const c of children) {
    if (isActive(c)) out.active += 1;
    out.messages += c.messages;
    out.presences += c.presences;
    out.preview ||= c.preview;
  }
  return out;
}

// Cells of `level` whose box meets `[[s, w], [n, e]]`.
export function cellsMeeting(cells, level, [[s, w], [n, e]]) {
  return [...cells].filter(c => {
    const [[cs, cw], [cn, ce]] = c.bbox;
    return c.level === level && cs < n && cn > s && cw < e && ce > w;
  });
}
