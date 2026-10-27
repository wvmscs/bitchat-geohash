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
