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
