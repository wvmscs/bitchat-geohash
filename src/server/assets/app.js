// Map page. Live data arrives from lot 13 on (SSE, see PLAN_ET2.md).

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

const map = L.map("map").setView([20, 0], 2);
L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", {
  maxZoom: 19,
  attribution: '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors',
}).addTo(map);

const panel = document.getElementById("panel");
panel.append(el("h1", null, "BitChat geohash activity"));
panel.append(el("div", "sub", "waiting for the monitor"));
