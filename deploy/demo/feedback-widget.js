(function(){
  var sentiment='issue', category='Layout & sizing', severity='Annoying', shots=[];
  var ph={love:"What's working well? What do you like?",
          issue:"What's not working? e.g. the layer buttons up top are too small",
          idea:"What would you add or change?"};
  function el(id){return document.getElementById(id);}
  function ctx(){var d=window.devicePixelRatio||1;var s={
    screen:screen.width+'x'+screen.height, viewport:innerWidth+'x'+innerHeight,
    dpr:Math.round(d*100)/100, lang:navigator.language, url:location.href};
    try{ if(window.__etchyState) s.app=window.__etchyState; }catch(e){}
    return s;}
  window.etfbHideHint=function(){var h=el('etfb-hint');if(h)h.classList.remove('show');try{localStorage.setItem('etfb-hint-seen','1');}catch(e){}};
  window.etfbOpen=function(){etfbHideHint();el('etfb-panel').classList.add('open');el('etfb-fab').style.display='none';
    var c=ctx();var extra=c.app?(' · <b>view</b> '+(c.app.layer||'?')+' · <b>zoom</b> '+(c.app.zoomPct||'?')+'%'):'';
    el('etfb-ctx').innerHTML='Sent automatically: <b>screen</b> '+c.screen+' · <b>window</b> '+c.viewport+' · <b>scale</b> '+c.dpr+'×'+extra;};
  window.etfbClose=function(){el('etfb-panel').classList.remove('open');el('etfb-fab').style.display='flex';};
  [].forEach.call(document.querySelectorAll('.etfb-sent'),function(b){b.addEventListener('click',function(){
    [].forEach.call(document.querySelectorAll('.etfb-sent'),function(x){x.classList.remove('on');});b.classList.add('on');
    sentiment=b.getAttribute('data-v'); el('etfb-text').placeholder=ph[sentiment];
    el('etfb-sevrow').style.display=(sentiment==='issue')?'block':'none';});});
  el('etfb-pills').addEventListener('click',function(e){if(e.target.classList.contains('etfb-pill')){
    [].forEach.call(this.children,function(p){p.classList.remove('on');});e.target.classList.add('on');category=e.target.textContent.trim();}});
  el('etfb-sev').addEventListener('click',function(e){if(e.target.classList.contains('etfb-s')){
    [].forEach.call(this.children,function(p){p.classList.remove('on');});e.target.classList.add('on');severity=e.target.textContent.trim();}});
  // --- screenshots attach: paste, drop, or browse; multiple ---
  function renderShots(){var g=el('etfb-shots');g.innerHTML='';
    shots.forEach(function(s,i){var t=document.createElement('div');t.className='etfb-thumb';
      t.innerHTML='<span class="rm" title="remove">×</span><img>';
      var img=t.querySelector('img');img.alt=s.name||'screenshot';img.src=s.data;
      t.querySelector('.rm').onclick=function(){shots.splice(i,1);renderShots();};
      g.appendChild(t);});
    el('etfb-shot').textContent=shots.length
      ? ('📎 '+shots.length+' attached — paste, drop, or click to add more')
      : '📎 Paste screenshots (Ctrl+V), drop, or click to attach';}
  function takeFile(file){if(!file||String(file.type).indexOf('image/')!==0)return;
    var r=new FileReader();r.onload=function(){shots.push({data:r.result,name:file.name||'pasted.png',type:file.type});renderShots();};
    r.readAsDataURL(file);}
  el('etfb-shot').addEventListener('click',function(){el('etfb-file').click();});
  el('etfb-file').addEventListener('change',function(){[].forEach.call(this.files||[],takeFile);this.value='';});
  el('etfb-shot').addEventListener('dragover',function(e){e.preventDefault();});
  el('etfb-shot').addEventListener('drop',function(e){
    e.preventDefault();[].forEach.call((e.dataTransfer||{}).files||[],takeFile);});
  document.addEventListener('paste',function(e){
    if(!el('etfb-panel').classList.contains('open'))return;
    var items=((e.clipboardData||{}).items)||[],got=false;
    for(var i=0;i<items.length;i++){if(items[i].type&&items[i].type.indexOf('image/')===0){takeFile(items[i].getAsFile());got=true;}}
    if(got)e.preventDefault();});
  el('etfb-panel').addEventListener('keydown',function(e){
    if((e.ctrlKey||e.metaKey)&&(e.key==='Enter'||e.keyCode===13)){e.preventDefault();etfbSend();}});
  function done(ok){shots=[];renderShots();etfbClose();el('etfb-text').value='';el('etfb-name').value='';el('etfb-text').style.borderColor='';
    var t=el('etfb-toast');t.textContent=ok?'✓ Thanks — sent!':"⚠ Couldn't send — please open a GitHub issue instead";
    t.style.borderColor=ok?'#36d39a':'#e8b34a';t.style.color=ok?'#bff5e0':'#f0d49a';t.style.background=ok?'#10331f':'#33270f';
    t.classList.add('show');setTimeout(function(){t.classList.remove('show');},2800);}
  // Capture the live canvas as a PNG data-URI (#27). Returns null if the canvas
  // can't be read (e.g. tainted context). Done at SEND time so it reflects the
  // current pan/zoom, not a frozen view from when the panel opened.
  function captureCanvas(){try{var c=el('the_canvas_id');
    return (c&&c.toDataURL)?c.toDataURL('image/png'):null;}catch(e){return null;}}
  window.etfbSend=function(){var text=el('etfb-text').value.trim();
    if(!text){el('etfb-text').focus();el('etfb-text').style.borderColor='#e8b34a';return;}
    // If the user hasn't manually attached a screenshot, grab the canvas fresh
    // now so the attached image matches what they're currently looking at (#27).
    var sent=shots.map(function(s){return {data:s.data,name:s.name};});
    if(!sent.length){var live=captureCanvas();if(live)sent.push({data:live,name:'canvas.png'});}
    var payload={sentiment:sentiment,category:category,severity:(sentiment==='issue'?severity:null),
      text:text,name:el('etfb-name').value.trim(),ctx:ctx(),
      screenshots:sent};
    var b=el('etfb-send');b.disabled=true;b.textContent='Sending…';
    fetch('/feedback',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(payload)})
      .then(function(r){if(!r.ok)throw 0;done(true);}).catch(function(){done(false);})
      .then(function(){b.disabled=false;b.textContent='Send feedback';});};
  // --- opt-in: open a pre-filled GitHub issue in a new tab (#68) ---
  // Tokenless: builds a github.com/.../issues/new?title=&labels=&body= URL from
  // the current widget state. The user chooses to click it, so publishing is an
  // explicit action — a private board simply never gets opened here, and the body
  // carries an explicit "don't attach confidential screenshots" note. A token +
  // image upload would need a backend and is unsafe to embed in a public page, so
  // screenshots are pasted by the user after the issue form opens.
  var GH_REPO='Cimos/etchy';
  function ghLabels(){var ls=['feedback'];
    if(sentiment==='issue')ls.push('bug'); else if(sentiment==='idea')ls.push('enhancement');
    return ls.join(',');}
  function ghBody(text){var c=ctx(),app=c.app||{},L=[];
    L.push(text,'','---',
      '- **Sentiment:** '+sentiment,
      '- **Category:** '+category);
    if(sentiment==='issue')L.push('- **Severity:** '+severity);
    L.push('- **From:** '+(el('etfb-name').value.trim()||'anon'),
      '','**App context**',
      '- View / layer: '+(app.layer||'—'),
      '- Zoom: '+(app.zoomPct!=null?app.zoomPct+'%':'—'));
    if(app.mode)L.push('- Mode: '+app.mode);
    L.push('','**Environment**',
      '- Screen '+c.screen+' · Window '+c.viewport+' · Scale '+c.dpr+'×',
      '- URL: '+c.url,
      '','_Screenshot: paste it here if the board is public. Never attach images of confidential boards._');
    return L.join('\n');}
  function ghUrl(text){return 'https://github.com/'+GH_REPO+'/issues/new'
    +'?title='+encodeURIComponent(ghTitle(text))
    +'&labels='+encodeURIComponent(ghLabels())
    +'&body='+encodeURIComponent(ghBody(text));}
  function ghTitle(text){var t=text.replace(/\s+/g,' ').trim().slice(0,70);
    return text.length>70?t.replace(/\s+\S*$/,'')+'…':t;}
  // Exposed so the headless test can assert the generated URL without a real tab.
  window.__etfbGhUrl=ghUrl;
  window.etfbGithub=function(){var raw=el('etfb-text').value.trim();
    if(!raw){el('etfb-text').focus();el('etfb-text').style.borderColor='#e8b34a';return;}
    // Keep the URL well under browser/GitHub limits (~8KB): budget the free text
    // against the fixed overhead so the link never breaks; never raw-slice the URL
    // (that would cut a %-escape). The user pastes any overflow into the issue.
    var overhead=ghUrl('').length, text=raw;
    var budget=Math.floor((6000-overhead)/3); // ~3 bytes/char worst-case encoding
    if(budget>0&&text.length>budget)
      text=text.slice(0,Math.max(0,budget-40))+'\n\n…(truncated — paste the rest here)';
    window.open(ghUrl(text),'_blank','noopener');};
  // one-time hint bubble
  try{ if(!localStorage.getItem('etfb-hint-seen')){
    setTimeout(function(){var h=el('etfb-hint');if(h)h.classList.add('show');},2200);
    setTimeout(function(){var h=el('etfb-hint');if(h)h.classList.remove('show');},13000);
  }}catch(e){}
})();
