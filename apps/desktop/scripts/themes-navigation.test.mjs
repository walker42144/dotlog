import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import ts from 'typescript';
async function load(name) {
  const source = await readFile(new URL(`../src/lib/${name}.ts`, import.meta.url), 'utf8');
  const exports = {};
  vm.runInNewContext(ts.transpileModule(source, {compilerOptions:{module:ts.ModuleKind.CommonJS}}).outputText, {exports});
  return exports;
}
const themes = await load('themes');
const {pluginNavigation} = await load('plugin-navigation');
test('the local plugin exclusively owns its sidebar visibility and legacy preferences cannot restore it', () => {
  const builtin=[{id:'memos',name:'Memos',installed:true,enabled:true},{id:'tomatodo',name:'番茄Todo',installed:true,enabled:true}];
  const plugin={package:{id:'local-tomatodo-link',name:'番茄联动'},enabled:true,showInSidebar:true};
  assert.deepEqual(JSON.parse(JSON.stringify(pluginNavigation(builtin,[plugin],{tomatodo:true}))),[{id:'memos',name:'Memos'},{id:'local-tomatodo-link',name:'番茄联动'}]);
  for(const local of [[],[{...plugin,showInSidebar:false}],[{...plugin,enabled:false}]])
    assert.equal(pluginNavigation(builtin,local,{tomatodo:true}).some(p=>p.id.includes('tomatodo')),false);
  assert.equal(pluginNavigation(builtin,[plugin],{tomatodo:false}).some(p=>p.id==='local-tomatodo-link'),true);
});
test('every saved theme survives bootstrap before React mounts', async () => {
  const bootstrap = await readFile(new URL('../public/theme-init.js',import.meta.url),'utf8');
  for (const {value} of themes.THEMES) for (const dark of [true,false]) {
    const document = {documentElement:{dataset:{}}};
    vm.runInNewContext(bootstrap,{document,localStorage:{getItem:()=>value},window:{matchMedia:()=>({matches:dark})}});
    assert.equal(document.documentElement.dataset.theme,themes.resolveTheme(value,dark));
  }
  assert.equal(themes.resolveTheme('missing',true),'dark');
  assert.equal(themes.themeMode('forest'),'dark');
  assert.equal(themes.themeMode('rose'),'light');
});
test('all palettes keep readable text and primary button contrast', async () => {
  const tokens=await readFile(new URL('../src/styles/tokens.css',import.meta.url),'utf8');
  const css=await readFile(new URL('../src/styles/themes.css',import.meta.url),'utf8');
  const vars=s=>Object.fromEntries([...s.matchAll(/(--[\w-]+):\s*(#[a-f\d]{6})\s*;/gi)].map(m=>[m[1],m[2]]));
  const light=vars(tokens.slice(0,tokens.indexOf(':root[data-theme'))), dark=vars(tokens.slice(tokens.indexOf(':root[data-theme')));
  const luminance=color=>color.slice(1).match(/../g).map(c=>parseInt(c,16)/255).map(v=>v<=.04045?v/12.92:((v+.055)/1.055)**2.4).reduce((sum,v,i)=>sum+v*[.2126,.7152,.0722][i],0);
  const ratio=(a,b)=>{const x=luminance(a),y=luminance(b);return (Math.max(x,y)+.05)/(Math.min(x,y)+.05)};
  for(const {value} of themes.THEMES.filter(t=>t.value!=='system')) {
    const override=css.match(new RegExp(`:root\\[data-theme="${value}"\\] \\{([^}]+)\\}`));
    const palette={...light,...(themes.themeMode(value)==='dark'?dark:{}),...vars(override?.[1]??'')};
    for(const [fg,bg] of [['--text','--surface'],['--text-muted','--surface-raised'],['--brand-contrast','--brand']])
      assert.ok(ratio(palette[fg],palette[bg])>=4.5,`${value}: ${fg} / ${bg}`);
  }
});

const {validPluginView}=await load('plugin-view');
test('worker view rejects malformed tab metadata, duplicate field keys and invalid input types', () => {
  assert.equal(validPluginView({blocks:[{type:'text',text:'正常'}]}),true);
  for(const view of [
    {blocks:[],activeTab:{}}, {blocks:[],activeTab:'__proto__'}, {blocks:[],title:{}},
    {blocks:[{type:'input',id:'same'},{type:'textarea',id:'same'}]},
    {blocks:[{type:'input',inputType:'file'}]},
    {blocks:[],tabs:[{id:'a',label:'A',action:'a'}],activeTab:'missing'}
  ]) assert.equal(validPluginView(view),false);
});
test('motion preference survives bootstrap and invalid values fall back safely', async () => {
  const bootstrap=await readFile(new URL('../public/theme-init.js',import.meta.url),'utf8');
  for(const value of ['system','reduced','invalid']) {
    const document={documentElement:{dataset:{}}};
    vm.runInNewContext(bootstrap,{document,localStorage:{getItem:key=>key==='dotlog.motion'?value:'system'},window:{matchMedia:()=>({matches:false})}});
    assert.equal(document.documentElement.dataset.motion,value==='reduced'?'reduced':'system');
  }
});
