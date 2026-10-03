/* etchy GUI viewer demo — drives the hero "GUI" tab.
 * Renders simple example geometry per layer and per view mode. Pure vanilla; no dependencies. Degrades gracefully: if anything
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
    } else if (cur.mode === "old") {
      p.push(grp(L.base, NEUTRAL, 5), grp(L.removed, NEUTRAL, 5));
    } else if (cur.mode === "new") {
      p.push(grp(L.base, NEUTRAL, 5), grp(L.added, NEUTRAL, 5));
    } else if (cur.mode === "split") {
      p.push('<g transform="translate(0 45) scale(.48)">', grp(L.base, NEUTRAL, 5), grp(L.removed, NEUTRAL, 5), '</g>');
      p.push('<line x1="260" y1="12" x2="260" y2="188" stroke="#23302d"/>');
      p.push('<g transform="translate(270 45) scale(.48)">', grp(L.base, NEUTRAL, 5), grp(L.added, NEUTRAL, 5), '</g>');
      p.push('<text x="12" y="25" fill="#8b9491" font-size="12">Old</text><text x="282" y="25" fill="#8b9491" font-size="12">New</text>');
    } else if (cur.mode === "swipe") {
      p.push('<defs><clipPath id="old-half"><rect width="260" height="200"/></clipPath><clipPath id="new-half"><rect x="260" width="260" height="200"/></clipPath></defs>');
      p.push('<g clip-path="url(#old-half)">', grp(L.base, NEUTRAL, 5), grp(L.removed, NEUTRAL, 5), '</g>');
      p.push('<g clip-path="url(#new-half)">', grp(L.base, NEUTRAL, 5), grp(L.added, NEUTRAL, 5), '</g>');
      p.push('<line x1="260" y1="0" x2="260" y2="200" stroke="#f6c873" stroke-width="3"/><circle cx="260" cy="100" r="8" fill="#f6c873"/>');
    }
    p.push("</svg>");
    canvas.innerHTML = p.join("");
  }

  function activate(wrap, el) {
    var items = wrap.children;
    for (var i = 0; i < items.length; i++) {
      items[i].classList.remove("active");
      items[i].setAttribute("aria-pressed", "false");
    }
    el.classList.add("active");
    el.setAttribute("aria-pressed", "true");
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

  var tabs = Array.prototype.slice.call(document.querySelectorAll(".hp-tab"));
  function selectTab(tab) {
    tabs.forEach(function (item) {
      var selected = item === tab;
      var panel = document.getElementById(item.getAttribute("aria-controls"));
      item.setAttribute("aria-selected", selected ? "true" : "false");
      item.tabIndex = selected ? 0 : -1;
      panel.hidden = !selected;
      panel.classList.toggle("active", selected);
    });
  }
  tabs.forEach(function (tab, index) {
    tab.addEventListener("click", function () { selectTab(tab); });
    tab.addEventListener("keydown", function (event) {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      event.preventDefault();
      var next = (index + (event.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length;
      selectTab(tabs[next]);
      tabs[next].focus();
    });
  });

  render();
})();
