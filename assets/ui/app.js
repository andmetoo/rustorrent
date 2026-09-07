const themeKey='rustorrent-theme';
const filterKey='rustorrent-library-filter';
const searchKey='rustorrent-library-search';
const mainTabKey='rustorrent-main-tab';
const searchPluginSelectionKey='rustorrent-search-plugins';
const searchCategoryKey='rustorrent-search-category';
const searchSortStorageKey='rustorrent-search-sort';
const collapsePrefix='rustorrent-collapse:';
const panelCollapsePrefix='rustorrent-panel:';
let pendingHtml=null;
let activeTheme=null;
let queuedLiveHtml=null;
let liveRenderTimer=null;
let searchStateCache=null;
let searchCatalogCache=[];
let searchPollTimer=null;
let searchPollActive=false;
let searchCatalogLoading=false;
let searchCatalogFetchedAt=0;
let activeSearchPanelView='results';
const addedSearchResults=new Set();
let lastSearchStartedAt=0;
let activeSearchSort={key:'seeds',dir:'desc'};
const liveRenderIntervalMs=450;
const MAX_RATE_LIMIT_KBPS=102400;
const apiTokenMeta=document.querySelector('meta[name="rustorrent-api-token"]');
let apiToken=apiTokenMeta?String(apiTokenMeta.getAttribute('content')||'').trim():'';
function syncApiTokenMeta(){
  if(!apiTokenMeta){return;}
  const value=String(apiTokenMeta.getAttribute('content')||'').trim();
  if(value){apiToken=value;}
}
async function refreshApiToken(){
  try{
    const res=await fetch('/api-token',{cache:'no-store',headers:{'Accept':'application/json'}});
    if(!res.ok){return false;}
    const data=await res.json();
    const token=data&&typeof data.token==='string'?data.token.trim():'';
    if(!token){return false;}
    apiToken=token;
    if(apiTokenMeta){apiTokenMeta.setAttribute('content',token);}
    return true;
  }catch(e){
    return false;
  }
}
syncApiTokenMeta();
function resolveTheme(){
  let theme='light';
  try{
    const stored=localStorage.getItem(themeKey);
    if(stored==='light'||stored==='dark'){theme=stored;}
  }catch(e){}
  return theme;
}
function applyTheme(theme){
  const root=document.documentElement;
  if(activeTheme!==theme){
    root.classList.add('theme-switching');
    root.setAttribute('data-theme',theme);
    activeTheme=theme;
    requestAnimationFrame(()=>requestAnimationFrame(()=>root.classList.remove('theme-switching')));
  }
  const themeBtn=document.getElementById('themeToggle');
  if(themeBtn){themeBtn.innerHTML='<svg class="material-symbols-rounded"><use href=#i-'+(theme==='light'?'light_mode':'dark_mode')+'></use></svg>';}
}
function resolveMainTab(){
  try{
    const value=localStorage.getItem(mainTabKey)||'library';
    if(value==='search'){return 'search';}
  }catch(e){}
  return 'library';
}
function applyMainTab(tab){
  const next=tab==='search'?'search':'library';
  const tabs=Array.from(document.querySelectorAll('[data-main-tab-target]'));
  tabs.forEach(btn=>{const active=btn.dataset.mainTabTarget===next;btn.classList.toggle('active',active);btn.setAttribute('aria-pressed',String(active));});
  const workspaces=Array.from(document.querySelectorAll('.workspace[data-main-tab]'));
  workspaces.forEach(view=>view.classList.toggle('active',view.dataset.mainTab===next));
  if(next==='search'&&searchCatalogCache.length===0&&!searchCatalogLoading){
    loadSearchCatalog(true).catch(err=>console.warn('search catalog failed',err));
  }
}
function resolveSearchPanelView(){return activeSearchPanelView;}
function applySearchPanelView(view){
  const next=view==='plugins'?'plugins':'results';
  activeSearchPanelView=next;
  const panels=Array.from(document.querySelectorAll('[data-search-view]'));
  panels.forEach(panel=>panel.classList.toggle('active',panel.dataset.searchView===next));
  const toggles=Array.from(document.querySelectorAll('[data-search-view-target]'));
  toggles.forEach(btn=>btn.classList.toggle('active',btn.dataset.searchViewTarget===next));
  if(next==='plugins'&&searchCatalogCache.length===0&&!searchCatalogLoading){
    loadSearchCatalog(true).catch(err=>console.warn('search catalog failed',err));
  }
}
function resolveFilter(){
  let filter='all';
  try{const stored=localStorage.getItem(filterKey);if(stored){filter=stored;}}catch(e){}
  return filter;
}
function resolveSearch(){
  try{return localStorage.getItem(searchKey)||'';}catch(e){}
  return '';
}
function applyFilter(filter){
  const searchRaw=resolveSearch();
  const search=searchRaw.trim().toLowerCase();
  const searchInput=document.getElementById('librarySearch');
  if(searchInput&&searchInput.value!==searchRaw){searchInput.value=searchRaw;}
  const buttons=Array.from(document.querySelectorAll('.nav-item[data-filter]'));
  buttons.forEach(btn=>{const active=btn.dataset.filter===filter;btn.classList.toggle('active',active);});
  const cards=Array.from(document.querySelectorAll('.torrent-card'));
  cards.forEach(card=>{
    const status=card.dataset.status||'downloading';
    const name=(card.dataset.name||'').toLowerCase();
    const cardLabel=card.dataset.label||'';
    let matchesFilter;
    if(filter==='all'){matchesFilter=true;}
    else if(filter.startsWith('label:')){matchesFilter=cardLabel===filter.slice(6);}
    else{matchesFilter=status===filter;}
    const matchesSearch=!search||name.includes(search);
    if(matchesFilter&&matchesSearch){card.style.display='';}else{card.style.display='none';}
  });
  const empty=document.getElementById('filterEmpty');
  if(empty){empty.hidden=cards.length===0||cards.some(card=>card.style.display!=='none');}
  buttons.forEach(btn=>btn.setAttribute('aria-pressed',String(btn.dataset.filter===filter)));
}
function clearLibraryFilters(){
  try{localStorage.removeItem(searchKey);localStorage.removeItem(filterKey);}catch(e){}
  applyFilter('all');
}
function collapseKey(hash){return collapsePrefix+hash;}
function panelCollapseKey(name){return panelCollapsePrefix+name;}
function defaultPanelCollapsed(name){return name==='transfer';}
function isPanelCollapsed(name){
  try{
    const stored=localStorage.getItem(panelCollapseKey(name));
    if(stored==='1'){return true;}
    if(stored==='0'){return false;}
  }catch(e){}
  return defaultPanelCollapsed(name);
}
function applyCollapseState(){
  const cards=Array.from(document.querySelectorAll('.torrent-card'));
  cards.forEach(card=>{
    const hash=card.dataset.infoHash||'';
    let collapsed=true;
    try{
      const stored=localStorage.getItem(collapseKey(hash));
      if(stored==='0'){collapsed=false;}
      if(stored==='1'){collapsed=true;}
    }catch(e){}
    card.dataset.collapsed=collapsed?'true':'false';
    const toggle=card.querySelector("[data-action='toggle-expand']");
    if(toggle){toggle.innerHTML=collapsed?'<svg class="material-symbols-rounded"><use href=#i-unfold_more></use></svg>Expand':'<svg class="material-symbols-rounded"><use href=#i-unfold_less></use></svg>Collapse';toggle.setAttribute('aria-expanded',collapsed?'false':'true');}
  });
}
function applyPanelState(){
  const panels=Array.from(document.querySelectorAll('[data-panel]'));
  panels.forEach(panel=>{
    const name=panel.dataset.panel||'';
    const collapsed=isPanelCollapsed(name);
    panel.dataset.collapsed=collapsed?'true':'false';
    const toggle=panel.querySelector('[data-action="toggle-panel"]');
    if(toggle){
      toggle.innerHTML=collapsed?'<svg class="material-symbols-rounded"><use href=#i-unfold_more></use></svg>Expand':'<svg class="material-symbols-rounded"><use href=#i-unfold_less></use></svg>Collapse';
      toggle.setAttribute('aria-expanded',collapsed?'false':'true');
    }
  });
}
function syncAttributes(current,next){
  const isModal=current.id==='addModal';
  const hadOpen=isModal&&current.classList.contains('open');
  const isDz=current.id==='dropZone';
  const dzHasFile=isDz&&current.classList.contains('has-file');
  const toRemove=[];
  for(const attr of Array.from(current.attributes||[])){
    if(attr.name==='open'&&current.tagName==='DETAILS'){continue;}
    if(!next.hasAttribute(attr.name)){toRemove.push(attr.name);}
  }
  toRemove.forEach(name=>current.removeAttribute(name));
  for(const attr of Array.from(next.attributes||[])){
    if(current.getAttribute(attr.name)!==attr.value){current.setAttribute(attr.name,attr.value);}
  }
  if(hadOpen){current.classList.add('open');}
  if(dzHasFile){current.classList.add('has-file');}
}
function syncElementValue(current,next){
  if(current===document.activeElement){return;}
  const tag=current.tagName;
  if(tag==='INPUT'){
    const type=String(current.type||'').toLowerCase();
    if(type==='checkbox'||type==='radio'){
      if(current.checked!==next.checked){current.checked=next.checked;}
    }else if(current.value!==next.value){
      current.value=next.value;
    }
    return;
  }
  if(tag==='TEXTAREA'){
    if(current.value!==next.value){current.value=next.value;}
    return;
  }
  if(tag==='SELECT'){
    if(current.value!==next.value){current.value=next.value;}
  }
}
function morphNode(current,next){
  if(!current||!next){return;}
  if(current.nodeType!==next.nodeType||current.nodeName!==next.nodeName){
    current.replaceWith(next.cloneNode(true));
    return;
  }
  if(current.nodeType===Node.TEXT_NODE||current.nodeType===Node.COMMENT_NODE){
    if(current.nodeValue!==next.nodeValue){current.nodeValue=next.nodeValue;}
    return;
  }
  if(current.classList?.contains('file-cell')&&current.contains(document.activeElement)){return;}
  syncAttributes(current,next);
  syncElementValue(current,next);
  morphChildren(current,next);
}
function morphChildren(current,next){
  const nextChildren=Array.from(next.childNodes);
  const key=node=>node.nodeType===1?(node.id|| (node.matches('.torrent-card')?'torrent:'+node.dataset.id:'')):'';
  for(let i=0;i<nextChildren.length;i+=1){
    let curr=current.childNodes[i];
    const nxt=nextChildren[i];
    const nextKey=key(nxt);
    if(nextKey&&(!curr||key(curr)!==nextKey)){
      const match=Array.from(current.childNodes).find(node=>key(node)===nextKey);
      if(match){current.insertBefore(match,curr||null);curr=match;}
      else{current.insertBefore(nxt.cloneNode(true),curr||null);continue;}
    }
    if(!curr&&nxt){
      current.appendChild(nxt.cloneNode(true));
      continue;
    }
    morphNode(curr,nxt);
  }
  while(current.childNodes.length>nextChildren.length){current.lastChild.remove();}
}
function renderApp(html){
  const root=document.getElementById('appRoot');
  if(!root){return;}
  if(!root.firstChild){
    root.innerHTML=html;
  }else{
    const tpl=document.createElement('template');
    tpl.innerHTML=html;
    morphChildren(root,tpl.content);
  }
  applyTheme(activeTheme||resolveTheme());
  applyMainTab(resolveMainTab());
  applySearchPanelView(resolveSearchPanelView());
  applyFilter(resolveFilter());
  applyCollapseState();
  applyPanelState();
  updateRateLimitLabels();
  if(searchStateCache){renderSearchStatus(searchStateCache);}
  if(searchCatalogCache.length>0){renderSearchCatalog(searchCatalogCache);}
  enhanceUI();
}
function scheduleLiveRender(html){
  queuedLiveHtml=html;
  if(liveRenderTimer!==null){return;}
  liveRenderTimer=setTimeout(()=>{
    liveRenderTimer=null;
    const nextHtml=queuedLiveHtml;
    queuedLiveHtml=null;
    if(nextHtml){
      if(isAddModalOpen()){pendingHtml=nextHtml;return;}
      renderApp(nextHtml);
    }
  },liveRenderIntervalMs);
}
function isAddModalOpen(){
  const modal=document.getElementById('addModal');
  return !!(modal&&modal.classList.contains('open'))||!!document.getElementById('removeDialog')?.open;
}
function applyUpdate(html){
  if(isAddModalOpen()){pendingHtml=html;return;}
  scheduleLiveRender(html);
  pendingHtml=null;
}
let addParseToken=0;
let addDraft={kind:'none',name:'',fileName:'',files:[],totalBytes:0,infoHash:'',bytes:null,parseError:'',parsing:false};
const textDecoder=new TextDecoder();
function resetAddDraft(){
  addParseToken+=1;
  addDraft={kind:'none',name:'',fileName:'',files:[],totalBytes:0,infoHash:'',bytes:null,parseError:'',parsing:false};
}
let addReturnFocus=null;
function openAdd(){
  const modal=document.getElementById('addModal');
  if(!modal){return;}
  addReturnFocus=document.activeElement;
  document.getElementById('addError')?.remove();
  modal.classList.add('open');
  resetAddDraft();
  const fileInput=document.getElementById('torrentFile');
  const magnetInput=document.getElementById('magnet');
  const startWhenAdded=document.getElementById('startWhenAdded');
  if(fileInput){fileInput.value='';}
  if(magnetInput){magnetInput.value='';}
  if(startWhenAdded){startWhenAdded.checked=true;}
  updateDropZoneState();
  renderAddReview();
  enhanceUI();
  document.getElementById('magnet')?.focus();
}
function closeAdd(){
  const modal=document.getElementById('addModal');
  if(modal){modal.classList.remove('open')}
  resetAddDraft();
  if(pendingHtml){
    const html=pendingHtml;
    pendingHtml=null;
    queuedLiveHtml=null;
    if(liveRenderTimer!==null){
      clearTimeout(liveRenderTimer);
      liveRenderTimer=null;
    }
    renderApp(html);
  }
  if(addReturnFocus?.isConnected){addReturnFocus.focus();}
}
function maybeClose(e){if(e.target&&e.target.id==='addModal'){closeAdd()}}
document.addEventListener('keydown',e=>{
  const modal=document.getElementById('addModal');
  if(!(modal&&modal.classList.contains('open'))){
    if((e.metaKey||e.ctrlKey)&&e.key.toLowerCase()==='o'){e.preventDefault();openAdd();}
    return;
  }
  if(e.key==='Escape'){e.preventDefault();closeAdd();}
  if(e.key==='Tab'){
    const items=Array.from(modal.querySelectorAll('button:not(:disabled),input:not(:disabled),select:not(:disabled),[tabindex="0"]')).filter(node=>node.getClientRects().length);
    const first=items[0],last=items[items.length-1];
    if(e.shiftKey&&document.activeElement===first){e.preventDefault();last?.focus();}
    else if(!e.shiftKey&&document.activeElement===last){e.preventDefault();first?.focus();}
  }
});
async function chooseDownloadDir(){
  const input=document.getElementById('downloadDir');
  if(!input){return;}
  const current=(input.value||'').trim();
  try{
    const data=await apiPostJson('/select-download-dir');
    const path=data&&typeof data.path==='string'?data.path.trim():'';
    if(path){input.value=path;}
    return;
  }catch(err){
    const message=actionErrorMessage(err);
    const unsupported=message.toLowerCase().includes('not available on this platform');
    if(!unsupported){
      alert('Folder picker failed: '+message);
      return;
    }
  }
  const next=prompt('Download directory path:',current||'');
  if(next===null){return;}
  const value=next.trim();
  if(value){input.value=value;}
}
function actionErrorMessage(err){
  if(err&&err.message){return err.message;}
  return String(err||'unknown error');
}
function toastStack(){
  let stack=document.querySelector('.toast-stack');
  if(stack){return stack;}
  stack=document.createElement('div');
  stack.className='toast-stack';
  stack.setAttribute('role','status');
  stack.setAttribute('aria-live','polite');
  document.body.appendChild(stack);
  return stack;
}
function showToast(title,message){
  const stack=toastStack();
  const toast=document.createElement('div');
  toast.className='toast';
  toast.innerHTML=''
    +'<div class="toast-icon"><svg class="material-symbols-rounded" style="font-size:16px"><use href=#i-check></use></svg></div>'
    +'<div class="toast-copy">'
    +'<div class="toast-title">'+escapeHtml(title||'Done')+'</div>'
    +(message?('<div class="toast-body">'+escapeHtml(message)+'</div>'):'')
    +'</div>';
  stack.appendChild(toast);
  requestAnimationFrame(()=>toast.classList.add('show'));
  setTimeout(()=>{
    toast.classList.remove('show');
    setTimeout(()=>toast.remove(),260);
  },2600);
}
function showActionError(err){
  showToast('Action failed',actionErrorMessage(err));
}
function showAddError(message){
  let error=document.getElementById('addError');
  if(!error){error=document.createElement('div');error.id='addError';error.className='add-error';error.setAttribute('role','alert');document.querySelector('#addModal .modal-actions')?.before(error);}
  error.textContent=message;
}
async function apiPost(url,options){
  if(!apiToken){await refreshApiToken();}
  let attemptedRefresh=false;
  while(true){
    const req=Object.assign({method:'POST',cache:'no-store'},options||{});
    const headers=new Headers(req.headers||{});
    if(apiToken){headers.set('X-Rustorrent-Token',apiToken);}
    req.headers=headers;

    const res=await fetch(url,req);
    if(res.ok){return res;}

    let message='HTTP '+res.status;
    try{
      const type=(res.headers.get('content-type')||'').toLowerCase();
      if(type.includes('application/json')){
        const data=await res.json();
        if(data&&typeof data.error==='string'&&data.error.trim()){message=data.error.trim();}
      }else{
        const text=(await res.text()).trim();
        if(text){message=text;}
      }
    }catch(e){}

    const lower=String(message||'').toLowerCase();
    const tokenError=lower.includes('invalid api token')||(lower.includes('missing')&&lower.includes('api token'));
    if(!attemptedRefresh&&res.status===403&&tokenError){
      attemptedRefresh=true;
      if(await refreshApiToken()){continue;}
    }
    throw new Error(message);
  }
}
async function apiPostJson(url,options){
  const res=await apiPost(url,options);
  const type=(res.headers.get('content-type')||'').toLowerCase();
  if(type.includes('application/json')){
    try{
      const data=await res.json();
      if(data&&typeof data==='object'){return data;}
    }catch(e){}
  }
  return {ok:true};
}
async function fetchJson(url){
  if(!apiToken){await refreshApiToken();}
  const headers=new Headers({'Accept':'application/json'});
  if(apiToken){headers.set('X-Rustorrent-Token',apiToken);}
  const res=await fetch(url,{cache:'no-store',headers:headers});
  if(!res.ok){
    let message='HTTP '+res.status;
    try{
      const data=await res.json();
      if(data&&typeof data.error==='string'&&data.error.trim()){message=data.error.trim();}
    }catch(e){}
    throw new Error(message);
  }
  return res.json();
}
function loadStoredSearchPlugins(){
  try{
    const raw=localStorage.getItem(searchPluginSelectionKey)||'[]';
    const parsed=JSON.parse(raw);
    if(Array.isArray(parsed)){return parsed.map(v=>String(v||'')).filter(Boolean);}
  }catch(e){}
  return [];
}
function saveStoredSearchPlugins(modules){
  try{localStorage.setItem(searchPluginSelectionKey,JSON.stringify(modules||[]));}catch(e){}
}
function resolveSearchCategory(){
  try{
    const value=localStorage.getItem(searchCategoryKey)||'all';
    return value||'all';
  }catch(e){}
  return 'all';
}
function saveSearchCategory(value){
  try{localStorage.setItem(searchCategoryKey,String(value||'all'));}catch(e){}
}
function resolveSearchSort(){
  try{
    const raw=localStorage.getItem(searchSortStorageKey)||'';
    const [key,dir]=raw.split(':');
    if(key&&dir&&(dir==='asc'||dir==='desc')){
      return {key:String(key),dir:String(dir)};
    }
  }catch(e){}
  return {key:'seeds',dir:'desc'};
}
function saveSearchSort(sort){
  try{localStorage.setItem(searchSortStorageKey,String(sort.key||'seeds')+':'+String(sort.dir||'desc'));}catch(e){}
}
function setSearchSort(key){
  const nextKey=String(key||'seeds');
  const nextDir=(activeSearchSort.key===nextKey&&activeSearchSort.dir==='desc')?'asc':'desc';
  activeSearchSort={key:nextKey,dir:nextDir};
  saveSearchSort(activeSearchSort);
  renderSearchResults(searchStateCache&&Array.isArray(searchStateCache.results)?searchStateCache.results:[]);
}
function activeSearchPlugins(plugins){
  const available=(Array.isArray(plugins)?plugins:[]).filter(plugin=>plugin&&plugin.healthy).map(plugin=>String(plugin.module||'')).filter(Boolean);
  const stored=loadStoredSearchPlugins().filter(module=>available.indexOf(module)!==-1);
  if(stored.length>0){return stored;}
  return available;
}
function selectedSearchPluginsFromDom(){
  return Array.from(document.querySelectorAll('input[data-search-plugin]'))
    .filter(input=>input.checked&&!input.disabled)
    .map(input=>String(input.getAttribute('data-search-plugin')||''))
    .filter(Boolean);
}
function persistSearchPluginSelectionFromDom(){
  saveStoredSearchPlugins(selectedSearchPluginsFromDom());
}
function recommendedCatalogModules(){
  return ['piratebay','1337x','bitsearch','limetorrents','torlock','nyaasi','eztv','yts'];
}
function recommendedCatalogEntries(entries){
  const preferred=recommendedCatalogModules();
  const excluded=new Set(['magnetdl']);
  const byModule=new Map();
  const byName=new Map();
  for(const entry of (Array.isArray(entries)?entries:[])){
    if(!entry){continue;}
    const module=String(entry.module||'').toLowerCase();
    const name=String(entry.name||'').toLowerCase();
    if(excluded.has(module)||name.includes('magnetdl')){continue;}
    if(module){byModule.set(module,entry);}
    if(name){byName.set(name,entry);}
  }
  const picked=[];
  for(const key of preferred){
    const normalized=String(key||'').toLowerCase();
    const entry=byModule.get(normalized)||Array.from(byName.values()).find(item=>String(item.name||'').toLowerCase().includes(normalized));
    if(entry&&!picked.some(item=>item.module===entry.module)){
      picked.push(entry);
    }
  }
  return picked;
}
function useAllReadyPlugins(){
  const plugins=Array.isArray(searchStateCache&&searchStateCache.plugins)?searchStateCache.plugins:[];
  const all=plugins.filter(plugin=>plugin&&plugin.healthy).map(plugin=>String(plugin.module||'')).filter(Boolean);
  saveStoredSearchPlugins(all);
  renderSearchStatus(searchStateCache||{plugins:plugins||[]});
}
function renderSearchPluginList(plugins){
  const list=document.getElementById('searchPluginList');
  if(!list){return;}
  const visiblePlugins=(Array.isArray(plugins)?plugins:[]).filter(plugin=>plugin&&String(plugin.module||'')!=='__init__');
  const selected=activeSearchPlugins(visiblePlugins);
  saveStoredSearchPlugins(selected);
  if(visiblePlugins.length===0){
    list.innerHTML='<div class="rss-item"><span class="rss-item-info">No search plugins installed yet.</span></div>';
    return;
  }
  list.innerHTML='<table class="search-plugin-table"><tbody>'+visiblePlugins.map(plugin=>{
    const module=escapeHtml(plugin.module||'');
    const checked=selected.indexOf(plugin.module)!==-1;
    const healthy=!!plugin.healthy;
    const badge=healthy?'<span class="search-plugin-badge ready">Ready</span>':'<span class="search-plugin-badge">Broken</span>';
    const version=plugin.version?(' v'+escapeHtml(plugin.version)):'';
    const cats=Array.isArray(plugin.categories)&&plugin.categories.length>0?escapeHtml(plugin.categories.join(', ')):'all';
    const reason=plugin.broken_reason?'<div style="color:var(--error);font-size:11px;margin-top:2px">'+escapeHtml(plugin.broken_reason)+'</div>':'';
    return '<tr>'
      +'<td><input type="checkbox" data-search-plugin="'+module+'" '+(checked&&healthy?'checked ':'')+(healthy?'':'disabled ')+'title="Enable for search"></td>'
      +'<td><div style="font-weight:600">'+escapeHtml(plugin.display_name||plugin.module||'plugin')+version+' '+badge+'</div>'
      +'<div style="opacity:0.6;font-size:11px">'+cats+'</div>'+reason+'</td>'
      +'<td><button class="btn ghost search-remove-btn" type="button" data-action="search-remove-plugin" data-module="'+module+'" title="Uninstall plugin"><svg class="material-symbols-rounded" style="font-size:16px"><use href=#i-delete></use></svg></button></td>'
      +'</tr>';
  }).join('')+'</tbody></table>';
}
function renderRecommendedCatalog(entries){
  const container=document.getElementById('searchRecommended');
  if(!container){return;}
  const recommended=recommendedCatalogEntries(entries);
  if(recommended.length===0){
    container.innerHTML='<div class="rss-item"><span class="rss-item-info">Recommended public plugins will appear here when they are available in the live catalog.</span></div>';
    return;
  }
  container.innerHTML='<div class="search-recommended-list">'+recommended.map(entry=>''
    +'<div class="search-recommended-card">'
    +'<div class="search-recommended-name">'+escapeHtml(entry.name||entry.module||'plugin')
      +(entry.installed?(' <span class="search-plugin-badge'+(entry.installed_healthy?' ready':'')+'">'+(entry.installed_healthy?'Installed':'Needs fix')+'</span>'):'')
      +'</div>'
    +'<div class="search-recommended-copy">'+escapeHtml(entry.comment||'Popular public search source.')+'</div>'
    +'<div class="search-recommended-meta">'+escapeHtml([entry.author||'',entry.version?('wiki v'+entry.version):'',entry.updated||''].filter(Boolean).join(' / '))+'</div>'
    +'<button class="btn primary" type="button" data-action="search-install-catalog" data-url="'+escapeHtml(entry.download_url||'')+'">'+(entry.installed?'Update':'Install')+'</button>'
    +'</div>'
  ).join('')+'</div>';
}
function renderSearchResults(results){
  const container=document.getElementById('searchResults');
  if(!container){return;}
  if(!activeSearchSort||!activeSearchSort.key){
    activeSearchSort=resolveSearchSort();
  }
  if(!Array.isArray(results)||results.length===0){
    container.innerHTML='<div class="search-results-empty"><div>Results will appear here after you search.</div><div class="small" style="margin-top:6px">Enter a query on the left, click Search, and use Manage Plugins if you want to enable or install providers.</div></div>';
    return;
  }
  const sortKey=String(activeSearchSort&&activeSearchSort.key||'seeds');
  const sortDir=String(activeSearchSort&&activeSearchSort.dir||'desc');
  const multiplier=sortDir==='asc'?1:-1;
  const sorted=results.slice().sort((left,right)=>{
    const key=sortKey;
    let cmp=0;
    if(key==='name'){
      cmp=String(left.name||'').localeCompare(String(right.name||''));
    }else if(key==='plugin'){
      cmp=String(left.plugin||left.site_url||'').localeCompare(String(right.plugin||right.site_url||''));
    }else if(key==='size'){
      cmp=(Number(left.size)||0)-(Number(right.size)||0);
    }else if(key==='leech'){
      cmp=(Number(left.leech)||0)-(Number(right.leech)||0);
    }else if(key==='updated'){
      cmp=(Number(left.pub_date)||0)-(Number(right.pub_date)||0);
    }else{
      cmp=(Number(left.seeds)||0)-(Number(right.seeds)||0);
    }
    if(cmp===0){
      cmp=String(left.name||'').localeCompare(String(right.name||''));
    }
    return cmp*multiplier;
  });
  const sortIcon=(key)=>{
    if(sortKey!==key){return 'unfold_more';}
    return sortDir==='asc'?'arrow_upward':'arrow_downward';
  };
  const sortHeader=(key,label)=>'<button class="search-sort-btn" type="button" data-search-sort="'+key+'">'+label+'<svg class="material-symbols-rounded" style="font-size:14px"><use href=#i-'+sortIcon(key)+'></use></svg></button>';
  const formatUpdated=(value)=>{
    const ts=Number(value)||0;
    if(ts<=0){return '—';}
    const date=new Date(ts*1000);
    if(Number.isNaN(date.getTime())){return '—';}
    return date.toLocaleDateString();
  };
  container.innerHTML=''
    +'<div class="search-results-table-wrap"><table class="search-results-table">'
    +'<thead><tr>'
    +'<th>'+sortHeader('name','Name')+'</th>'
    +'<th>'+sortHeader('plugin','Plugin')+'</th>'
    +'<th>'+sortHeader('size','Size')+'</th>'
    +'<th>'+sortHeader('seeds','Seeds')+'</th>'
    +'<th>'+sortHeader('leech','Leech')+'</th>'
    +'<th>'+sortHeader('updated','Updated')+'</th>'
    +'<th>Action</th>'
    +'</tr></thead><tbody>'
    +sorted.map(result=>{
      const added=addedSearchResults.has(String(result.index));
      const pluginLabel=result.plugin?escapeHtml(result.plugin):escapeHtml(result.site_url||'plugin');
      const size=Number(result.size);
      const seeds=Number(result.seeds);
      const leech=Number(result.leech);
      const safeDesc=safeExternalUrl(result.desc_link||'');
      const desc=safeDesc?'<a class="search-result-link" href="'+escapeHtml(safeDesc)+'" target="_blank" rel="noopener noreferrer">Open description</a>':'';
      return ''
        +'<tr>'
        +'<td><div class="search-result-title">'+escapeHtml(result.name||'result')+'</div>'+desc+'</td>'
        +'<td>'+pluginLabel+'</td>'
        +'<td>'+(size>0?formatBytes(size):'size unknown')+'</td>'
        +'<td>'+(seeds>=0?seeds:'?')+'</td>'
        +'<td>'+(leech>=0?leech:'?')+'</td>'
        +'<td>'+formatUpdated(result.pub_date)+'</td>'
        +'<td><button class="btn primary'+(added?' added':'')+'" type="button" data-action="search-add-result" data-index="'+escapeHtml(String(result.index))+'" data-name="'+escapeHtml(result.name||'Search result')+'" '+(added?'disabled aria-disabled="true"':'')+'>'+(added?'Added':'Add')+'</button></td>'
        +'</tr>';
    }).join('')
    +'</tbody></table></div>';
}
function renderSearchStatus(data){
  searchStateCache=data||null;
  if(!activeSearchSort||!activeSearchSort.key){
    activeSearchSort=resolveSearchSort();
  }
  const startedAt=Number(data&&data.last_started_at)||0;
  if(startedAt>0&&startedAt!==lastSearchStartedAt){
    lastSearchStartedAt=startedAt;
    addedSearchResults.clear();
  }
  const category=document.getElementById('searchCategory');
  if(category&&category.value!==resolveSearchCategory()){category.value=resolveSearchCategory();}
  const pluginError=document.getElementById('searchPluginError');
  if(pluginError){
    const message=(data&&data.plugin_error?String(data.plugin_error):'').trim();
    pluginError.textContent=message;
    pluginError.style.display=message?'block':'none';
  }
  renderSearchPluginList(data&&Array.isArray(data.plugins)?data.plugins:[]);
  renderSearchResults(data&&Array.isArray(data.results)?data.results:[]);
  const summary=document.getElementById('searchSelectionSummary');
  const warning=document.getElementById('searchPluginWarning');
  if(summary){
    const plugins=Array.isArray(data&&data.plugins)?data.plugins:[];
    const selected=activeSearchPlugins(plugins);
    const healthyCount=plugins.filter(plugin=>plugin&&plugin.healthy).length;
    let summaryHtml='';
    if(healthyCount===0){
      summaryHtml='<span class="search-selection-text">No ready plugins installed yet.</span>';
    }else if(selected.length===healthyCount){
      summaryHtml='<span class="search-selection-text">Using all '+healthyCount+' ready plugins.</span>';
    }else{
      summaryHtml='<span class="search-selection-text">Using '+selected.length+' of '+healthyCount+' ready plugins. Some providers are disabled in Plugins.</span>'
        +'<button class="btn ghost" type="button" onclick="useAllReadyPlugins()">Use All</button>';
    }
    summary.innerHTML=summaryHtml;
  }
  if(warning){
    const plugins=Array.isArray(data&&data.plugins)?data.plugins:[];
    const healthyCount=plugins.filter(plugin=>plugin&&plugin.healthy).length;
    if(healthyCount===0){
      warning.style.display='block';
      warning.innerHTML=''
        +'<div class="search-warning-title">No Search Plugins Installed</div>'
        +'<div class="search-warning-copy">Install at least one public plugin before searching. rustorrent does not ship with active public search providers by default.</div>'
        +'<button class="btn primary" type="button" data-search-view-target="plugins"><svg class="material-symbols-rounded"><use href=#i-extension></use></svg>Manage Plugins</button>';
    }else{
      warning.style.display='none';
      warning.innerHTML='';
    }
  }
  const queryInput=document.getElementById('searchQuery');
  if(queryInput&&!queryInput.matches(':focus')){
    const next=data&&typeof data.query==='string'?data.query:'';
    if(queryInput.value!==next){queryInput.value=next;}
  }
  const status=document.getElementById('searchStatusText');
  if(status){
    let message='Enter a query on the left and click Search. Use Manage Plugins to change providers.';
    if(data&&data.busy){message='Searching across installed plugins...';}
    else if(data&&data.last_error){message=String(data.last_error);}
    else if(data&&Array.isArray(data.results)&&data.results.length>0){message='Found '+data.results.length+' search results.';}
    else if(data&&data.plugin_error){message=String(data.plugin_error);}
    else if(data&&Array.isArray(data.plugins)&&data.plugins.length>0){message='Installed plugins are ready. Enter a query to search.';}
    else if(data&&data.python_available){message='No search plugins are installed yet. Open Plugins or Community Catalog to add one.';}
    status.textContent=message;
  }
  if(!(data&&data.busy)){
    searchPollActive=false;
    if(searchPollTimer!==null){
      clearTimeout(searchPollTimer);
      searchPollTimer=null;
    }
  }else{
    scheduleSearchStatusPoll(900);
  }
}
async function loadSearchStatus(_forceRefresh){
  const data=await fetchJson('/search/status');
  renderSearchStatus(data);
  return data;
}
function scheduleSearchStatusPoll(delayMs){
  if(searchPollActive){return;}
  if(searchPollTimer!==null){
    clearTimeout(searchPollTimer);
  }
  searchPollTimer=setTimeout(()=>pumpSearchStatus().catch(err=>console.warn(err)),delayMs);
}
async function pumpSearchStatus(){
  if(searchPollActive){return;}
  searchPollActive=true;
  if(searchPollTimer!==null){
    clearTimeout(searchPollTimer);
    searchPollTimer=null;
  }
  try{
    while(true){
      const data=await fetchJson('/search/status');
      renderSearchStatus(data);
      if(!(data&&data.busy)){break;}
      await sleep(900);
    }
  }finally{
    searchPollActive=false;
  }
}
function renderSearchCatalog(entries){
  const container=document.getElementById('searchCatalog');
  if(!container){return;}
  renderRecommendedCatalog(entries);
  const meta=document.getElementById('searchCatalogMeta');
  const filter=document.getElementById('searchCatalogFilter');
  const search=filter?String(filter.value||'').trim().toLowerCase():'';
  const sorted=(Array.isArray(entries)?entries:[]).slice().sort((a,b)=>{
    const installedA=!!(a&&a.installed);
    const installedB=!!(b&&b.installed);
    if(installedA!==installedB){return installedA?-1:1;}
    return String(a&&a.name||'').localeCompare(String(b&&b.name||''));
  });
  const filtered=sorted.filter(entry=>{
    if(!search){return true;}
    const haystack=[entry.name,entry.author,entry.comment,entry.version,entry.module].join(' ').toLowerCase();
    return haystack.includes(search);
  });
  if(meta){
    if(searchCatalogLoading){
      meta.textContent='Loading latest qBittorrent unofficial plugin list...';
    }else if(searchCatalogFetchedAt>0){
      meta.textContent='Live list loaded from the qBittorrent unofficial search plugin wiki.';
    }else{
      meta.textContent='Latest unofficial qBittorrent search plugins.';
    }
  }
  if(filtered.length===0){
    container.innerHTML='<div class="rss-item"><span class="rss-item-info">'+(search?'No community plugins match that filter.':'Load the community list to install unofficial qBittorrent plugins with one click.')+'</span></div>';
    return;
  }
  container.innerHTML='<table class="search-catalog-table"><tbody>'+filtered.map(entry=>{
    const name=escapeHtml(entry.name||entry.module||'plugin');
    const author=entry.author?escapeHtml(entry.author):'';
    const version=entry.version?'v'+escapeHtml(entry.version):'';
    const status=entry.installed
      ?'<span class="search-plugin-badge'+(entry.installed_healthy?' ready':'')+'">'+
       (entry.installed_healthy?'Installed':'Needs fix')+'</span>'
      :'';
    const meta=[author,version,entry.updated||''].filter(Boolean).join(' \u00b7 ');
    const btnLabel=entry.installed?'Update':'Install';
    return '<tr>'
      +'<td><div style="font-weight:600">'+name+' '+status+'</div>'
      +'<div style="opacity:0.6;font-size:11px">'+escapeHtml(meta)+'</div></td>'
      +'<td><button class="btn '+(entry.installed?'ghost':'primary')+' search-catalog-btn" type="button" data-action="search-install-catalog" data-url="'+escapeHtml(entry.download_url||'')+'">'+btnLabel+'</button></td>'
      +'</tr>';
  }).join('')+'</tbody></table>';
}
async function loadSearchCatalog(force){
  if(searchCatalogLoading){return {entries:searchCatalogCache};}
  searchCatalogLoading=true;
  renderSearchCatalog(searchCatalogCache);
  const suffix=force?'?refresh=1':'';
  try{
    const data=await fetchJson('/search/catalog'+suffix);
    searchCatalogCache=Array.isArray(data&&data.entries)?data.entries:[];
    searchCatalogFetchedAt=Number(data&&data.fetched_at)||0;
    renderSearchCatalog(searchCatalogCache);
    if(data&&data.error){
      const box=document.getElementById('searchPluginError');
      if(box){
        box.textContent=String(data.error);
        box.style.display='block';
      }
    }
    return data;
  }finally{
    searchCatalogLoading=false;
    renderSearchCatalog(searchCatalogCache);
  }
}
function safeExternalUrl(value){
  if(!value){return '';}
  try{
    const parsed=new URL(String(value),window.location.href);
    return parsed.protocol==='http:'||parsed.protocol==='https:'?parsed.href:'';
  }catch(e){return '';}
}
function confirmSearchPluginInstall(name){
  return confirm('Search plugins run third-party Python code on this computer. Install '+(name||'this plugin')+' only if you trust its source. Continue?');
}
async function installCatalogPlugin(url,skipConfirmation){
  if(!url){return;}
  if(!skipConfirmation&&!confirmSearchPluginInstall('the selected plugin')){return;}
  await apiPost('/search/install-url',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:'url='+encodeURIComponent(url)});
  await Promise.all([
    loadSearchStatus(true),
    loadSearchCatalog(true),
  ]);
}
async function updateInstalledCatalogPlugins(){
  const installed=(searchCatalogCache||[]).filter(entry=>entry&&entry.installed&&entry.download_url);
  if(installed.length===0){
    alert('No installed community plugins are linked to the live catalog.');
    return;
  }
  if(!confirmSearchPluginInstall('updates for all installed community plugins')){return;}
  for(const entry of installed){
    await apiPost('/search/install-url',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:'url='+encodeURIComponent(entry.download_url)});
  }
  await Promise.all([
    loadSearchStatus(true),
    loadSearchCatalog(true),
  ]);
}
async function submitSearchQuery(e){
  if(e){e.preventDefault();}
  const queryInput=document.getElementById('searchQuery');
  const categoryInput=document.getElementById('searchCategory');
  const query=queryInput?String(queryInput.value||'').trim():'';
  const category=categoryInput?String(categoryInput.value||'all'):'all';
  const engines=selectedSearchPluginsFromDom();
  if(!query){alert('Enter a search query.');return false;}
  saveStoredSearchPlugins(engines);
  saveSearchCategory(category);
  addedSearchResults.clear();
  lastSearchStartedAt=0;
  const submitBtn=document.querySelector('.search-form button[type=\"submit\"]');
  if(submitBtn){submitBtn.disabled=true;submitBtn.textContent='Searching\u2026';}
  const status=document.getElementById('searchStatusText');
  if(status){status.textContent='Searching across installed plugins\u2026';}
  const resultsList=document.getElementById('searchResults');
  if(resultsList){resultsList.innerHTML='<div style=\"text-align:center;padding:32px 0;opacity:0.6\">Searching\u2026</div>';}
  const body='query='+encodeURIComponent(query)+'&category='+encodeURIComponent(category)+'&engines='+encodeURIComponent(engines.join(','));
  try{
    await apiPost('/search/run',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:body});
  }finally{
    if(submitBtn){submitBtn.disabled=false;submitBtn.textContent='Search';}
  }
  applySearchPanelView('results');
  scheduleSearchStatusPoll(100);
  await loadSearchStatus(false);
  return false;
}
async function installSearchPluginUrl(e){
  if(e){e.preventDefault();}
  const input=document.getElementById('searchPluginUrl');
  const url=input?String(input.value||'').trim():'';
  if(!url){return false;}
  await installCatalogPlugin(url);
  if(input){input.value='';}
  return false;
}
async function installSearchPluginFile(e){
  const input=e&&e.target?e.target:null;
  const file=input&&input.files&&input.files[0]?input.files[0]:null;
  if(!file){return;}
  if(!confirmSearchPluginInstall(file.name)){
    if(input){input.value='';}
    return;
  }
  const bytes=new Uint8Array(await file.arrayBuffer());
  await apiPost('/search/install-plugin?filename='+encodeURIComponent(file.name),{headers:{'Content-Type':'text/x-python'},body:bytes});
  if(input){input.value='';}
  await Promise.all([
    loadSearchStatus(true),
    loadSearchCatalog(true),
  ]);
}
async function removeSearchPlugin(module){
  if(!module){return;}
  const ok=confirm('Remove search plugin '+module+'?');
  if(!ok){return;}
  await apiPost('/search/remove-plugin',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:'module='+encodeURIComponent(module)});
  await Promise.all([
    loadSearchStatus(true),
    loadSearchCatalog(true),
  ]);
}
async function addSearchResult(index,name){
  const downloadDir=document.getElementById('downloadDir');
  const preallocate=document.getElementById('preallocate');
  const body='index='+encodeURIComponent(index)
    +'&dir='+encodeURIComponent(downloadDir?String(downloadDir.value||'').trim():'')
    +'&prealloc='+(preallocate&&preallocate.checked?'1':'0');
  await apiPostJson('/search/add-result',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:body});
  addedSearchResults.add(String(index));
  if(searchStateCache&&Array.isArray(searchStateCache.results)){
    renderSearchResults(searchStateCache.results);
  }
  showToast('Torrent added',name||'The search result was queued in rustorrent.');
  scheduleRefreshFallback();
}
function escapeHtml(value){
  return String(value||'')
    .replace(/&/g,'&amp;')
    .replace(/</g,'&lt;')
    .replace(/>/g,'&gt;')
    .replace(/"/g,'&quot;')
    .replace(/'/g,'&#39;');
}
function formatBytes(value){
  const units=['B','KB','MB','GB','TB'];
  let size=Math.max(0,Number(value)||0);
  let unit=0;
  while(size>=1024&&unit+1<units.length){size/=1024;unit+=1;}
  if(unit===0){return Math.round(size)+' '+units[unit];}
  return size.toFixed(2)+' '+units[unit];
}
function formatRateLimitKbps(kbps){
  const value=Math.max(0,Number(kbps)||0);
  if(value===0){return 'Unlimited';}
  return formatBytes(value*1024)+'/s';
}
function updateRateLimitLabels(){
  const down=document.getElementById('downloadLimit');
  const up=document.getElementById('uploadLimit');
  const downLabel=document.getElementById('downloadLimitValue');
  const upLabel=document.getElementById('uploadLimitValue');
  if(down&&downLabel){
    const next=Math.max(0,Math.min(MAX_RATE_LIMIT_KBPS,Number(down.value)||0));
    down.value=String(next);
    downLabel.textContent=formatRateLimitKbps(next);
  }
  if(up&&upLabel){
    const next=Math.max(0,Math.min(MAX_RATE_LIMIT_KBPS,Number(up.value)||0));
    up.value=String(next);
    upLabel.textContent=formatRateLimitKbps(next);
  }
}
function sleep(ms){return new Promise(resolve=>setTimeout(resolve,ms));}
function decodeUtf8(bytes){
  try{return textDecoder.decode(bytes);}catch(e){return '';}
}
function parseBencode(bytes){
  let nodes=0;
  if(bytes.length>2*1024*1024){throw new Error('Torrent files must be smaller than 2 MiB.');}
  function parseAt(offset,depth=0){
    if(depth>64||++nodes>100000){throw new Error('Torrent metadata is too complex.');}
    if(offset>=bytes.length){throw new Error('unexpected end of file');}
    const marker=bytes[offset];
    if(marker===100){
      let idx=offset+1;
      const dict=Object.create(null);
      while(idx<bytes.length&&bytes[idx]!==101){
        const keyNode=parseAt(idx,depth+1);
        if(keyNode.t!=='bytes'){throw new Error('invalid dictionary key');}
        const key=decodeUtf8(keyNode.v);
        const valueNode=parseAt(keyNode.end,depth+1);
        if(Object.prototype.hasOwnProperty.call(dict,key)){throw new Error('duplicate dictionary key');}
        dict[key]=valueNode;
        idx=valueNode.end;
      }
      if(bytes[idx]!==101){throw new Error('unterminated dictionary');}
      return {t:'dict',v:dict,start:offset,end:idx+1};
    }
    if(marker===108){
      let idx=offset+1;
      const list=[];
      while(idx<bytes.length&&bytes[idx]!==101){
        const valueNode=parseAt(idx,depth+1);
        list.push(valueNode);
        idx=valueNode.end;
      }
      if(bytes[idx]!==101){throw new Error('unterminated list');}
      return {t:'list',v:list,start:offset,end:idx+1};
    }
    if(marker===105){
      let idx=offset+1;
      while(idx<bytes.length&&bytes[idx]!==101){idx+=1;}
      if(bytes[idx]!==101){throw new Error('unterminated integer');}
      const raw=textDecoder.decode(bytes.subarray(offset+1,idx));
      const value=Number(raw);
      if(!Number.isSafeInteger(value)||!(/^(0|-?[1-9][0-9]*)$/).test(raw)){throw new Error('invalid integer');}
      return {t:'int',v:value,start:offset,end:idx+1};
    }
    if(marker>=48&&marker<=57){
      let colon=offset;
      while(colon<bytes.length&&bytes[colon]!==58){
        if(bytes[colon]<48||bytes[colon]>57){throw new Error('invalid byte string length');}
        colon+=1;
      }
      if(bytes[colon]!==58){throw new Error('invalid byte string');}
      const lenRaw=textDecoder.decode(bytes.subarray(offset,colon));
      const len=Number(lenRaw);
      if(!Number.isSafeInteger(len)||len<0){throw new Error('invalid byte string length');}
      const start=colon+1;
      const end=start+len;
      if(end>bytes.length){throw new Error('byte string exceeds buffer');}
      return {t:'bytes',v:bytes.slice(start,end),start:offset,end:end};
    }
    throw new Error('invalid bencode token');
  }
  const root=parseAt(0);
  if(root.end!==bytes.length){throw new Error('trailing data');}
  return root;
}
function nodeString(node){
  if(!node||node.t!=='bytes'){return '';}
  return decodeUtf8(node.v);
}
function nodeInt(node){
  if(!node||node.t!=='int'){return 0;}
  return Math.max(0,Math.floor(node.v));
}
function extractInfoHashFromMagnet(magnet){
  const match=/[?&]xt=urn:btih:([^&]+)/i.exec(String(magnet||''));
  if(!match){return '';}
  let value='';
  try{value=decodeURIComponent(match[1]);}catch(e){value=match[1];}
  if(/^[a-f0-9]{40}$/i.test(value)){return value.toLowerCase();}
  return '';
}
async function sha1Hex(bytes){
  const digest=await crypto.subtle.digest('SHA-1',bytes);
  return Array.from(new Uint8Array(digest)).map(v=>v.toString(16).padStart(2,'0')).join('');
}
async function parseTorrentPreview(bytes,fallbackName){
  const root=parseBencode(bytes);
  if(!root||root.t!=='dict'){throw new Error('invalid torrent file');}
  const info=root.v.info;
  if(!info||info.t!=='dict'){throw new Error('missing info dictionary');}
  const name=nodeString(info.v['name.utf-8']||info.v.name)||fallbackName||'torrent';
  const files=[];
  const fileList=info.v.files;
  if(fileList&&fileList.t==='list'&&fileList.v.length>0){
    fileList.v.forEach((entry,idx)=>{
      if(!entry||entry.t!=='dict'){return;}
      const length=nodeInt(entry.v.length);
      const pathNode=entry.v['path.utf-8']||entry.v.path;
      const segments=(pathNode&&pathNode.t==='list')?pathNode.v.map(nodeString).filter(Boolean):[];
      const relPath=segments.join('/');
      files.push({index:idx,path:relPath?name+'/'+relPath:name,length:length,selected:true});
    });
  }else if(info.v['file tree']?.t==='dict'){
    function visit(tree,segments){
      if(tree['']?.t==='dict'){
        files.push({index:files.length,path:[name,...segments].join('/'),length:nodeInt(tree[''].v.length),selected:true});
      }
      for(const key of Object.keys(tree).sort()){
        if(key&&tree[key].t==='dict'){visit(tree[key].v,[...segments,key]);}
      }
    }
    visit(info.v['file tree'].v,[]);
  }else{
    files.push({index:0,path:name,length:nodeInt(info.v.length),selected:true});
  }
  const totalBytes=files.reduce((acc,file)=>acc+file.length,0);
  const infoHash=await sha1Hex(bytes.slice(info.start,info.end));
  return {name:name,files:files,totalBytes:totalBytes,infoHash:infoHash};
}
function renderAddReview(){
  const summary=document.getElementById('addSummary');
  const review=document.getElementById('addReview');
  const fileInput=document.getElementById('torrentFile');
  const magnetInput=document.getElementById('magnet');
  const addBtn=document.querySelector('#addModal .btn.primary');
  if(!summary||!review||!addBtn){return;}
  const hasFile=!!(fileInput&&fileInput.files&&fileInput.files[0]);
  const magnet=magnetInput?magnetInput.value.trim():'';
  if(addDraft.parsing){
    summary.textContent='Reading torrent metadata...';
    review.style.display='none';
    addBtn.disabled=true;
    return;
  }
  if(hasFile){
    if(addDraft.parseError){
      summary.textContent='Could not read this .torrent file. '+addDraft.parseError;
      review.style.display='none';
      addBtn.disabled=true;
      return;
    }
    if(addDraft.kind==='file'&&addDraft.bytes&&addDraft.files.length>0){
      const selected=addDraft.files.filter(file=>file.selected).length;
      summary.textContent='Adding 1 torrent';
      const rows=addDraft.files.map((file,idx)=>(
        '<div class=\"add-file-row\">'
        +'<div><input aria-label=\"Download '+escapeHtml(file.path)+'\" type=\"checkbox\" '+(file.selected?'checked ':'')+'onchange=\"toggleReviewFile('+idx+',this.checked)\"></div>'
        +'<div class=\"add-file-name\" title=\"'+escapeHtml(file.path)+'\">'+escapeHtml(file.path)+'</div>'
        +'<div class=\"add-file-size\">'+formatBytes(file.length)+'</div>'
        +'</div>'
      )).join('');
      review.innerHTML=''
        +'<div class=\"add-review-title\">'+escapeHtml(addDraft.name||addDraft.fileName||'torrent')+'</div>'
        +'<div class=\"add-review-meta\"><span>'+addDraft.files.length+' files</span><span>'+formatBytes(addDraft.totalBytes)+'</span><span>'+selected+' selected</span></div>'
        +'<div class=\"add-file-actions\"><button class=\"btn\" type=\"button\" onclick=\"toggleAllReviewFiles(true)\">Select all</button><button class=\"btn\" type=\"button\" onclick=\"toggleAllReviewFiles(false)\">Clear all</button></div>'
        +'<div class=\"add-file-list\"><div class=\"add-file-head\"><div></div><div>Name</div><div class=\"add-file-size\">Size</div></div>'+rows+'</div>';
      review.style.display='block';
      addBtn.disabled=selected===0;
      return;
    }
    summary.textContent='Select a valid .torrent file.';
    review.style.display='none';
    addBtn.disabled=true;
    return;
  }
  if(magnet){
    let valid=false;
    try{const link=new URL(magnet);valid=link.protocol==='magnet:'&&link.searchParams.getAll('xt').some(value=>/^(urn:btih:([0-9a-f]{40}|[a-z2-7]{32})|urn:btmh:1220[0-9a-f]{64})$/i.test(value));}catch(e){}
    summary.textContent=valid?'Magnet link ready to add.':'Enter a valid magnet link with a BitTorrent info hash.';
    review.style.display='none';
    addBtn.disabled=!valid;
    return;
  }
  summary.textContent='Select a .torrent file or paste a magnet link.';
  review.style.display='none';
  addBtn.disabled=true;
}
function toggleReviewFile(index,checked){
  if(addDraft.kind!=='file'||!addDraft.files[index]){return;}
  addDraft.files[index].selected=!!checked;
  renderAddReview();
}
function toggleAllReviewFiles(checked){
  if(addDraft.kind!=='file'){return;}
  addDraft.files.forEach(file=>{file.selected=!!checked;});
  renderAddReview();
}
async function handleTorrentInputChange(){
  const fileInput=document.getElementById('torrentFile');
  const magnetInput=document.getElementById('magnet');
  const file=fileInput&&fileInput.files?fileInput.files[0]:null;
  if(file&&magnetInput){magnetInput.value='';}
  const token=addParseToken+1;
  addParseToken=token;
  updateDropZoneState();
  if(!file){
    resetAddDraft();
    renderAddReview();
    return;
  }
  addDraft={kind:'file',name:file.name,fileName:file.name,files:[],totalBytes:file.size||0,infoHash:'',bytes:null,parseError:'',parsing:true};
  renderAddReview();
  try{
    const bytes=new Uint8Array(await file.arrayBuffer());
    if(token!==addParseToken){return;}
    const parsed=await parseTorrentPreview(bytes,file.name);
    if(token!==addParseToken){return;}
    addDraft={kind:'file',name:parsed.name,fileName:file.name,files:parsed.files,totalBytes:parsed.totalBytes,infoHash:parsed.infoHash,bytes:bytes,parseError:'',parsing:false};
  }catch(err){
    if(token!==addParseToken){return;}
    addDraft={kind:'file',name:file.name,fileName:file.name,files:[],totalBytes:file.size||0,infoHash:'',bytes:null,parseError:actionErrorMessage(err),parsing:false};
  }
  renderAddReview();
}
function handleMagnetInput(){
  const magnetInput=document.getElementById('magnet');
  const fileInput=document.getElementById('torrentFile');
  const magnet=magnetInput?magnetInput.value.trim():'';
  addParseToken+=1;
  if(magnet&&fileInput){fileInput.value='';}
  if(magnet){
    addDraft={kind:'magnet',name:'',fileName:'',files:[],totalBytes:0,infoHash:extractInfoHashFromMagnet(magnet),bytes:null,parseError:'',parsing:false};
  }else{
    resetAddDraft();
  }
  updateDropZoneState();
  renderAddReview();
}
function clearTorrentFile(){
  const fileInput=document.getElementById('torrentFile');
  if(fileInput){fileInput.value='';}
  resetAddDraft();
  updateDropZoneState();
  renderAddReview();
}
function updateDropZoneState(){
  const dz=document.getElementById('dropZone');
  const nameEl=document.getElementById('dzFileName');
  if(!dz){return;}
  const fileInput=document.getElementById('torrentFile');
  const hasFile=!!(fileInput&&fileInput.files&&fileInput.files[0]);
  if(hasFile){
    dz.classList.add('has-file');
    if(nameEl){nameEl.textContent=fileInput.files[0].name;}
  }else{
    dz.classList.remove('has-file');
    if(nameEl){nameEl.textContent='';}
  }
}
(function initDragDrop(){
  let dragCount=0;
  const overlay=()=>document.getElementById('pageDropOverlay');
  function isTorrentDrag(e){
    if(!e.dataTransfer||!e.dataTransfer.types){return false;}
    return e.dataTransfer.types.indexOf('Files')!==-1;
  }
  document.addEventListener('dragenter',e=>{
    if(!isTorrentDrag(e)){return;}
    e.preventDefault();
    dragCount+=1;
    const ov=overlay();
    if(ov){ov.classList.add('active');}
  });
  document.addEventListener('dragleave',e=>{
    dragCount-=1;
    if(dragCount<=0){
      dragCount=0;
      const ov=overlay();
      if(ov){ov.classList.remove('active');}
    }
  });
  document.addEventListener('dragover',e=>{
    if(!isTorrentDrag(e)){return;}
    e.preventDefault();
    e.dataTransfer.dropEffect='copy';
  });
  document.addEventListener('drop',e=>{
    e.preventDefault();
    dragCount=0;
    const ov=overlay();
    if(ov){ov.classList.remove('active');}
    const files=e.dataTransfer&&e.dataTransfer.files;
    if(!files||files.length===0){return;}
    let torrentFile=null;
    for(let i=0;i<files.length;i+=1){
      if(files[i].name.endsWith('.torrent')){torrentFile=files[i];break;}
    }
    if(!torrentFile){return;}
    const modal=document.getElementById('addModal');
    if(!modal||!modal.classList.contains('open')){openAdd();}
    const fileInput=document.getElementById('torrentFile');
    if(fileInput){
      const dt=new DataTransfer();
      dt.items.add(torrentFile);
      fileInput.files=dt.files;
      updateDropZoneState();
      handleTorrentInputChange();
    }
  });
  const dzEl=document.getElementById('dropZone');
  if(dzEl){
    dzEl.addEventListener('dragenter',e=>{e.preventDefault();dzEl.classList.add('drag-over');});
    dzEl.addEventListener('dragleave',e=>{dzEl.classList.remove('drag-over');});
    dzEl.addEventListener('dragover',e=>{e.preventDefault();e.dataTransfer.dropEffect='copy';});
    dzEl.addEventListener('drop',e=>{dzEl.classList.remove('drag-over');});
  }
})();
document.addEventListener('click',e=>{
  const target=e.target;
  const mainTabBtn=target&&target.closest('[data-main-tab-target]');
  if(mainTabBtn){
    const tab=mainTabBtn.dataset.mainTabTarget==='search'?'search':'library';
    try{localStorage.setItem(mainTabKey,tab);}catch(e){}
    applyMainTab(tab);
    if(tab==='search'){
      loadSearchCatalog(true).catch(showActionError);
    }
    return;
  }
  const searchViewBtn=target&&target.closest('[data-search-view-target]');
  if(searchViewBtn){
    const view=searchViewBtn.dataset.searchViewTarget==='plugins'?'plugins':'results';
    applySearchPanelView(view);
    return;
  }
  const searchSortBtn=target&&target.closest('[data-search-sort]');
  if(searchSortBtn){
    setSearchSort(searchSortBtn.dataset.searchSort||'seeds');
    return;
  }
  if(target&&target.closest('#themeToggle')){
    let theme=activeTheme||resolveTheme();
    theme=theme==='dark'?'light':'dark';
    try{localStorage.setItem(themeKey,theme);}catch(e){}
    applyTheme(theme);
  }
  const navItem=target.closest('.nav-item[data-filter]');
  if(navItem){
    const filter=navItem.dataset.filter||'all';
    try{localStorage.setItem(filterKey,filter);}catch(e){}
    applyFilter(filter);
  }
  const actionBtn=target.closest('[data-action]');
  if(actionBtn){
    const action=actionBtn.dataset.action;
    if(action==='toggle-panel'){
      const name=actionBtn.dataset.panel||'';
      try{
        const collapsed=isPanelCollapsed(name);
        localStorage.setItem(panelCollapseKey(name),collapsed?'0':'1');
      }catch(e){}
      applyPanelState();
      return;
    }
    if(action==='search-remove-plugin'){
      removeSearchPlugin(actionBtn.dataset.module||'').catch(showActionError);
      return;
    }
    if(action==='search-install-catalog'){
      const url=actionBtn.dataset.url||'';
      if(!url){return;}
      actionBtn.disabled=true;
      const origLabel=actionBtn.textContent;
      actionBtn.textContent='Installing\u2026';
      installCatalogPlugin(url).catch(showActionError).finally(()=>{actionBtn.disabled=false;actionBtn.textContent=origLabel;});
      return;
    }
    if(action==='search-add-result'){
      addSearchResult(actionBtn.dataset.index||'',actionBtn.dataset.name||'Search result').catch(showActionError);
      return;
    }
    const card=actionBtn.closest('.torrent-card');
    const id=card?card.dataset.id:'';
    if(!id){return;}
    const paused=card.dataset.paused==='true';
      if(action==='toggle-pause'){togglePause(id,paused).catch(showActionError);}
      else if(action==='stop'){torrentAction('stop',id).catch(showActionError);}
      else if(action==='archive'){torrentAction('archive',id).catch(showActionError);}
      else if(action==='delete'){confirmDelete(id,card.dataset.name||'torrent').catch(showActionError);}
      else if(action==='open-folder'){torrentAction('open-folder',id).catch(showActionError);}
      else if(action==='recheck'){torrentAction('recheck',id).catch(showActionError);}
    else if(action==='toggle-expand'){toggleExpand(card);}
    else if(action==='add-tracker'){
      const input=card.querySelector('.tracker-add-input');
      if(input&&input.value.trim()){
        apiPost('/torrent/add-tracker',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:'id='+encodeURIComponent(id)+'&url='+encodeURIComponent(input.value.trim())}).then(()=>{input.value='';}).catch(showActionError);
      }
    }
    else if(action==='remove-tracker'){
      const url=actionBtn.dataset.url||'';
      if(url){apiPost('/torrent/remove-tracker',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:'id='+encodeURIComponent(id)+'&url='+encodeURIComponent(url)}).catch(showActionError);}
    }
    else if(action==='set-label'){
      const input=card.querySelector('.label-input');
      if(input){apiPost('/torrent/set-label',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:'id='+encodeURIComponent(id)+'&label='+encodeURIComponent(input.value.trim())}).catch(showActionError);}
    }
  }
});
document.addEventListener('input',e=>{
  const target=e.target;
  if(target&&target.id==='librarySearch'){
    const search=String(target.value||'');
    try{localStorage.setItem(searchKey,search);}catch(e){}
    applyFilter(resolveFilter());
    return;
  }
  if(target&&target.id==='searchCatalogFilter'){
    renderSearchCatalog(searchCatalogCache);
    return;
  }
  if(target&&(target.id==='downloadLimit'||target.id==='uploadLimit')){
    updateRateLimitLabels();
  }
  if(target&&target.id==='seedRatio'){
    const v=Number(target.value)/10;
    const label=document.getElementById('seedRatioValue');
    if(label){label.textContent=v>0?v.toFixed(2):'unlimited';}
  }
});
document.addEventListener('change',e=>{
  const target=e.target;
  if(target&&target.matches('input[data-search-plugin]')){
    persistSearchPluginSelectionFromDom();
    return;
  }
  if(target&&target.id==='searchCategory'){
    saveSearchCategory(target.value||'all');
    return;
  }
  if(target&&(target.id==='downloadLimit'||target.id==='uploadLimit')){
    setGlobalRateLimits().catch(showActionError);
  }
  if(target&&target.id==='seedRatio'){
    const v=Number(target.value)/10;
    const body='ratio='+encodeURIComponent(v);
    apiPost('/settings/seed-ratio',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:body}).catch(showActionError);
  }
  if(target&&target.id==='peerProfile'){
    setPeerProfile().catch(showActionError);
  }
});
function toggleExpand(card){
  if(!card){return;}
  const collapsed=card.dataset.collapsed==='true';
  const nextCollapsed=!collapsed;
  card.dataset.collapsed=nextCollapsed?'true':'false';
  const toggle=card.querySelector("[data-action='toggle-expand']");
  if(toggle){toggle.innerHTML=nextCollapsed?'<svg class="material-symbols-rounded"><use href=#i-unfold_more></use></svg>Expand':'<svg class="material-symbols-rounded"><use href=#i-unfold_less></use></svg>Collapse';toggle.setAttribute('aria-expanded',nextCollapsed?'false':'true');}
  try{localStorage.setItem(collapseKey(card.dataset.infoHash||''),nextCollapsed?'1':'0');}catch(e){}
}
async function setGlobalRateLimits(){
  const down=document.getElementById('downloadLimit');
  const up=document.getElementById('uploadLimit');
  if(!down||!up){return;}
  const downloadKbps=Math.max(0,Math.min(MAX_RATE_LIMIT_KBPS,Math.round(Number(down.value)||0)));
  const uploadKbps=Math.max(0,Math.min(MAX_RATE_LIMIT_KBPS,Math.round(Number(up.value)||0)));
  down.value=String(downloadKbps);
  up.value=String(uploadKbps);
  updateRateLimitLabels();
  const body='download_kbps='+encodeURIComponent(downloadKbps)+'&upload_kbps='+encodeURIComponent(uploadKbps);
  await apiPost('/rate-limits',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:body});
}
async function setPeerProfile(){
  const select=document.getElementById('peerProfile');
  if(!select){return;}
  const profile=String(select.value||'balanced').trim().toLowerCase();
  const body='profile='+encodeURIComponent(profile);
  await apiPost('/settings/peer-profile',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:body});
}
async function submitAdd(){
  const fileInput=document.getElementById('torrentFile');
  const file=fileInput&&fileInput.files?fileInput.files[0]:null;
  const magnet=document.getElementById('magnet').value.trim();
  const dir=document.getElementById('downloadDir').value.trim();
  const prealloc=document.getElementById('preallocate').checked?'1':'0';
  const startWhenAdded=document.getElementById('startWhenAdded').checked;
  const addBtn=document.querySelector('#addModal .btn.primary');
  if(addBtn){addBtn.disabled=true;addBtn.textContent='Adding...';}
  try{
    if(file){
      if(addDraft.parsing){
        showAddError('Please wait until torrent metadata is loaded.');
        return false;
      }
      if(addDraft.kind==='file'&&addDraft.files.length>0&&addDraft.files.every(file=>!file.selected)){
        showAddError('Select at least one file to download.');
        return false;
      }
      const bytes=(addDraft.kind==='file'&&addDraft.bytes)?addDraft.bytes:new Uint8Array(await file.arrayBuffer());
      const postPlan={
        torrentId:null,
        infoHash:addDraft.infoHash||'',
        skipFiles:(addDraft.kind==='file'&&addDraft.files.length>0)?addDraft.files.filter(file=>!file.selected).map(file=>file.index):[],
        startPaused:!startWhenAdded,
      };
      const addResponse=await apiPostJson('/add-torrent?dir='+encodeURIComponent(dir)+'&prealloc='+prealloc+'&paused='+(startWhenAdded?'0':'1')+'&skip='+encodeURIComponent(postPlan.skipFiles.join(',')),{headers:{'Content-Type':'application/x-bittorrent'},body:bytes});
      const torrentId=Number(addResponse&&addResponse.torrent_id);
      postPlan.torrentId=Number.isFinite(torrentId)&&torrentId>0?torrentId:null;
      closeAdd();
      showToast(
        'Torrent added',
        (!startWhenAdded)
          ? ((addDraft.name||file.name||'Torrent')+' was added and will be paused.')
          : ((addDraft.name||file.name||'Torrent')+' was added to rustorrent.')
      );
      scheduleRefreshFallback();
      return false;
    }
    if(magnet){
      const infoHash=extractInfoHashFromMagnet(magnet);
      const body='magnet='+encodeURIComponent(magnet)+'&dir='+encodeURIComponent(dir)+'&prealloc='+prealloc+'&paused='+(startWhenAdded?'0':'1');
      const addResponse=await apiPostJson('/add-magnet',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:body});
      const torrentId=Number(addResponse&&addResponse.torrent_id);
      closeAdd();
      showToast(
        'Magnet added',
        !startWhenAdded
          ? 'The magnet was added paused. Resume when you are ready.'
          : 'The magnet was added to rustorrent.'
      );
      scheduleRefreshFallback();
      return false;
    }
    showAddError('Select a .torrent file or paste a magnet link.');
    return false;
  }catch(err){
    showAddError('Could not add torrent: '+actionErrorMessage(err));
    return false;
  }finally{
    if(addBtn){addBtn.disabled=false;addBtn.textContent='Add Torrent';}
  }
}

function scheduleRefreshFallback(){
  setTimeout(()=>{
    const stale=Date.now()-lastUpdateAt>1200;
    if(stale&&(!source||source.readyState!==1)){
      location.reload();
    }
  },1200);
}
async function torrentAction(action,id){
  await apiPost('/torrent/'+action+'?id='+encodeURIComponent(id));
}
async function togglePause(id,paused){
  const action=paused?'resume':'pause';
  await torrentAction(action,id);
}
async function confirmDelete(id,name){
  let dialog=document.getElementById('removeDialog');
  if(!dialog){
    dialog=document.createElement('dialog');dialog.id='removeDialog';dialog.className='remove-dialog';
    dialog.setAttribute('aria-labelledby','removeTitle');
    dialog.innerHTML='<h2 id="removeTitle">Remove transfer?</h2><p id="removeName"></p><label><input id="removeFiles" type="checkbox">Also delete downloaded files</label><div class="modal-actions"><button class="btn ghost" id="removeCancel">Cancel</button><button class="btn danger" id="removeConfirm">Remove transfer</button></div>';
    document.body.appendChild(dialog);
    if(typeof dialog.showModal!=='function'){
      dialog.setAttribute('role','dialog');dialog.setAttribute('aria-modal','true');
      dialog.showModal=()=>{dialog.open=true;dialog.setAttribute('open','');dialog.style.cssText='position:fixed;inset:0;z-index:200;height:max-content;display:block;box-shadow:0 0 0 100vmax rgba(15,23,42,.45)';};
      dialog.close=()=>{dialog.open=false;dialog.removeAttribute('open');dialog.style.display='none';dialog.onclose?.();};
      dialog.addEventListener('keydown',event=>{
        if(event.key==='Escape'){event.preventDefault();dialog.close();}
        if(event.key==='Tab'){
          const items=Array.from(dialog.querySelectorAll('button:not(:disabled),input:not(:disabled)'));
          if(event.shiftKey&&document.activeElement===items[0]){event.preventDefault();items[items.length-1].focus();}
          else if(!event.shiftKey&&document.activeElement===items[items.length-1]){event.preventDefault();items[0].focus();}
        }
      });
    }
  }
  const returnFocus=document.activeElement;
  dialog.querySelector('#removeName').textContent=name+' will be removed from your library. Files are kept unless you select the option below.';
  dialog.querySelector('#removeFiles').checked=false;
  dialog.querySelector('#removeCancel').onclick=()=>dialog.close();
  const button=dialog.querySelector('#removeConfirm');button.disabled=false;
  button.onclick=async()=>{
    button.disabled=true;
    try{await apiPost('/torrent/delete?id='+encodeURIComponent(id)+'&data='+(dialog.querySelector('#removeFiles').checked?'1':'0'));dialog.close();showToast('Transfer removed','The library will update shortly.');}
    catch(err){showActionError(err);button.disabled=false;}
  };
  dialog.onclose=()=>{if(pendingHtml){const html=pendingHtml;pendingHtml=null;renderApp(html);}if(returnFocus?.isConnected){returnFocus.focus();}};
  dialog.showModal();dialog.querySelector('#removeCancel').focus();
}
async function setPriorityRequest(torrentId,index,priority){
  const body='id='+encodeURIComponent(torrentId)+'&index='+encodeURIComponent(index)+'&priority='+encodeURIComponent(priority);
  await apiPost('/file-priority',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:body});
}
async function setPriority(torrentId,index,priority){
  try{
    await setPriorityRequest(torrentId,index,priority);
  }catch(err){
    alert('Priority update failed: '+actionErrorMessage(err));
  }
}
function startRename(torrentId,index,cell){
  const oldText=cell.textContent;
  const parts=oldText.split('/');
  const basename=parts[parts.length-1];
  const input=document.createElement('input');
  input.type='text';input.className='input';input.value=basename;
  input.style.cssText='width:100%;box-sizing:border-box;font-size:12px';
  cell.textContent='';cell.appendChild(input);input.focus();input.select();
  let done=false;
  function finish(save){
    if(done)return;done=true;
    const val=input.value.trim();
    cell.textContent=oldText;
    if(save&&val&&val!==basename&&!val.includes('/')&&!val.includes('\\\\')&&val!=='.'&&val!=='..'){
      const newPath=parts.length>1?parts.slice(0,-1).join('/')+'/'+val:val;
      cell.textContent=newPath;
      const body='id='+encodeURIComponent(torrentId)+'&index='+encodeURIComponent(index)+'&name='+encodeURIComponent(val);
      apiPost('/rename-file',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:body}).catch(function(err){
        cell.textContent=oldText;
        alert('Rename failed: '+actionErrorMessage(err));
      });
    }
  }
  input.addEventListener('keydown',function(e){if(e.key==='Enter'){finish(true)}else if(e.key==='Escape'){finish(false)}});
  input.addEventListener('blur',function(){finish(true)});
}
function addRssFeed(e){
  e.preventDefault();
  const url=document.getElementById('rssUrl').value.trim();
  if(!url)return;
  apiPost('/rss/add-feed',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:'url='+encodeURIComponent(url)}).then(()=>location.reload()).catch(err=>alert('Add feed failed: '+actionErrorMessage(err)));
}
function removeRssFeed(url){
  apiPost('/rss/remove-feed',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:'url='+encodeURIComponent(url)}).then(()=>location.reload()).catch(err=>alert('Remove feed failed: '+actionErrorMessage(err)));
}
function removeRssRule(name){
  apiPost('/rss/remove-rule',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:'name='+encodeURIComponent(name)}).then(()=>location.reload()).catch(err=>alert('Remove rule failed: '+actionErrorMessage(err)));
}
function addRssRule(e){
  e.preventDefault();
  var name=document.getElementById('rssRuleName').value.trim();
  var pattern=document.getElementById('rssRulePattern').value.trim();
  if(!name||!pattern)return;
  var body='name='+encodeURIComponent(name)+'&pattern='+encodeURIComponent(pattern);
  apiPost('/rss/add-rule',{headers:{'Content-Type':'application/x-www-form-urlencoded'},body:body}).then(function(){location.reload()}).catch(function(err){alert('Add rule failed: '+actionErrorMessage(err))});
}
function enhanceUI(){
  const labels={librarySearch:'Filter transfers',searchQuery:'Search torrents',searchCategory:'Search category',searchPluginUrl:'Plugin URL',searchCatalogFilter:'Filter plugins',rssUrl:'RSS feed URL',rssRuleName:'Rule name',rssRulePattern:'Rule pattern',magnet:'Magnet link',downloadDir:'Save to folder',torrentFile:'Torrent file',downloadLimit:'Download limit',uploadLimit:'Upload limit',seedRatio:'Seeding ratio limit',peerProfile:'Connection profile'};
  Object.entries(labels).forEach(([id,label])=>document.getElementById(id)?.setAttribute('aria-label',label));
  const modal=document.getElementById('addModal');if(modal){modal.setAttribute('role','dialog');modal.setAttribute('aria-modal','true');modal.setAttribute('aria-label','Add Torrent');}
  const zone=document.getElementById('dropZone');if(zone){
    const browse=zone.querySelector('.dz-text');
    if(browse){browse.tabIndex=0;browse.setAttribute('role','button');browse.setAttribute('aria-label','Choose torrent file');browse.onkeydown=e=>{if(e.key==='Enter'||e.key===' '){e.preventDefault();document.getElementById('torrentFile')?.click();}};}
  }
  document.querySelectorAll('.table select').forEach(node=>node.setAttribute('aria-label','File priority'));
  document.querySelectorAll('.tracker-add-input').forEach(node=>node.setAttribute('aria-label','Tracker URL'));
  document.querySelectorAll('.label-input').forEach(node=>node.setAttribute('aria-label','Transfer label'));
}
enhanceUI();
applyMainTab(resolveMainTab());
applyTheme(resolveTheme());
applySearchPanelView(resolveSearchPanelView());
activeSearchSort=resolveSearchSort();
applyFilter(resolveFilter());
applyCollapseState();
applyPanelState();
updateRateLimitLabels();
renderAddReview();
const searchCategoryInput=document.getElementById('searchCategory');
if(searchCategoryInput){searchCategoryInput.value=resolveSearchCategory();}
loadSearchStatus(false).catch(err=>console.warn('search status failed',err));
let lastUpdateAt=Date.now();
const source=new EventSource('/events');
source.addEventListener('status',event=>{if(event&&event.data){applyUpdate(event.data);lastUpdateAt=Date.now();}});
const connectionBanner=document.createElement('div');connectionBanner.className='connection-banner';connectionBanner.hidden=true;connectionBanner.setAttribute('role','status');connectionBanner.textContent='Connection lost. Reconnecting to Rustorrent…';document.body.appendChild(connectionBanner);
source.onopen=()=>{connectionBanner.hidden=true;};
source.onerror=()=>{connectionBanner.hidden=false;};
