const $ = id => document.getElementById(id);
const states = { focus: '专注中', paused: '已暂停', rest: '休息中', idle: '空闲' };
const events = { baseline: '连接成功，建立当前状态基线', 'focus-observed': '观察到专注开始',
  'rest-observed': '观察到进入休息', 'idle-observed': '观察到专注退出',
  'pause-observed': '观察到暂停', 'resume-observed': '观察到继续', 'rest-ended': '休息结束，返回空闲', 'state-changed': '状态发生变化',
  'session-changed': '观察到另一场专注', 'title-changed': '任务名变化', disconnected: '连接断开，状态未知' };
let latest = null;
let stopped = false;
const clock = value => new Date(value).toLocaleTimeString('zh-CN', { hour12: false });
const duration = value => Number.isFinite(value) ?
  `${Math.floor(Math.max(0, value) / 60)}:${String(Math.floor(Math.max(0, value) % 60)).padStart(2, '0')}` : '—';
async function refresh() {
  if (stopped) return;
  try {
    const response = await fetch('/snapshot', { cache: 'no-store' });
    if (!response.ok) throw new Error('观察器响应异常');
    latest = await response.json();
    const s = latest.snapshot;
    $('connection').textContent = latest.connected ? '● 已连接 · 仅读取' : '○ 未连接';
    $('title').textContent = s?.title || (s?.state === 'idle' ? '等待开始专注' : '等待番茄 ToDo');
    $('state').textContent = states[s?.state] || '未知';
    $('time').textContent = duration(s?.remainingSeconds);
    $('updated').textContent = s ? clock(s.observedAt) : '—';
    $('error').textContent = latest.error || '';
    $('diagnostics').textContent = s ? JSON.stringify({ sessionKey: s.sessionKey, sequence: s.sequence,
      generation: s.generation, startMs: s.startMs, countUp: s.countUp }) : '未连接时不沿用旧状态';
    $('events').replaceChildren(...latest.events.map(event => {
      const li = document.createElement('li');
      li.textContent = `${clock(event.time)} · ${events[event.type] || event.type}${event.title ? ' · ' + event.title : ''}`;
      return li;
    }));
  } catch (error) {
    $('connection').textContent = '○ 观察器连接断开'; $('title').textContent = '等待重新连接';
    $('state').textContent = '未知'; $('time').textContent = '—'; $('updated').textContent = '—';
    $('error').textContent = error.message; $('diagnostics').textContent = '';
  } finally { if (!stopped) setTimeout(refresh, 1000); }
}
$('stop').onclick = async () => {
  try {
    const response = await fetch('/stop', { method: 'POST' });
    if (!response.ok) throw new Error('停止失败');
    stopped = true; $('connection').textContent = '○ 已停止观察';
    $('state').textContent = '未知'; $('title').textContent = '观察器已停止';
    $('time').textContent = '—'; $('updated').textContent = '—';
    $('diagnostics').textContent = ''; $('error').textContent = '';
    $('stop').disabled = true;
  } catch (error) { $('error').textContent = error.message; }
};
$('export').onclick = () => {
  if (!latest) return;
  const url = URL.createObjectURL(new Blob([JSON.stringify(latest, null, 2)], { type: 'application/json' }));
  const a = document.createElement('a'); a.href = url; a.download = '番茄只读观察.json'; a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
};
void refresh();
