import { readFile } from 'node:fs/promises';
import vm from 'node:vm';
import { test } from 'node:test';
import assert from 'node:assert/strict';
const source = await readFile(new URL('../src/components/PluginPage.tsx',import.meta.url),'utf8');
const bootstrap = source.match(/const BOOTSTRAP = `([\s\S]*?)`;/)[1];
function fixture(){
  const sent=[];
  const context=vm.createContext({setTimeout,clearTimeout,postMessage:m=>sent.push(m)});
  new vm.Script(bootstrap).runInContext(context,{timeout:1000});
  return {sent,context};
}
test('worker SDK only exposes scoped data APIs and propagates host rejection',async()=>{
  const {sent,context}=fixture();
  assert.deepEqual(Object.keys(context.DotLog).sort(),['getDay','link','onAction','render','storage','today','tomatodo']);
  const request=context.DotLog.getDay('2026-10-29');
  assert.equal(sent[0].method,'day.get');
  assert.equal(sent[0].params.date,'2026-10-29');
  assert.equal(sent[0].pluginId,undefined);
  context.onmessage({data:{type:'rpc-result',id:sent[0].id,error:'未授权'}});
  await assert.rejects(request,/未授权/);
});
test('link SDK exposes only scoped actions and retains revision in note requests',async()=>{
  const {sent,context}=fixture();
  assert.equal(Object.isFrozen(context.DotLog.link),true);
  assert.equal(context.DotLog.link.observe,undefined);
  assert.equal(context.DotLog.link.sql,undefined);
  const pending=context.DotLog.link.saveNote({id:'owned-record',revision:3,note:'备注'});
  assert.equal(sent[0].method,'link.saveNote');
  assert.equal(sent[0].params.revision,3);
  context.onmessage({data:{type:'rpc-result',id:sent[0].id,error:'记录已变化'}});
  await assert.rejects(pending,/记录已变化/);
});
test('worker action receives form values and storage resolves with host data',async()=>{
  const {sent,context}=fixture();
  const request=context.DotLog.storage.get();
  context.onmessage({data:{type:'rpc-result',id:sent[0].id,result:{note:'已保存'}}});
  assert.equal((await request).note,'已保存');
  context.DotLog.onAction((action,values)=>context.DotLog.render({blocks:[{type:'text',text:`${action}:${values.note}`}]}));
  context.onmessage({data:{type:'action',action:'save',values:{note:'新的复盘'}}});
  await Promise.resolve();await Promise.resolve();
  assert.equal(sent.at(-1).type,'render');
  assert.equal(sent.at(-1).view.blocks[0].text,'save:新的复盘');
});

test('link panel provides an enable retry while background remains enabled', async()=>{
  const script=await readFile(new URL('../../../examples/plugins/tomatodo-link/plugin.js',import.meta.url),'utf8');
  let view, handler, enabled=0;
  const snapshot={enabled:true,error:'helper exited',now:Date.now(),context:{connected:false,source_phase:'unknown',pending_notes:[]},pendingNotes:[]};
  const context=vm.createContext({setInterval:()=>0,DotLog:{
    link:{status:async()=>snapshot,enable:async()=>{enabled++;}},
    render:v=>{view=v;},onAction:h=>{handler=h;}
  }});
  new vm.Script(script).runInContext(context,{timeout:1000});
  await new Promise(resolve=>setImmediate(resolve));
  assert.ok(view.blocks.some(b=>b.action==='enable'&&b.text.includes('重新连接')));
  await handler('enable',{});
  assert.equal(enabled,1);
});

test('disabled link panel never presents persisted connected state as live', async()=>{
  const script=await readFile(new URL('../../../examples/plugins/tomatodo-link/plugin.js',import.meta.url),'utf8');
  let view;
  const snapshot={enabled:false,error:null,now:Date.now(),context:{connected:true,source_phase:'focus',session:{title:'旧任务'}},pendingNotes:[]};
  const context=vm.createContext({setInterval:()=>0,DotLog:{link:{status:async()=>snapshot},render:v=>{view=v;},onAction:()=>{}}});
  new vm.Script(script).runInContext(context,{timeout:1000});
  await new Promise(resolve=>setImmediate(resolve));
  assert.ok(view.blocks.some(b=>b.label==='连接状态'&&b.value==='未连接'));
  assert.ok(!view.blocks.some(b=>b.text==='已连接番茄 ToDo'));
});

test('failed status refresh cannot keep a stale connected label', async()=>{
  const script=await readFile(new URL('../../../examples/plugins/tomatodo-link/plugin.js',import.meta.url),'utf8');
  let view,handler,failed=false;
  const snapshot={enabled:true,error:null,now:Date.now(),context:{connected:true,source_phase:'idle'},pendingNotes:[]};
  const context=vm.createContext({setInterval:()=>0,DotLog:{link:{status:async()=>{if(failed)throw Error('断开');return snapshot;}},render:v=>{view=v;},onAction:h=>{handler=h;}}});
  new vm.Script(script).runInContext(context,{timeout:1000});await new Promise(resolve=>setImmediate(resolve));
  assert.ok(view.blocks.some(b=>b.label==='连接状态'&&b.value==='已连接'));
  failed=true;await handler('refresh',{});
  assert.ok(view.blocks.some(b=>b.label==='连接状态'&&b.value==='未连接'));
});
test('independent history tab calls scoped APIs and requires an explicit import confirmation',async()=>{
  const script=await readFile(new URL('../../../examples/plugins/tomatodo-link/plugin.js',import.meta.url),'utf8');
  let view,handler,scans=0,imports=0;
  const context=vm.createContext({setInterval:()=>0,DotLog:{link:{status:async()=>({enabled:false,context:{connected:false},pendingNotes:[]})},tomatodo:{
    scan:async()=>{scans++;return {completedRecords:2,totalMinutes:30,preview:[]};},import:async()=>{imports++;return {imported:2,skippedExisting:0,skippedIncomplete:0,conflicts:0,errors:[]};}
  },render:v=>{view=v;},onAction:h=>{handler=h;}}});
  new vm.Script(script).runInContext(context,{timeout:1000});await new Promise(resolve=>setImmediate(resolve));
  assert.equal(scans,0);await handler('tab:history',{});assert.equal(view.activeTab,'history');
  await handler('scan',{});assert.equal(scans,1);
  await handler('request-import',{});assert.equal(imports,0);assert.ok(view.blocks.some(b=>b.action==='import'));
  await handler('import',{});assert.equal(imports,1);
});
