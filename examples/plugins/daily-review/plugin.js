let date = DotLog.today();
let notes = {};
async function show(message = '') {
  try {
    const day = await DotLog.getDay(date);
    const seconds = day.stats.recordedSeconds;
    const items = day.segments.filter(s => !s.isIgnored).map(s => `- ${s.title || '未命名记录'} · ${Math.round(s.durationSeconds / 60)} 分钟`);
    DotLog.render({title: '今天做了什么，明天如何改进？', blocks: [
      {type:'input',id:'date',label:'记录日期',inputType:'date',value:date},
      {type:'button',text:'查看所选日期',action:'load'},
      {type:'stat',label:'有效记录（不含暂停）',value:`${Math.floor(seconds/3600)} 小时 ${Math.floor(seconds%3600/60)} 分`},
      {type:'markdown',text:items.join('\n') || '这一天还没有记录。'},
      {type:'textarea',id:`review-${date}`,label:'收获、问题与下一步',value:notes[date] || ''},
      {type:'button',text:'保存复盘',action:'save'},
      {type:'text',text:message || '复盘只保存在本插件中，不会上传，也不会修改计时记录。'}
    ]});
  } catch (error) {
    DotLog.render({blocks:[{type:'text',text:`读取失败：${error.message}`},{type:'button',text:'重试',action:'load'}]});
  }
}
DotLog.onAction(async (action, values) => {
  if (action === 'save') {
    notes[date] = values[`review-${date}`] || '';
    await DotLog.storage.set(notes);
    await show('复盘已保存。');
  } else {
    date = values.date || date;
    await show();
  }
});
(async () => { notes = await DotLog.storage.get() || {}; await show(); })()
  .catch(error => DotLog.render({blocks:[{type:'text',text:`启动失败：${error.message}`}]}));
