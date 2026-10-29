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
  assert.deepEqual(Object.keys(context.DotLog).sort(),['getDay','onAction','render','storage','today']);
  const request=context.DotLog.getDay('2026-10-29');
  assert.equal(sent[0].method,'day.get');
  assert.equal(sent[0].params.date,'2026-10-29');
  assert.equal(sent[0].pluginId,undefined);
  context.onmessage({data:{type:'rpc-result',id:sent[0].id,error:'未授权'}});
  await assert.rejects(request,/未授权/);
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
