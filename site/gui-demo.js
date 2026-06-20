/* etchy GUI viewer demo — drives the hero "GUI" tab.
 * Renders simple example geometry per layer and per view mode (overlay / before /
 * after / split). Pure vanilla; no dependencies. Degrades gracefully: if anything
 * is missing the page just shows an empty canvas.
 */
(function () {
  "use strict";
  var canvas = document.getElementById("gd-canvas");
  var layersWrap = document.getElementById("gd-layers");
  var modesWrap = document.getElementById("gd-modes");
  if (!canvas || !layersWrap || !modesWrap) return;

  var NEUTRAL = "#b9a888", GREEN = "#46d18a", RED = "#ff5d73";

  // Per layer: base geometry (unchanged), added (in revB only), removed (in revA only).
  // Each value is raw SVG element markup; colour is applied by the wrapping <g> at render.
  var DATA = {
    top: {
      base: '<path d="M40 50 H230 V120 H380"/><path d="M40 95 H180 V160 H320"/><circle cx="40" cy="50" r="8"/>',
      added: '<path d="M380 120 H460 V165 H490"/><circle cx="490" cy="165" r="9"/>',
      removed: '<path d="M180 160 H320"/>'
    },
    bottom: {
      base: '<path d="M60 60 H300"/><path d="M60 110 H220 V170 H400"/><circle cx="60" cy="60" r="8"/>',
      added: '<path d="M300 60 V140 H460"/>',
      removed: ''
    },
    silk: {
      base: '<rect x="60" y="48" width="120" height="58" rx="5"/><path d="M210 70 H360"/>',
      added: '<rect x="372" y="120" width="86" height="44" rx="5"/>',
      removed: ''
    },
    drill: {
      base: '<circle cx="90" cy="70" r="11"/><circle cx="170" cy="70" r="11"/><circle cx="250" cy="70" r="11"/><circle cx="90" cy="140" r="11"/>',
      added: '',
      removed: '<circle cx="170" cy="140" r="11"/><circle cx="250" cy="140" r="11"/>'
    }
  };

  var cur = { layer: "top", mode: "overlay" };

  function grp(content, color, w) {
    if (!content) return "";
    return '<g stroke="' + color + '" stroke-width="' + (w || 5) + '" fill="none" stroke-linecap="round">' + content + "</g>";
  }

  function render() {
    var L = DATA[cur.layer] || DATA.top;
    var p = ['<svg viewBox="0 0 520 200" xmlns="http://www.w3.org/2000/svg"><rect width="520" height="200" fill="#0e1413"/>'];
    if (cur.mode === "overlay") {
      p.push(grp(L.base, NEUTRAL, 5), grp(L.added, GREEN, 6), grp(L.removed, RED, 6));
    } else if (cur.mode === "before") {
      // revA = unchanged geometry + the parts that were later removed
      p.push(grp(L.base, NEUTRAL, 5), grp(L.removed, NEUTRAL, 5));
    } else if (cur.mode === "after") {
      // revB = unchanged geometry + the parts that were added
      p.push(grp(L.base, NEUTRAL, 5), grp(L.added, NEUTRAL, 5));
    }
    p.push("</svg>");
    canvas.innerHTML = p.join("");
  }

  function activate(wrap, el) {
    var items = wrap.children;
    for (var i = 0; i < items.length; i++) items[i].classList.remove("active");
    el.classList.add("active");
  }

  layersWrap.addEventListener("click", function (e) {
    var b = e.target.closest("[data-layer]");
    if (!b) return;
    cur.layer = b.getAttribute("data-layer");
    activate(layersWrap, b);
    render();
  });
  modesWrap.addEventListener("click", function (e) {
    var b = e.target.closest("[data-mode]");
    if (!b) return;
    cur.mode = b.getAttribute("data-mode");
    activate(modesWrap, b);
    render();
  });

  render();
})();
