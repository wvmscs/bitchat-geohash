// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

// Pure helpers of the map page (lot 14). Run: node --test tests/js/
import { test } from "node:test";
import assert from "node:assert/strict";
import * as v from "../../src/server/assets/view.js";

test("log colour scale", () => {
  assert.equal(v.fillColour(0, 10), null);
  assert.equal(v.fillColour("low", 10), null);
  assert.equal(v.fillColour(1, 10), v.PALETTE[0]);
  assert.equal(v.fillColour(10, 10), v.PALETTE[9]);
  assert.equal(v.fillColour(1, 1), v.PALETTE[9]);
  assert.equal(v.fillColour(50, 10), v.PALETTE[9]);
  assert.equal(v.maxValue([{ participants: 3 }, { participants: "low" }, { participants: 7 }], "participants"), 7);
  assert.equal(v.maxValue([], "messages"), 1);
});

test("labels", () => {
  assert.equal(v.ago(5), "5 s ago");
  assert.equal(v.ago(125), "2 min ago");
  assert.equal(v.ago(7300), "2 h ago");
  assert.equal(v.rank([2, 3]), "#2+3");
  assert.equal(v.rank([]), "–");
  assert.equal(v.rank(1), "#1");
  assert.equal(v.rank(null), "–");
  assert.equal(v.levelLabel(6), "level 6 (neighbourhood)");
});

test("counts per level, finest first", () => {
  const cells = [
    { level: 4, messages: 1, presences: 0 },
    { level: 5, messages: 0, presences: 0 },
    { level: 5, messages: 0, presences: 2 },
  ];
  assert.deepEqual(v.levelCounts(cells), [[5, 1, 2], [4, 1, 1]]);
  assert.equal(v.rejectedTotal({ rejected: { duplicate: 3, out_of_window: 2 } }), 5);
});

test("descent helpers", () => {
  assert.equal(v.childLevel(2), 4);
  assert.equal(v.childLevel(5), 6);
  assert.equal(v.childLevel(7), null);
  assert.deepEqual(v.ancestors("u09tv"), ["u0", "u09t", "u09tv"]);
  assert.deepEqual(v.ancestors("u09tvw"), ["u0", "u09t", "u09tv", "u09tvw"]);
  assert.deepEqual(v.ancestors("u0"), ["u0"]);
});

test("view in the URL", () => {
  assert.deepEqual(v.parseHash("#level=5&scale=messages"), { level: 5, scale: "messages" });
  assert.deepEqual(v.parseHash("#level=9&scale=evil"), {});
  assert.deepEqual(v.parseHash(""), {});
  assert.equal(v.formatHash(6, "participants"), "#level=6&scale=participants");
});

test("refusals in words", () => {
  assert.equal(
    v.refusalText({ error: "too_many_channels", count: 2080, max: 2048 }),
    "refused: 2080 channels would be watched, more than the maximum of 2048",
  );
  assert.equal(v.refusalText({ error: "outside_area", start: "u09" }), "refused: outside the watched area #u09");
  assert.equal(v.refusalText({ error: "x", message: "boom" }), "refused: boom");
  assert.equal(v.refusalText(null), "refused: unknown error");
  assert.match(v.refusalText({ error: "invalid_geohash" }), /^not a geohash/);
  assert.equal(v.refusalText({ error: "area_fixed" }), "this map watches a fixed area");
  assert.match(v.refusalText({ error: "place_not_found" }), /^no place found/);
  assert.equal(
    v.refusalText({ error: "geocode", message: "Nominatim request: 429" }),
    "the place lookup failed: Nominatim request: 429",
  );
});

test("typed area (lot 23)", () => {
  assert.equal(v.typedArea(" #U09T "), "u09t");
  assert.equal(v.typedArea("u09t"), "u09t");
  assert.equal(v.typedArea("# u09tv"), "u09tv");
  assert.equal(v.typedArea(" Rue de Rivoli, Paris "), "Rue de Rivoli, Paris");
  assert.equal(v.typedArea("Paris"), "Paris");
});

test("activity below a cell (preview, D21)", () => {
  const cells = [
    { geohash: "u09tv", level: 5, messages: 9, presences: 9 },
    { geohash: "u09tvq", level: 6, messages: 2, presences: 1, preview: true },
    { geohash: "u09tvr", level: 6, messages: 0, presences: 0, preview: true },
    { geohash: "u09tvqp", level: 7, messages: 5, presences: 0 },
    { geohash: "u09tuq", level: 6, messages: 4, presences: 0 },
  ];
  const below = v.childrenOf(cells, "u09tv");
  assert.deepEqual(below.map(c => c.geohash), ["u09tvq", "u09tvr"]);
  assert.deepEqual(v.belowSummary(below),
    { active: 1, total: 2, messages: 2, presences: 1, preview: true });
  assert.deepEqual(v.childrenOf(cells, "u09tvqp"), []);
  assert.equal(v.belowSummary([]).total, 0);
});

test("cells meeting the view", () => {
  const cells = [
    { level: 4, bbox: [[0, 0], [1, 1]] },
    { level: 4, bbox: [[2, 2], [3, 3]] },
    { level: 5, bbox: [[0, 0], [1, 1]] },
  ];
  assert.equal(v.cellsMeeting(cells, 4, [[0.5, 0.5], [0.9, 0.9]]).length, 1);
  assert.equal(v.cellsMeeting(cells, 4, [[-1, -1], [5, 5]]).length, 2);
  assert.equal(v.cellsMeeting(cells, 4, [[1, 1], [2, 2]]).length, 0);
});

test("relays at one point go on a ring (lot 16)", () => {
  const relays = [
    { url: "a", lat: 48.85, lon: 2.35, ios: true, android: true },
    { url: "b", lat: 48.85, lon: 2.35, ios: true, android: false },
    { url: "c", lat: 50.1, lon: 8.6, ios: false, android: true },
  ];
  const r = v.ringed(relays);
  assert.deepEqual(r.map(([x, i, n]) => [x.url, i, n]), [["a", 0, 2], ["b", 1, 2], ["c", 0, 1]]);
  // Alone: centred on its point; two: opposite sides, same distance.
  assert.deepEqual(v.ringAnchors(0, 1, 16).iconAnchor, [8, 8]);
  const [p0, p1] = [v.ringAnchors(0, 2, 16).iconAnchor, v.ringAnchors(1, 2, 16).iconAnchor];
  assert.ok(Math.abs(p0[0] - 8 + (p1[0] - 8)) < 1e-9 && Math.abs(p0[1] - 8 + (p1[1] - 8)) < 1e-9);
  assert.equal(Math.round(Math.hypot(p0[0] - 8, p0[1] - 8)), 18);
  assert.deepEqual(relays.map(v.appsLabel), ["iOS and Android", "iOS only", "Android only"]);
  assert.equal(v.appsLabel({ ios: false, android: false, strategy: true }), "the strategy");
  assert.ok(v.appsMode(null) && v.appsMode({ strategy: "apps" }) && !v.appsMode({ strategy: "a1" }));
});

test("URL view with meshes and relays", () => {
  assert.equal(v.formatHash(5, "messages"), "#level=5&scale=messages");
  const h = v.formatHash(3, "participants", { show: "meshes", relays: true });
  assert.equal(h, "#level=3&scale=participants&show=meshes&relays=1");
  assert.deepEqual(v.parseHash(h), { level: 3, scale: "participants", show: "meshes", relays: true });
  assert.deepEqual(v.parseHash("#show=x&relays=2"), {});
});

test("reach of the reference apps (lot 28)", () => {
  assert.deepEqual(v.reachBadges(null, false), []);
  assert.deepEqual(v.reachBadges({ ios: true, android: false }, true), [["bitchat iOS ↔ Android", true]]);
  assert.deepEqual(v.reachBadges({ ios: true, android: null }, false),
    [["bitchat iOS", true], ["bitchat Android", null]]);
  const cells = [
    { watched: true, reach: { ios: true, android: false } },
    { watched: true, reach: { ios: false, android: null } },
    { watched: false, reach: { ios: true, android: true } },
  ];
  assert.deepEqual(v.reachSummary(cells, false), [["bitchat iOS", [1, 2]], ["bitchat Android", [0, 1]]]);
});

test("lines end on the pins of a ring; places shared by relays", () => {
  assert.deepEqual(v.ringOffset(0, 1), [0, 0]);
  // Two pins: opposite sides, as their anchors (ringAnchors).
  const [a, b] = [v.ringOffset(0, 2), v.ringOffset(1, 2)];
  const anchor = v.ringAnchors(0, 2, 16).iconAnchor;
  assert.ok(Math.abs(8 - anchor[0] - a[0]) < 1e-9 && Math.abs(8 - anchor[1] - a[1]) < 1e-9);
  assert.ok(Math.abs(a[0] + b[0]) < 1e-9 && Math.abs(a[1] + b[1]) < 1e-9);
  assert.deepEqual(v.sharedPlaces([{ lat: 1, lon: 2 }, { lat: 1, lon: 2 }, { lat: 3, lon: 4 },
    { lat: 5, lon: 6 }, { lat: 5, lon: 6 }, { lat: 5, lon: 6 }]), [2, 3]);
  assert.ok(v.isDown("dead") && v.isDown("down") && !v.isDown("ok") && !v.isDown(null));
});
