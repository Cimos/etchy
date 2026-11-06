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
  // Same limits as the feedback server (deploy/demo/etchy-server.py MAX_*):
  // anything over them is a 413, so refuse it here and say why instead.
  var MAX_SHOTS=8, MAX_SHOT_BYTES=8*1024*1024, MAX_BODY=16*1024*1024;
  // Room kept for the text, name and context when budgeting attachments.
  var BODY_RESERVE=256*1024;
  var pending=0, pendingBytes=0, epoch=0;
  function mb(n){return (n/1048576).toFixed(1).replace(/\.0$/,'')+' MB';}
  function names(a){return a.length>3?a.slice(0,3).join(', ')+' and '+(a.length-3)+' more':a.join(', ');}
  function urlBytes(type,size){return ('data:'+type+';base64,').length+Math.ceil(size/3)*4+16;}
  function shotBytes(){var n=pendingBytes;shots.forEach(function(s){n+=s.data.length+(s.name||'').length+32;});return n;}
  function showErr(msg){var e=el('etfb-err');e.textContent=msg||'';e.classList.toggle('show',!!msg);}
  function renderShots(){var g=el('etfb-shots');g.innerHTML='';
    shots.forEach(function(s,i){var t=document.createElement('div');t.className='etfb-thumb';
      t.innerHTML='<span class="rm" title="remove">×</span><img>';
      var img=t.querySelector('img');img.alt=s.name||'screenshot';img.src=s.data;
      t.querySelector('.rm').onclick=function(){shots.splice(i,1);showErr('');renderShots();};
      g.appendChild(t);});
    el('etfb-shot').textContent=shots.length
      ? ('📎 '+shots.length+' attached (max '+MAX_SHOTS+') — paste, drop, or click to add more')
      : '📎 Paste screenshots (Ctrl+V), drop, or click to attach';}
  // Checks every file against the caps before reading it (reserving its slot
  // and bytes so parallel reads can't overshoot), then reports all refusals in
  // one visible message. Nothing is dropped without saying so.
  function takeFiles(files){var many=[],big=[],total=[],notImg=[],bad=[],my=epoch;
    [].forEach.call(files||[],function(file){if(!file)return;
      var name=file.name||'pasted image';
      if(String(file.type).indexOf('image/')!==0){notImg.push(name);return;}
      if(file.size>MAX_SHOT_BYTES){big.push(name+' ('+mb(file.size)+')');return;}
      if(shots.length+pending>=MAX_SHOTS){many.push(name);return;}
      var est=urlBytes(file.type,file.size)+name.length+32;
      if(shotBytes()+est>MAX_BODY-BODY_RESERVE){total.push(name);return;}
      pending++;pendingBytes+=est;
      var r=new FileReader();
      function settle(){if(my!==epoch)return false;pending--;pendingBytes-=est;return true;}
      r.onload=function(){if(!settle())return;
        shots.push({data:r.result,name:file.name||'pasted.png',type:file.type});renderShots();};
      r.onerror=function(){if(!settle())return;showErr("⚠ Couldn't read "+name+' — not attached.');};
      r.readAsDataURL(file);});
    var m=[];
    if(big.length)m.push('Image too large (max '+mb(MAX_SHOT_BYTES)+' each) — not attached: '+names(big)+'.');
    if(many.length)m.push('Max '+MAX_SHOTS+' screenshots — not attached: '+names(many)+'.');
    if(total.length)m.push('Screenshots over the '+mb(MAX_BODY)+' total limit — not attached: '+names(total)+'. Remove one to make room.');
    if(notImg.length)m.push('Not an image — not attached: '+names(notImg)+'.');
    showErr(m.length?'⚠ '+m.join(' '):'');}
  el('etfb-shot').addEventListener('click',function(){el('etfb-file').click();});
  el('etfb-file').addEventListener('change',function(){takeFiles(this.files);this.value='';});
  el('etfb-shot').addEventListener('dragover',function(e){e.preventDefault();});
  el('etfb-shot').addEventListener('drop',function(e){
    e.preventDefault();takeFiles((e.dataTransfer||{}).files);});
  document.addEventListener('paste',function(e){
    if(!el('etfb-panel').classList.contains('open'))return;
    var items=((e.clipboardData||{}).items)||[],got=[];
    for(var i=0;i<items.length;i++){if(items[i].type&&items[i].type.indexOf('image/')===0)got.push(items[i].getAsFile());}
    if(got.length){e.preventDefault();takeFiles(got);}});
  el('etfb-panel').addEventListener('keydown',function(e){
    if((e.ctrlKey||e.metaKey)&&(e.key==='Enter'||e.keyCode===13)){e.preventDefault();etfbSend();}});
  function done(ok,note){epoch++;pending=0;pendingBytes=0;showErr('');
    shots=[];renderShots();etfbClose();el('etfb-text').value='';el('etfb-name').value='';el('etfb-text').style.borderColor='';
    var t=el('etfb-toast');t.textContent=ok?'✓ Thanks — sent!'+(note?' ('+note+')':''):"⚠ Couldn't send — please open a GitHub issue instead";
    t.style.borderColor=ok?'#36d39a':'#e8b34a';t.style.color=ok?'#bff5e0':'#f0d49a';t.style.background=ok?'#10331f':'#33270f';
    t.classList.add('show');setTimeout(function(){t.classList.remove('show');},2800);}
  // Capture the live canvas as a PNG data-URI (#27). Returns null if the canvas
  // can't be read (e.g. tainted context). Done at SEND time so it reflects the
  // current pan/zoom, not a frozen view from when the panel opened.
  function captureCanvas(){try{var c=el('the_canvas_id');
    return (c&&c.toDataURL)?c.toDataURL('image/png'):null;}catch(e){return null;}}
  function decodedBytes(durl){var b64=String(durl).slice(String(durl).indexOf(',')+1).replace(/\s+/g,''),
    pad=(b64.slice(-2)==='=='?2:(b64.slice(-1)==='='?1:0));return Math.floor(b64.length*3/4)-pad;}
  window.etfbSend=function(){var text=el('etfb-text').value.trim();
    if(!text){el('etfb-text').focus();el('etfb-text').style.borderColor='#e8b34a';return;}
    if(pending){showErr('⚠ Still attaching screenshots — try again in a moment.');return;}
    // If the user hasn't manually attached a screenshot, grab the canvas fresh
    // now so the attached image matches what they're currently looking at (#27).
    var sent=shots.map(function(s){return {data:s.data,name:s.name};}),note='';
    function build(){return JSON.stringify({sentiment:sentiment,category:category,severity:(sentiment==='issue'?severity:null),
      text:text,name:el('etfb-name').value.trim(),ctx:ctx(),
      screenshots:sent});}
    function size(s){return new Blob([s]).size;}
    if(!sent.length){var live=captureCanvas();
      if(live&&decodedBytes(live)<=MAX_SHOT_BYTES){sent.push({data:live,name:'canvas.png'});
        if(size(build())>MAX_BODY)sent=[];}
      if(live&&!sent.length)note='the current view was too large to attach';}
    var body=build(),n=size(body);
    if(n>MAX_BODY){showErr('⚠ Feedback too large to send ('+mb(n)+', max '+mb(MAX_BODY)+')'
      +(shots.length?' — remove a screenshot or shorten the text.':' — shorten the text.'));return;}
    showErr('');
    var b=el('etfb-send');b.disabled=true;b.textContent='Sending…';
    fetch('/feedback',{method:'POST',headers:{'Content-Type':'application/json'},body:body})
      .then(function(r){if(!r.ok)throw 0;done(true,note);}).catch(function(){done(false);})
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
