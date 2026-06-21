/* etchy feedback widget — the one shared widget (landing, docs, and the app demo).
 * Floating button → overlay: sentiment + category + (severity) + comment + name +
 * screenshots (paste/drag/browse) + auto context. Posts to the endpoint in its
 * <script data-feedback-endpoint>, tagged with data-feedback-key.
 *
 * Self-contained IIFE, no deps. Styled in the etchy copper brand (green/red stay
 * reserved for added/removed diffs, so they're not used for chrome here).
 * DEV/REVIEW today (localhost feedback server); repoint the endpoint to the hosted
 * one to collect feedback from anywhere (incl. teammates).
 */
(function () {
  "use strict";
  var script = document.currentScript;
  var ENDPOINT = (script && script.dataset.feedbackEndpoint) || "http://localhost:8765/submit";
  var FB_KEY = (script && script.dataset.feedbackKey) || "";

  var css = "\
  .efb-fab{position:fixed;right:20px;bottom:20px;z-index:2147483600;display:inline-flex;align-items:center;gap:.5rem;\
    background:linear-gradient(135deg,#f6c873,#b9762a);color:#1a1206;border:0;border-radius:999px;padding:.7rem 1.1rem;\
    font:700 14px/1 ui-monospace,'JetBrains Mono',monospace;cursor:pointer;box-shadow:0 8px 24px -8px #000}\
  .efb-fab:hover{filter:brightness(1.06)}\
  .efb-panel{position:fixed;right:20px;bottom:20px;z-index:2147483601;width:344px;max-width:calc(100vw - 40px);\
    background:#121817;color:#e7e3d6;border:1px solid #2b3a36;border-radius:14px;box-shadow:0 24px 60px -18px #000;\
    font:14px/1.5 ui-monospace,'JetBrains Mono',monospace;display:none;overflow:hidden}\
  .efb-panel.efb-open{display:block}\
  .efb-h{display:flex;align-items:center;padding:.8rem 1rem;border-bottom:1px solid #23302d}\
  .efb-h b{font:700 14px/1 'Zilla Slab',serif,ui-monospace;color:#f6c873}\
  .efb-x{margin-left:auto;background:none;border:0;color:#8b9491;font-size:19px;line-height:1;cursor:pointer}\
  .efb-x:hover{color:#e7e3d6}\
  .efb-body{padding:.9rem 1rem;max-height:min(74vh,470px);overflow:auto}\
  .efb-l{display:block;font-size:11px;letter-spacing:.07em;text-transform:uppercase;color:#b9762a;margin:2px 0 7px}\
  .efb-row{display:flex;gap:6px;margin-bottom:13px}\
  .efb-sent{flex:1;cursor:pointer;background:#0b0f0e;border:1px solid #23302d;border-radius:9px;color:#cdc9bd;padding:9px 4px;font:600 12.5px/1 inherit}\
  .efb-sent.on{border-color:#e8a33d;background:rgba(232,163,61,.16);color:#f6c873}\
  .efb-pills{display:flex;flex-wrap:wrap;gap:6px;margin-bottom:13px}\
  .efb-pill{font-size:12px;padding:5px 10px;border-radius:14px;border:1px solid #23302d;background:#0b0f0e;color:#e7e3d6;cursor:pointer}\
  .efb-pill.on{background:rgba(232,163,61,.16);border-color:#e8a33d;color:#f6c873}\
  .efb-s{flex:1;text-align:center;font-size:11.5px;padding:7px 0;border-radius:7px;border:1px solid #23302d;background:#0b0f0e;color:#8b9491;cursor:pointer}\
  .efb-s.on{border-color:#e8a33d;color:#f6c873;background:rgba(232,163,61,.12)}\
  .efb-text{width:100%;min-height:64px;background:#0b0f0e;color:#e7e3d6;border:1px solid #23302d;border-radius:8px;padding:9px 10px;font:13.5px/1.5 inherit;resize:vertical}\
  .efb-name{width:100%;background:#0b0f0e;color:#e7e3d6;border:1px solid #23302d;border-radius:8px;padding:8px 10px;font:13.5px inherit;margin-top:10px}\
  .efb-shot{margin-top:11px;border:1.5px dashed #2b3a36;border-radius:8px;background:#0e1413;color:#8b9491;font-size:12px;padding:10px;cursor:pointer;text-align:center}\
  .efb-shot:hover{border-color:#e8a33d;color:#e7e3d6}\
  .efb-shots{display:flex;flex-wrap:wrap;gap:6px;margin-top:11px}\
  .efb-thumb{position:relative;width:64px;height:64px;border-radius:6px;overflow:hidden;border:1px solid #23302d;background:#0b0f0e}\
  .efb-thumb img{width:100%;height:100%;object-fit:cover;display:block}\
  .efb-thumb .rm{position:absolute;top:1px;right:2px;width:17px;height:17px;border-radius:9px;background:rgba(0,0,0,.62);color:#ff5d73;font-weight:800;font-size:12px;line-height:16px;text-align:center;cursor:pointer}\
  .efb-ctx{margin-top:11px;font-size:11px;color:#5d6b66;border-top:1px dashed #23302d;padding-top:9px}\
  .efb-ctx b{color:#8b9491}\
  .efb-send{width:100%;margin-top:12px;border:0;border-radius:8px;background:linear-gradient(135deg,#f6c873,#b9762a);color:#1a1206;font:800 14px/1 inherit;padding:11px;cursor:pointer}\
  .efb-send:disabled{opacity:.6;cursor:default}\
  .efb-kbd{display:block;text-align:center;margin-top:8px;font-size:11px;color:#5d6b66}.efb-kbd b{color:#8b9491}\
  .efb-status{display:block;text-align:center;margin-top:8px;font-size:12px;color:#8b9491}.efb-status.ok{color:#46d18a}.efb-status.err{color:#ff5d73}";

  var styleEl = document.createElement("style");
  styleEl.textContent = css;
  document.head.appendChild(styleEl);

  var fab = document.createElement("button");
  fab.className = "efb-fab"; fab.type = "button"; fab.innerHTML = "💬 Feedback";

  var panel = document.createElement("div");
  panel.className = "efb-panel"; panel.setAttribute("role", "dialog"); panel.setAttribute("aria-label", "Send feedback");
  panel.innerHTML =
    '<div class="efb-h"><b>Send feedback</b><button class="efb-x" type="button" aria-label="Close">✕</button></div>' +
    '<div class="efb-body">' +
    '<div class="efb-row efb-sents">' +
      '<button type="button" class="efb-sent" data-v="love">💚 Love</button>' +
      '<button type="button" class="efb-sent on" data-v="issue">🛠 Needs work</button>' +
      '<button type="button" class="efb-sent" data-v="idea">💡 Idea</button></div>' +
    '<span class="efb-l">Which part?</span>' +
    '<div class="efb-pills">' +
      '<span class="efb-pill on">Layout &amp; sizing</span><span class="efb-pill">Colors &amp; contrast</span>' +
      '<span class="efb-pill">Content &amp; wording</span><span class="efb-pill">Clarity</span><span class="efb-pill">Other</span></div>' +
    '<div class="efb-sevrow"><span class="efb-l">How much does it bug you?</span>' +
      '<div class="efb-row efb-sev"><button type="button" class="efb-s">Minor</button><button type="button" class="efb-s on">Annoying</button><button type="button" class="efb-s">Blocking</button></div></div>' +
    '<span class="efb-l">Tell me more</span>' +
    '<textarea class="efb-text" placeholder="What\'s not working? e.g. the install tabs are hard to spot"></textarea>' +
    '<input class="efb-name" placeholder="Your name or initials (optional)">' +
    '<div class="efb-shots"></div>' +
    '<div class="efb-shot">📎 Paste screenshots (Ctrl+V) — or click to attach</div>' +
    '<input type="file" class="efb-file" accept="image/*" multiple hidden>' +
    '<div class="efb-ctx"></div>' +
    '<button type="button" class="efb-send">Send feedback</button>' +
    '<span class="efb-kbd"><b>Ctrl/⌘+Enter</b> to send · <b>Ctrl+V</b> to paste · <b>Esc</b> to close</span>' +
    '<span class="efb-status"></span>' +
    "</div>";

  document.body.appendChild(fab);
  document.body.appendChild(panel);

  var q = function (s) { return panel.querySelector(s); };
  var textarea = q(".efb-text"), nameInput = q(".efb-name"), drop = q(".efb-shot"),
      fileInput = q(".efb-file"), thumbs = q(".efb-shots"), sendBtn = q(".efb-send"),
      status = q(".efb-status"), ctxEl = q(".efb-ctx"), sevrow = q(".efb-sevrow");

  var NAME_KEY = "etchy_fb_name";
  nameInput.value = localStorage.getItem(NAME_KEY) || "";

  var sentiment = "issue", category = "Layout & sizing", severity = "Annoying", shots = [];
  var PH = { love: "What's working well? What do you like?", issue: "What's not working? e.g. the install tabs are hard to spot", idea: "What would you add or change?" };

  function ctx() {
    return { screen: screen.width + "x" + screen.height, viewport: innerWidth + "x" + innerHeight,
             dpr: Math.round((window.devicePixelRatio || 1) * 100) / 100, ua: navigator.userAgent,
             lang: navigator.language, url: location.href };
  }
  function open() {
    panel.classList.add("efb-open"); fab.style.display = "none";
    var c = ctx();
    ctxEl.innerHTML = "Sent automatically: <b>screen</b> " + c.screen + " · <b>window</b> " + c.viewport + " · <b>scale</b> " + c.dpr + "×";
    status.textContent = ""; status.className = "efb-status"; textarea.focus();
  }
  function close() { panel.classList.remove("efb-open"); fab.style.display = "inline-flex"; }

  fab.addEventListener("click", open);
  q(".efb-x").addEventListener("click", close);

  q(".efb-sents").addEventListener("click", function (e) {
    var b = e.target.closest(".efb-sent"); if (!b) return;
    [].forEach.call(this.children, function (x) { x.classList.remove("on"); });
    b.classList.add("on"); sentiment = b.getAttribute("data-v");
    textarea.placeholder = PH[sentiment];
    sevrow.style.display = sentiment === "issue" ? "block" : "none";
  });
  q(".efb-pills").addEventListener("click", function (e) {
    if (!e.target.classList.contains("efb-pill")) return;
    [].forEach.call(this.children, function (p) { p.classList.remove("on"); });
    e.target.classList.add("on"); category = e.target.textContent.trim();
  });
  q(".efb-sev").addEventListener("click", function (e) {
    var b = e.target.closest(".efb-s"); if (!b) return;
    [].forEach.call(this.children, function (p) { p.classList.remove("on"); });
    b.classList.add("on"); severity = b.textContent.trim();
  });

  function renderThumbs() {
    thumbs.innerHTML = "";
    shots.forEach(function (s, i) {
      var t = document.createElement("div"); t.className = "efb-thumb";
      var img = document.createElement("img"); img.src = s.data; img.alt = s.name || "screenshot";
      var rm = document.createElement("span"); rm.className = "rm"; rm.textContent = "✕"; rm.title = "remove";
      rm.onclick = function () { shots.splice(i, 1); renderThumbs(); };
      t.appendChild(img); t.appendChild(rm); thumbs.appendChild(t);
    });
    drop.textContent = shots.length ? ("📎 " + shots.length + " attached — paste or click to add more")
                                    : "📎 Paste screenshots (Ctrl+V) — or click to attach";
  }
  function takeFile(file) {
    if (!file || String(file.type).indexOf("image/") !== 0) return;
    var r = new FileReader();
    r.onload = function () { shots.push({ data: r.result, name: file.name || "pasted.png", type: file.type, size: file.size }); renderThumbs(); };
    r.readAsDataURL(file);
  }
  drop.addEventListener("click", function () { fileInput.click(); });
  fileInput.addEventListener("change", function () { [].forEach.call(this.files || [], takeFile); this.value = ""; });
  document.addEventListener("paste", function (e) {
    if (!panel.classList.contains("efb-open") || !e.clipboardData) return;
    var items = e.clipboardData.items || [], got = false;
    for (var i = 0; i < items.length; i++) {
      if (items[i].type && items[i].type.indexOf("image/") === 0) { takeFile(items[i].getAsFile()); got = true; }
    }
    if (got) e.preventDefault();
  });
  document.addEventListener("keydown", function (e) {
    if (!panel.classList.contains("efb-open")) return;
    if (e.key === "Escape") close();
    else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) { e.preventDefault(); submit(); }
  });
  sendBtn.addEventListener("click", submit);

  function submit() {
    var text = textarea.value.trim();
    if (!text) { textarea.focus(); textarea.style.borderColor = "#e8a33d"; return; }
    var name = nameInput.value.trim();
    if (name) localStorage.setItem(NAME_KEY, name);
    var payload = {
      key: FB_KEY, source: "etchy-widget",
      sentiment: sentiment, category: category, severity: sentiment === "issue" ? severity : null,
      comment: text, text: text, username: name || null, name: name || null,
      page: { url: location.href, title: document.title }, ctx: ctx(),
      attachments: shots.map(function (s) { return { name: s.name, type: s.type, size: s.size, data: s.data }; }),
      ts: new Date().toISOString()
    };
    sendBtn.disabled = true; sendBtn.textContent = "Sending…";
    status.textContent = ""; status.className = "efb-status";
    // text/plain + no-cors = CORS simple request: reaches the feedback server cross-origin
    // (e.g. :8765) with no preflight. Server reads the JSON body regardless of content-type.
    fetch(ENDPOINT, { method: "POST", mode: "no-cors", headers: { "Content-Type": "text/plain" }, body: JSON.stringify(payload) })
      .then(function () {
        status.textContent = "✓ Thanks — sent. I'll pick it up."; status.className = "efb-status ok";
        textarea.value = ""; textarea.style.borderColor = ""; shots = []; renderThumbs();
        setTimeout(close, 1400);
      })
      .catch(function () {
        status.textContent = "⚠ Couldn't reach the feedback server."; status.className = "efb-status err";
      })
      .then(function () { sendBtn.disabled = false; sendBtn.textContent = "Send feedback"; });
  }

  renderThumbs();
})();
