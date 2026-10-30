// Browser demonstration only. Never contacts the observer or desktop database.
type Note = {id:string;title:string;note:string;revision:number;startAt:number;endAt:number};
let counter=0;
let active: {id:string;title:string;startAt:number}|null=null;
const context={initialized:false,connected:false,source_phase:"unknown",session:null as null|{title:string;key:string;started_at_ms:number},active_segment_id:null as string|null,manually_stopped:false,resume_on_focus:false,pending_notes:[] as string[],gap_detected:false,conflict:null};
let enabled=false;
const notes: Note[]=[];
function open(){
 if(active)throw new Error("已有模拟分段");
 active={id:"preview-link-"+(++counter),title:context.session?.title||"线代（模拟）",startAt:Date.now()};
 context.active_segment_id=active.id;context.manually_stopped=false;
}
function close(endAt=Date.now()){
 if(!active)throw new Error("没有模拟分段");
 notes.push({...active,endAt:Math.max(active.startAt+1,endAt),revision:1,note:""});
 context.pending_notes.push(active.id);active=null;context.active_segment_id=null;context.manually_stopped=true;
}
export function previewLink(action:string,params:Record<string,unknown>={}){
 switch(action){
 case "enable": enabled=true;context.initialized=true;context.connected=true;context.source_phase="focus";
 context.session={title:"线代（模拟）",key:"preview",started_at_ms:Date.now()};if(!active)open();break;
 case "disable": if(active)throw new Error("请先结束当前分段");enabled=false;context.connected=false;context.source_phase="unknown";context.session=null;break;
 case "startNext":if(!enabled)throw new Error("请先启用模拟联动");open();break;
 case "endSegment":close();break;
 case "reconcile":if(active)close(Number(params.endAt));context.session=null;break;
 case "saveNote":case "skipNote":{
 const index=notes.findIndex(n=>n.id===params.id);if(index<0)throw new Error("记录已变化");
 if(notes[index].revision!==params.revision)throw new Error("版本冲突");
 notes.splice(index,1);context.pending_notes=context.pending_notes.filter(id=>id!==params.id);break;
 }
 case "status":break;
 default:throw new Error("不支持的模拟操作");
 }
 return JSON.parse(JSON.stringify({enabled,owner:enabled?"local-tomatodo-link":null,error:"浏览器演示：模拟数据，不连接真实番茄或 DotLog 记录。",context,pendingNotes:notes,now:Date.now()}));
}
