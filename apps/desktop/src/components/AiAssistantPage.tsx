import { useEffect, useRef, useState } from "react";
import { ipc, IS_TAURI, friendlyMessage, toIpcError, type AiReviewResult, type AiSuggestion, type CategoryView, type SegmentView } from "../lib/ipc";
import { dayKeyOf, parseDayKey, formatDuration } from "../lib/format";
import { PLUGINS_CHANGED } from "../lib/plugins";
import { IntegrationsPage } from "./IntegrationsPage";
import { MemoContent } from "./MemoContent";

type Draft = { suggestion: AiSuggestion; original: SegmentView; title: string; categoryId: string; tags: string };
export function AiAssistantPage({ onNotify, onDataChanged }: {
  onNotify: (message: string, tone?: "info" | "error") => void;
  onDataChanged: () => void;
}) {
  const [tab, setTab] = useState<"organize" | "review" | "settings">("organize");
  const [date,setDate] = useState(()=>dayKeyOf(Date.now()));
  const [from,setFrom] = useState(date), [to,setTo] = useState(date);
  const [question,setQuestion] = useState("");
  const [rows,setRows] = useState<Draft[]>([]), [categories,setCategories] = useState<CategoryView[]>([]);
  const [report,setReport] = useState<AiReviewResult|null>(null);
  const [busy,setBusy] = useState(""), [error,setError] = useState(""), [organized,setOrganized] = useState(false);
  const [config,setConfig] = useState<{enabled:boolean;model:string;privacy:string}|null>(null);
  useEffect(()=>{
    let live=true;
    const load=()=>{void Promise.all([ipc.integrationList(),ipc.settingsGet("plugin:ai")]).then(([plugins,s])=>{
      if(live)setConfig({enabled:plugins.some(p=>p.id==="ai"&&p.enabled),model:String(s.model||"未配置"),privacy:String(s.privacyMode||"title_only")});
    }).catch(e=>{if(live)setError(friendlyMessage(toIpcError(e)));});};
    load(); window.addEventListener(PLUGINS_CHANGED,load);
    return()=>{live=false;window.removeEventListener(PLUGINS_CHANGED,load);};
  },[]);
  const running = useRef(false);
  const ready=!!config?.enabled&&config.privacy!=="off";
  const run=async(key:string,fn:()=>Promise<void>)=>{
    if(running.current)return; running.current=true;setBusy(key);setError("");
    try{await fn();}catch(e){setError(friendlyMessage(toIpcError(e)));}finally{running.current=false;setBusy("");}
  };
  const organize=()=>run("organize",async()=>{
    setRows([]);setOrganized(false);
    const [day,cats]=await Promise.all([ipc.dayGet(date),ipc.categories()]);
    const suggestions=await ipc.aiSuggest(date);
    const byId=new Map(day.segments.map(s=>[s.id,s]));
    setCategories(cats);
    setRows(suggestions.flatMap(s=>{const original=s.original??byId.get(s.segmentId);return original?[{
      suggestion:s,original,title:s.title??original.title,categoryId:s.categoryId??"",tags:s.tags.join(", ")
    }]:[];}));
    setOrganized(true);
  });
  const update=(id:string,patch:Partial<Draft>)=>setRows(prev=>prev.map(r=>r.suggestion.segmentId===id?{...r,...patch}:r));
  const apply=(row:Draft)=>run(row.suggestion.segmentId,async()=>{
    await ipc.aiApply({segmentId:row.suggestion.segmentId,revision:row.suggestion.revision,
      title:row.title.trim()||undefined,categoryId:row.categoryId||undefined,
      tags:row.tags.split(/[,，、\n]/).map(t=>t.trim().replace(/^#+/,"")).filter(Boolean)});
    setRows(prev=>prev.filter(r=>r!==row));onDataChanged();onNotify("已应用这条建议");
  });
  const review=()=>run("review",async()=>{setReport(await ipc.aiReview({from,to,question}));});
  const quickRange=(days:number)=>{
    const today=dayKeyOf(Date.now()),start=parseDayKey(today);start.setDate(start.getDate()-days+1);
    setFrom(`${start.getFullYear()}-${String(start.getMonth()+1).padStart(2,"0")}-${String(start.getDate()).padStart(2,"0")}`);setTo(today);
  };
  const exportReport=()=>{
    if(!report)return;
    const content=`# DotLog 时间复盘\n\n${report.from} 至 ${report.to} · ${report.recordCount} 条记录 · ${formatDuration(report.recordedSeconds)}\n\n${report.markdown}\n`;
    const url=URL.createObjectURL(new Blob([content],{type:"text/markdown;charset=utf-8"}));
    const anchor=document.createElement("a");anchor.href=url;anchor.download=`DotLog-复盘-${report.from}-${report.to}.md`;anchor.click();setTimeout(()=>URL.revokeObjectURL(url),1000);
  };
  return <div className="settings-page ai-workspace">
    <header className="page-head"><h1>AI 助手</h1><p>整理记录、回顾时间，把下一步想清楚。</p></header>
    {!IS_TAURI&&<p className="badge">浏览器演示 · 模拟结果，不调用真实模型</p>}
    <div className="ai-status"><span>{config?.enabled?"已启用":"未启用"} · {config?.model||"读取配置中"}</span><span>{config?.privacy==="with_note"?"发送标题、时间、分类标签与备注":config?.privacy==="off"?"已关闭联网":"记录内容：标题、时间和分类标签"}</span></div>
    <div className="tomatodo-tabs" role="group" aria-label="AI 助手功能">
      {([['organize','整理记录'],['review','复盘与提问'],['settings','模型与连接']] as const).map(([key,label])=><button key={key} aria-pressed={tab===key} disabled={!!busy} onClick={()=>{setTab(key);setError("");}}>{label}</button>)}
    </div>
    {error&&<p role="alert" className="integration-card__error">{error}</p>}
    {!ready&&tab!=="settings"&&<div className="settings-group"><p>{config?.privacy==="off"?"当前隐私模式已关闭联网。":"请先配置并启用 AI 助手。"}</p><button className="ghost-btn" onClick={()=>setTab("settings")}>前往模型与连接</button></div>}
    {tab==="organize"&&<>
      <section className="settings-group ai-section"><h2>按日期整理</h2><p>为已结束且未忽略的记录生成标题、分类和标签建议。你可以编辑后采纳，也可以忽略。</p>
        <div className="integration-actions ai-organize-controls"><label className="field">记录日期<input className="field__input" type="date" value={date} disabled={!!busy} onChange={e=>{setDate(e.target.value);setRows([]);setOrganized(false);}} /></label><button className="primary-btn primary-btn--sm" disabled={!date||!ready||!!busy} onClick={()=>void organize()}>{busy==="organize"?"正在生成建议…":"获取整理建议"}</button></div>
      </section>
      {organized&&rows.length===0&&<p className="panel__empty">这次没有待处理的建议。可以换一天查看。</p>}
      {rows.map(row=><section className="settings-group ai-section" key={row.suggestion.segmentId}>
        <div className="ai-record-head"><strong>{row.original.title||"未命名记录"}</strong><span>{new Date(row.original.startAt).toLocaleTimeString([],{hour:'2-digit',minute:'2-digit'})} · {formatDuration(row.original.durationSeconds)}</span></div>
        <p className="ai-reason">{row.suggestion.reason}</p>
        <label className="field">建议标题<input className="field__input" maxLength={200} disabled={!!busy} value={row.title} onChange={e=>update(row.suggestion.segmentId,{title:e.target.value})}/></label>
        <div className="ai-edit-grid"><label className="field">建议分类<select className="field__input" disabled={!!busy} value={row.categoryId} onChange={e=>update(row.suggestion.segmentId,{categoryId:e.target.value})}><option value="">保留原分类{row.original.category?` · ${row.original.category.name}`:""}</option>{categories.map(c=><option key={c.id} value={c.id}>{c.name}</option>)}</select></label>
        <label className="field">追加标签（逗号分隔）<input className="field__input" maxLength={300} disabled={!!busy} value={row.tags} onChange={e=>update(row.suggestion.segmentId,{tags:e.target.value})}/></label></div>
        <div className="integration-actions"><button className="primary-btn primary-btn--sm" disabled={!!busy} onClick={()=>void apply(row)}>应用这条建议</button><button className="ghost-btn" disabled={!!busy} onClick={()=>setRows(prev=>prev.filter(r=>r!==row))}>忽略</button></div>
      </section>)}
    </>}
    {tab==="review"&&<>
      <section className="settings-group ai-section"><h2>回顾所选时间</h2><div className="integration-actions"><button className="ghost-btn" disabled={!!busy} onClick={()=>quickRange(1)}>今天</button><button className="ghost-btn" disabled={!!busy} onClick={()=>quickRange(7)}>最近 7 天</button><button className="ghost-btn" disabled={!!busy} onClick={()=>quickRange(30)}>最近 30 天</button></div>
        <div className="ai-edit-grid"><label className="field">开始日期<input type="date" className="field__input" value={from} disabled={!!busy} onChange={e=>setFrom(e.target.value)}/></label><label className="field">结束日期<input type="date" className="field__input" value={to} disabled={!!busy} onChange={e=>setTo(e.target.value)}/></label></div>
        <label className="field">关注的问题（可选）<textarea rows={3} className="field__input ai-question" maxLength={2000} disabled={!!busy} value={question} placeholder="例如：这周数学学习主要花在哪些活动上？下周可以怎么调整？" onChange={e=>setQuestion(e.target.value)}/></label>
        <p>最多 31 天、500 条已结束记录，遵循设置中的日期边界。问题会随所选记录发送给配置的模型；结果仅供复盘，不修改原记录。</p>
        <button className="primary-btn primary-btn--sm" disabled={!ready||!!busy||!from||!to||from>to} onClick={()=>void review()}>{busy==="review"?"正在分析所选记录…":question.trim()?"基于记录回答":"生成时间复盘"}</button>
      </section>
      {report&&<section className="settings-group ai-section"><div className="ai-record-head"><h2>复盘结果</h2><span>{report.from} — {report.to}</span></div><p>{report.recordCount} 条记录 · {formatDuration(report.recordedSeconds)} · {report.model}</p><MemoContent content={report.markdown}/><div className="integration-actions"><button className="ghost-btn" onClick={()=>{void navigator.clipboard.writeText(report.markdown).then(()=>onNotify("已复制复盘内容")).catch(()=>onNotify("复制失败，请使用导出 Markdown","error"));}}>复制内容</button><button className="ghost-btn" onClick={exportReport}>导出 Markdown</button></div></section>}
    </>}
    {tab==="settings"&&<IntegrationsPage focusPlugin="ai" embedded onNotify={onNotify} onDataChanged={onDataChanged}/>}
  </div>;
}
