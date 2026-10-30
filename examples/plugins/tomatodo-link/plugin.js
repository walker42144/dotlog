let snapshot=null, error='', busy=false, rendering=false, stateReadable=false, tab='live';
let scanned=null, imported=null, compared=null, analysis=null, confirmImport=false;
const phases={unknown:'未知',idle:'空闲',focus:'专注中',paused:'已暂停',rest:'休息中'};
const localTime=ms=>new Date(ms).toLocaleString('sv-SE');
const text=value=>({type:'text',text:String(value)});
const stat=(label,value)=>({type:'stat',label,value:String(value)});
const button=(label,action,disabled=false)=>({type:'button',text:label,action,disabled:busy||disabled});
function liveBlocks(){
  if(!snapshot)return [text(error||'正在读取连接状态…')];
  const c=snapshot.context, connected=stateReadable&&snapshot.enabled&&c.connected;
  const blocks=[stat('连接状态',connected?'已连接':'未连接'),stat('当前任务',c.session?.title||'暂无专注任务'),stat('番茄状态',connected?(phases[c.source_phase]||'未知'):'—')];
  if(error||snapshot.error)blocks.push(text(error||snapshot.error));
  if(c.conflict)blocks.push(text('需要核对：'+c.conflict));
  if(c.gap_detected)blocks.push(text('曾断开连接，请核对记录的截止时间。'));
  if(snapshot.enabled){
    if(c.active_segment_id)blocks.push(button('结束本段','end'));
    else if(connected&&c.source_phase==='focus'&&c.session)blocks.push(button('开始下一段','next'));
    if(!connected)blocks.push(button('重新连接','enable'));
    blocks.push(button('断开连接','disable',!!c.active_segment_id));
    blocks.push(text('自动连接已开启。'+(c.active_segment_id?'先结束本段，再断开连接。':'')));
  }else blocks.push(button('连接番茄 ToDo','enable'));
  if(snapshot.enabled&&(c.conflict||(!c.connected&&c.active_segment_id))){
    blocks.push({type:'input',id:'reconcile-time',label:'确认截止时间',value:localTime(snapshot.now)},
      text('按填写时间结束旧分段，保留待补记记录，再关联当前任务。'),button('确认并重新关联','reconcile'));
  }
  blocks.push({type:'heading',text:'待补记'+(snapshot.pendingNotes.length?'（'+snapshot.pendingNotes.length+'）':'')});
  if(!snapshot.pendingNotes.length)blocks.push(text('没有待补记记录。'));
  for(const n of snapshot.pendingNotes.slice(0,8)){
    if(n.missing){blocks.push(text(n.title),button('移出队列','skip:'+n.id));continue;}
    blocks.push(text((n.title||'未命名记录')+'　'+localTime(n.startAt)+' — '+localTime(n.endAt)),
      {type:'textarea',id:'note:'+n.id,label:'备注',value:n.note||''},button('保存备注','save:'+n.id),button('跳过补记','skip:'+n.id));
  }
  if(snapshot.pendingNotes.length>8)blocks.push(text('处理完这 8 段后显示其余记录。'));
  blocks.push(button('刷新','refresh'));
  return blocks;
}
function historyBlocks(){
  const blocks=[text('扫描番茄 ToDo 历史数据后，可增量导入或与 DotLog 对比。已通过实时联动记录的专注不会重复导入。'),button('扫描历史数据','scan'),button('对比记录','compare'),button('AI 分析差异','analyze')];
  if(error)blocks.push(text(error));
  if(scanned){
    blocks.push(stat('已完成专注',scanned.completedRecords+' 条'),stat('累计时长',scanned.totalMinutes+' 分钟'));
    for(const row of scanned.preview.slice(0,6))blocks.push(text(row.name+'　'+row.startText+'　'+row.durationMinutes+' 分钟'));
    blocks.push(button('导入已完成记录','request-import'));
  }
  if(confirmImport)blocks.push(text('导入无冲突的已完成记录，跳过重复记录。重叠记录会列为冲突，不覆盖 DotLog。'),button('确认导入','import'),button('取消','cancel-import'));
  if(imported){blocks.push(text('导入 '+imported.imported+' 条；已存在 '+imported.skippedExisting+' 条；未完成 '+imported.skippedIncomplete+' 条；冲突 '+imported.conflicts+' 条。'));for(const e of imported.errors.slice(0,5))blocks.push(text(e));}
  if(compared){
    const s=compared.summary;
    blocks.push({type:'heading',text:'对比结果'},text('番茄 '+s.tomatodoMinutes+' 分钟　DotLog '+s.dotlogMinutes+' 分钟　匹配 '+s.matchedPairs+' 对'),text('DotLog 缺少 '+s.missingInDotlog+' 条，多出 '+s.extraInDotlog+' 条。'));
    for(const p of compared.pairs.slice(0,8))blocks.push(text((p.tomatodo?.name||'番茄无对应记录')+' / '+(p.dotlogSegments.map(s=>s.title||'未命名').join('、')||'DotLog 无对应记录')));
  }
  if(analysis){blocks.push({type:'heading',text:'分析建议'});if(!analysis.length)blocks.push(text('没有可分析的配对记录。'));for(const a of analysis.slice(0,8))blocks.push(text(a.tomatodoSubject+'：'+a.suggestion+' '+a.reason));}
  return blocks;
}
async function render(){
  if(rendering)return;rendering=true;
  try{
    if(tab==='live')try{snapshot=await DotLog.link.status();stateReadable=true;}catch(e){stateReadable=false;error=String(e.message||e);}
    DotLog.render({title:'番茄联动',tabs:[{id:'live',label:'实时记录',action:'tab:live'},{id:'history',label:'历史数据',action:'tab:history'}],activeTab:tab,blocks:tab==='live'?liveBlocks():historyBlocks()});
  }finally{rendering=false;}
}
DotLog.onAction(async(action,values)=>{
  if(busy)return;
  if(action.startsWith('tab:')){tab=action.slice(4);error='';await render();return;}
  busy=true;error='';await render();
  try{
    if(action==='enable')await DotLog.link.enable();
    else if(action==='disable')await DotLog.link.disable();
    else if(action==='end')await DotLog.link.endSegment();
    else if(action==='next')await DotLog.link.startNext();
    else if(action==='scan')scanned=await DotLog.tomatodo.scan();
    else if(action==='compare')compared=await DotLog.tomatodo.compare();
    else if(action==='analyze')analysis=await DotLog.tomatodo.analyze();
    else if(action==='request-import')confirmImport=true;
    else if(action==='cancel-import')confirmImport=false;
    else if(action==='import'){imported=await DotLog.tomatodo.import();confirmImport=false;}
    else if(action==='reconcile'){
      const endAt=new Date(String(values['reconcile-time']||'').replace(' ','T')).getTime();
      if(!Number.isFinite(endAt))throw new Error('请输入有效截止时间');
      await DotLog.link.reconcile({endAt});
    }else if(action.startsWith('save:')||action.startsWith('skip:')){
      const id=action.slice(5),n=snapshot.pendingNotes.find(n=>n.id===id);
      if(!n)throw new Error('记录已变化，请刷新');
      const params={id,revision:n.revision,note:String(values['note:'+id]||'')};
      if(action.startsWith('save:'))await DotLog.link.saveNote(params);else await DotLog.link.skipNote(params);
    }
  }catch(e){error=String(e.message||e);}finally{busy=false;await render();}
});
void render();
setInterval(()=>{if(!busy)void render();},2000);
