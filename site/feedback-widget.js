/* etchy feedback widget — DEV/REVIEW ONLY.
 * Floating button + overlay for leaving comments and screenshots while reviewing
 * the page locally. Submissions POST to the feedback-loop backend (default
 * localhost:8770/submit), which saves screenshots to uploads/, appends to
 * feedback.jsonl, and bumps the wake signal.
 *
 * REMOVE (or gate) before the site goes public — it targets localhost.
 * Self-contained IIFE: injects its own scoped CSS + DOM, no dependencies.
 */
(function () {
  "use strict";
  var script = document.currentScript;
  var ENDPOINT = (script && script.dataset.feedbackEndpoint) || "http://localhost:8770/submit";
  var NAME_KEY = "etchy_fb_name";

  // ---- scoped styles -------------------------------------------------------
  var css = "\
  .efb-btn{position:fixed;right:20px;bottom:20px;z-index:2147483600;display:inline-flex;align-items:center;gap:.5rem;\
    background:linear-gradient(135deg,#f6c873,#b9762a);color:#1a1206;border:0;border-radius:999px;padding:.7rem 1.1rem;\
    font:600 14px/1 ui-monospace,'JetBrains Mono',monospace;cursor:pointer;box-shadow:0 8px 24px -8px #000;}\
  .efb-btn:hover{filter:brightness(1.06);}\
  .efb-panel{position:fixed;right:20px;bottom:76px;z-index:2147483601;width:min(360px,calc(100vw - 40px));\
    background:#121817;color:#e7e3d6;border:1px solid #2b3a36;border-radius:14px;box-shadow:0 30px 60px -20px #000;\
    font:14px/1.5 ui-monospace,'JetBrains Mono',monospace;display:none;overflow:hidden;}\
  .efb-panel.efb-open{display:block;}\
  .efb-head{display:flex;align-items:center;justify-content:space-between;padding:.8rem 1rem;border-bottom:1px solid #23302d;}\
  .efb-head h3{margin:0;font:700 14px/1 'Zilla Slab',serif,ui-monospace;color:#f6c873;letter-spacing:.02em;}\
  .efb-x{background:none;border:0;color:#8b9491;font-size:18px;cursor:pointer;line-height:1;padding:.2rem;}\
  .efb-x:hover{color:#e7e3d6;}\
  .efb-body{padding:1rem;display:flex;flex-direction:column;gap:.7rem;}\
  .efb-label{font-size:11px;letter-spacing:.08em;text-transform:uppercase;color:#b9762a;}\
  .efb-body textarea,.efb-body input[type=text]{width:100%;box-sizing:border-box;background:#0b0f0e;color:#e7e3d6;\
    border:1px solid #23302d;border-radius:8px;padding:.55rem .6rem;font:inherit;}\
  .efb-body textarea{min-height:84px;resize:vertical;}\
  .efb-drop{border:1.5px dashed #2b3a36;border-radius:9px;padding:.7rem;color:#8b9491;font-size:12.5px;text-align:center;cursor:pointer;}\
  .efb-drop.efb-over{border-color:#e8a33d;background:rgba(232,163,61,.08);color:#e7e3d6;}\
  .efb-thumbs{list-style:none;margin:.2rem 0 0;padding:0;display:flex;flex-wrap:wrap;gap:.5rem;}\
  .efb-thumb{position:relative;width:54px;height:54px;border:1px solid #23302d;border-radius:7px;overflow:hidden;background:#0b0f0e;}\
  .efb-thumb img{width:100%;height:100%;object-fit:cover;display:block;}\
  .efb-thumb .efb-rm{position:absolute;top:-6px;right:-6px;width:18px;height:18px;border-radius:50%;border:0;\
    background:#ff5d73;color:#160d05;font-size:11px;line-height:18px;cursor:pointer;padding:0;}\
  .efb-foot{display:flex;align-items:center;gap:.7rem;}\
  .efb-submit{background:linear-gradient(135deg,#f6c873,#b9762a);color:#1a1206;border:0;border-radius:8px;\
    padding:.55rem 1.1rem;font:700 13px/1 inherit;cursor:pointer;}\
  .efb-submit:disabled{opacity:.5;cursor:default;}\
  .efb-status{font-size:12px;color:#8b9491;}\
  .efb-status.efb-ok{color:#46d18a;}.efb-status.efb-err{color:#ff5d73;}\
  .efb-hint{margin-left:auto;font-size:10.5px;color:#5d6b66;}";

  var styleEl = document.createElement("style");
  styleEl.textContent = css;
  document.head.appendChild(styleEl);

  // ---- DOM -----------------------------------------------------------------
  var btn = document.createElement("button");
  btn.className = "efb-btn";
  btn.type = "button";
  btn.setAttribute("aria-haspopup", "dialog");
  btn.innerHTML = "💬 Feedback";

  var panel = document.createElement("div");
  panel.className = "efb-panel";
  panel.setAttribute("role", "dialog");
  panel.setAttribute("aria-label", "Page feedback");
  panel.innerHTML =
    '<div class="efb-head"><h3>Page feedback</h3><button class="efb-x" type="button" aria-label="Close">✕</button></div>' +
    '<div class="efb-body">' +
    '<div><div class="efb-label">Comment / suggestion</div><textarea placeholder="What works, what doesn\'t, what to change…"></textarea></div>' +
    '<div><div class="efb-label">Your name</div><input type="text" class="efb-name" placeholder="who’s this from"></div>' +
    '<div><div class="efb-label">Screenshots</div><div class="efb-drop" tabindex="0">Paste (Ctrl+V), drag images here, or <u>browse</u><input type="file" accept="image/*" multiple hidden></div><ul class="efb-thumbs"></ul></div>' +
    '<div class="efb-foot"><button class="efb-submit" type="button" disabled>Send</button><span class="efb-status"></span><span class="efb-hint">⌘/Ctrl+Enter · Esc</span></div>' +
    "</div>";

  document.body.appendChild(btn);
  document.body.appendChild(panel);

  var textarea = panel.querySelector("textarea");
  var nameInput = panel.querySelector(".efb-name");
  var drop = panel.querySelector(".efb-drop");
  var fileInput = panel.querySelector("input[type=file]");
  var thumbs = panel.querySelector(".efb-thumbs");
  var submitBtn = panel.querySelector(".efb-submit");
  var status = panel.querySelector(".efb-status");
  var closeBtn = panel.querySelector(".efb-x");

  nameInput.value = localStorage.getItem(NAME_KEY) || "";

  var picked = []; // {file, dataUrl}

  // ---- helpers -------------------------------------------------------------
  function refreshSubmit() {
    submitBtn.disabled = textarea.value.trim() === "";
  }
  function renderThumbs() {
    thumbs.innerHTML = "";
    picked.forEach(function (p, i) {
      var li = document.createElement("li");
      li.className = "efb-thumb";
      var img = document.createElement("img");
      img.src = p.dataUrl;
      img.alt = p.file.name || "screenshot";
      var rm = document.createElement("button");
      rm.className = "efb-rm";
      rm.type = "button";
      rm.textContent = "✕";
      rm.title = "Remove";
      rm.onclick = function () { picked.splice(i, 1); renderThumbs(); };
      li.appendChild(img);
      li.appendChild(rm);
      thumbs.appendChild(li);
    });
  }
  function addFiles(list) {
    Array.prototype.forEach.call(list, function (file) {
      if (!file || file.type.indexOf("image/") !== 0) return;
      var r = new FileReader();
      r.onload = function () { picked.push({ file: file, dataUrl: r.result }); renderThumbs(); };
      r.readAsDataURL(file);
    });
  }
  function open() {
    panel.classList.add("efb-open");
    status.textContent = ""; status.className = "efb-status";
    textarea.focus();
  }
  function close() { panel.classList.remove("efb-open"); }

  // ---- events --------------------------------------------------------------
  btn.addEventListener("click", function () {
    panel.classList.contains("efb-open") ? close() : open();
  });
  closeBtn.addEventListener("click", close);
  textarea.addEventListener("input", refreshSubmit);

  drop.addEventListener("click", function (e) { if (e.target.tagName !== "INPUT") fileInput.click(); });
  drop.addEventListener("keydown", function (e) { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); fileInput.click(); } });
  fileInput.addEventListener("change", function () { addFiles(fileInput.files); fileInput.value = ""; });
  ["dragenter", "dragover"].forEach(function (ev) {
    drop.addEventListener(ev, function (e) { e.preventDefault(); drop.classList.add("efb-over"); });
  });
  ["dragleave", "drop"].forEach(function (ev) {
    drop.addEventListener(ev, function (e) { e.preventDefault(); drop.classList.remove("efb-over"); });
  });
  drop.addEventListener("drop", function (e) { if (e.dataTransfer) addFiles(e.dataTransfer.files); });

  // paste images anywhere while the panel is open
  document.addEventListener("paste", function (e) {
    if (!panel.classList.contains("efb-open") || !e.clipboardData) return;
    var imgs = [];
    Array.prototype.forEach.call(e.clipboardData.items || [], function (it) {
      if (it.kind === "file" && it.type.indexOf("image/") === 0) imgs.push(it.getAsFile());
    });
    if (imgs.length) { e.preventDefault(); addFiles(imgs); }
  });

  // keyboard: Esc closes, Cmd/Ctrl+Enter submits
  document.addEventListener("keydown", function (e) {
    if (!panel.classList.contains("efb-open")) return;
    if (e.key === "Escape") { close(); }
    else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) { e.preventDefault(); submit(); }
  });

  submitBtn.addEventListener("click", submit);

  function submit() {
    if (submitBtn.disabled) return;
    var name = nameInput.value.trim();
    if (name) localStorage.setItem(NAME_KEY, name);
    var payload = {
      source: "etchy-widget",
      page: { url: location.href, title: document.title },
      username: name || null,
      comment: textarea.value.trim(),
      attachments: picked.map(function (p) {
        return { name: p.file.name || "screenshot.png", type: p.file.type, size: p.file.size, data: p.dataUrl };
      }),
      viewport: { w: window.innerWidth, h: window.innerHeight },
      ts: new Date().toISOString()
    };
    submitBtn.disabled = true;
    status.textContent = "Sending…"; status.className = "efb-status";
    // text/plain + no-cors = CORS "simple request": crosses :8000 -> :8770 with no
    // preflight and no change to the shared server. Response is opaque (fire-and-forget).
    fetch(ENDPOINT, {
      method: "POST", mode: "no-cors",
      headers: { "Content-Type": "text/plain" },
      body: JSON.stringify(payload)
    }).then(function () {
      status.textContent = "Saved ✓ — thanks, I’ll pick it up.";
      status.className = "efb-status efb-ok";
      textarea.value = ""; picked = []; renderThumbs(); refreshSubmit();
      setTimeout(close, 1400);
    }).catch(function () {
      status.textContent = "Couldn’t reach the feedback server (running on :8770?)";
      status.className = "efb-status efb-err";
      submitBtn.disabled = false;
    });
  }

  refreshSubmit();
})();
