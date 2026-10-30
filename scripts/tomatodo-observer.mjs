import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

// Deliberately reads scalar fields only. No IPC, storage, timer or DOM mutation.
export const SNAPSHOT_EXPRESSION = `(() => {
 if (typeof TimerCoreState !== 'object' || TimerCoreState === null ||
     typeof TimerCoreState.isRunning !== 'boolean' || typeof isRestMode !== 'boolean')
   return { compatible: false };
 const c = TimerCoreState;
 const t = typeof currentTodo === 'object' && currentTodo !== null ? currentTodo : c.targetTodo;
 return { compatible: true, running: c.isRunning, paused: c.isPauseTimer === true ||
   (typeof isPaused === 'boolean' && isPaused), rest: isRestMode,
   taskId: t && (typeof t.id === 'number' || typeof t.id === 'string') ? t.id : null,
   title: t && typeof t.name === 'string' ? t.name : '',
   startMs: c.startDate, endMs: c.endDate,
   sequence: typeof activeMainTimerSequence === 'number' ? activeMainTimerSequence : null,
   generation: typeof activeRendererTimerGeneration === 'number' ? activeRendererTimerGeneration : null,
   countUp: typeof isCountUp === 'boolean' ? isCountUp : null,
   remainingSeconds: isRestMode && typeof restRemainingSeconds === 'number' ? restRemainingSeconds :
     typeof remainingSeconds === 'number' ? remainingSeconds : null,
   totalSeconds: typeof totalSeconds === 'number' ? totalSeconds : null };
})()`;

export function normalize(raw, now = Date.now()) {
  if (!raw?.compatible || typeof raw.running !== 'boolean' || typeof raw.rest !== 'boolean')
    throw new Error('当前番茄版本未提供预期状态，已停止解释计时事件');
  const state = raw.rest ? 'rest' : raw.running ? (raw.paused ? 'paused' : 'focus') : 'idle';
  const startMs = Number.isFinite(raw.startMs) && raw.startMs > 0 ? raw.startMs : null;
  if (['focus', 'paused'].includes(state) && (!startMs || typeof raw.title !== 'string' ||
      !raw.title.trim() || raw.countUp !== true ||
      !['string','number'].includes(typeof raw.taskId)))
    throw new Error('专注快照缺少任务、会话身份或正计时标志，状态未知');
  return { ...raw, title: String(raw.title || '').slice(0, 500), state, startMs,
    // Sequence is retained for diagnosis; pause/resume may change it, so it is not session identity.
    sessionKey: ['focus', 'paused'].includes(state) && startMs ? `${raw.taskId ?? ''}:${startMs}` : null,
    observedAt: now };
}

export function transition(previous, current) {
  if (!previous) return 'baseline';
  const active = s => s.state === 'focus' || s.state === 'paused';
  if (active(previous) && active(current) && previous.sessionKey && current.sessionKey &&
      previous.sessionKey !== current.sessionKey) return 'session-changed';
  if (!active(previous) && active(current)) return 'focus-observed';
  if (active(previous) && !active(current)) return current.state === 'rest' ? 'rest-observed' : 'idle-observed';
  if (previous.state === 'paused' && current.state === 'focus') return 'resume-observed';
  if (previous.state === 'rest' && current.state === 'idle') return 'rest-ended';
  if (previous.state !== current.state) return current.state === 'paused' ? 'pause-observed' : 'state-changed';
  if (previous.title !== current.title) return 'title-changed';
  return null;
}

export class ReadOnlyCdp {
  constructor(port = 19275) { this.port = port; this.pending = new Map(); this.nextId = 0; }
  async connect() {
    this.close();
    const response = await fetch(`http://127.0.0.1:${this.port}/json/list`, { signal: AbortSignal.timeout(2500) });
    if (!response.ok) throw new Error('无法列出番茄页面');
    const targets = await response.json();
    const candidates = targets.filter(t => t.type === 'page' &&
      decodeURIComponent(t.url || '').toLowerCase() ===
      'file:///c:/program files/tomatodo/resources/app.asar/index.html');
    if (candidates.length !== 1) throw new Error('未找到唯一的官方番茄页面，请检查版本及安装位置');
    const endpoint = new URL(candidates[0].webSocketDebuggerUrl);
    if (endpoint.protocol !== 'ws:' || endpoint.hostname !== '127.0.0.1' ||
        endpoint.port !== String(this.port)) throw new Error('拒绝非本机调试地址');
    const socket = new WebSocket(endpoint);
    this.socket = socket;
    await new Promise((ok, fail) => {
      const timer = setTimeout(() => { socket.close(); fail(new Error('调试连接超时')); }, 2500);
      socket.addEventListener('open', () => { clearTimeout(timer); ok(); }, { once: true });
      socket.addEventListener('error', () => { clearTimeout(timer); fail(new Error('调试连接失败')); }, { once: true });
    });
    socket.addEventListener('message', event => {
      let message;
      try { message = JSON.parse(event.data); } catch { return; }
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id); clearTimeout(pending.timer);
      message.error ? pending.reject(new Error(message.error.message)) : pending.resolve(message.result);
    });
    socket.addEventListener('close', () => { if (this.socket === socket) this.rejectPending(); });
    socket.addEventListener('error', () => { if (this.socket === socket) this.rejectPending(); });
  }
  rejectPending() {
    for (const item of this.pending.values()) { clearTimeout(item.timer); item.reject(new Error('调试连接断开')); }
    this.pending.clear();
  }
  async snapshot() {
    if (this.socket?.readyState !== WebSocket.OPEN) await this.connect();
    const id = ++this.nextId;
    const result = await new Promise((resolveResult, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error('读取计时状态超时')); }, 2500);
      this.pending.set(id, { resolve: resolveResult, reject, timer });
      try {
        this.socket.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: {
          expression: SNAPSHOT_EXPRESSION, returnByValue: true, throwOnSideEffect: true, timeout: 1000,
        } }));
      } catch (error) { clearTimeout(timer); this.pending.delete(id); reject(error); }
    });
    if (result.exceptionDetails) throw new Error('只读状态表达式未通过：' +
      (result.exceptionDetails.exception?.description || result.exceptionDetails.text));
    return normalize(result.result?.value);
  }
  close() {
    this.rejectPending();
    if (this.socket && this.socket.readyState < WebSocket.CLOSING) this.socket.close();
    this.socket = null;
  }
}

export async function serve({ cdpPort = 19275, port = 19276, parentPid = null } = {}) {
  const client = new ReadOnlyCdp(cdpPort);
  const assets = new Map(await Promise.all(['html', 'js'].map(async ext =>
    [ext, await readFile(new URL(`./tomatodo-observer.${ext}`, import.meta.url))])));
  let latest = { connected: false, error: '等待首次读取', snapshot: null, events: [] };
  let previous = null;
  let stopped = false;
  const loop = async () => {
    while (!stopped) {
      try {
        const current = await client.snapshot();
        const event = transition(previous, current);
        if (event) latest.events.unshift({ type: event, time: current.observedAt, title: current.title,
          state: current.state, sessionKey: current.sessionKey, startMs: current.startMs,
          endMs: current.endMs, sequence: current.sequence });
        latest = { connected: true, error: null, snapshot: current, events: latest.events.slice(0, 100) };
        previous = current;
      } catch (error) {
        if (latest.connected) latest.events.unshift({ type: 'disconnected', time: Date.now(), title: '' });
        latest = { connected: false, error: error.message, snapshot: null, events: latest.events.slice(0, 100) };
        previous = null; // A gap never becomes a fabricated end or start event.
        client.close();
      }
      await new Promise(ok => setTimeout(ok, 750));
    }
  };
  const server = http.createServer((request, response) => {
    if (request.headers.host !== `127.0.0.1:${port}` ||
        (request.headers.origin && request.headers.origin !== `http://127.0.0.1:${port}`)) {
      response.writeHead(403); response.end(); return;
    }
    response.setHeader('Cache-Control', 'no-store');
    response.setHeader('X-Content-Type-Options', 'nosniff');
    response.setHeader('X-Frame-Options', 'DENY');
    response.setHeader('Content-Security-Policy', "default-src 'self'; script-src 'self'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'");
    if (request.method === 'POST' && request.url === '/stop') {
      response.setHeader('Content-Type', 'application/json');
      response.end('{"stopped":true}');
      stopped = true; client.close(); server.close(); return;
    }
    if (request.method !== 'GET') { response.writeHead(405); response.end(); return; }
    if (request.url === '/snapshot') {
      response.setHeader('Content-Type', 'application/json; charset=utf-8');
      response.end(JSON.stringify({observerProtocol:1,parentPid,...latest})); return;
    }
    const ext = request.url === '/' ? 'html' : request.url === '/observer.js' ? 'js' : null;
    if (!ext) { response.writeHead(404); response.end(); return; }
    response.setHeader('Content-Type', ext === 'html' ? 'text/html; charset=utf-8' : 'text/javascript; charset=utf-8');
    response.end(assets.get(ext));
  });
  await new Promise((ok, fail) => { server.once('error', fail); server.listen(port, '127.0.0.1', ok); });
  void loop();
  return { server, stop() { stopped = true; client.close(); server.close(); } };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.includes('--once')) {
    const client = new ReadOnlyCdp();
    try { console.log(JSON.stringify(await client.snapshot(), null, 2)); }
    finally { client.close(); }
  } else {
    const portArg=process.argv.find(arg=>arg.startsWith('--port='));
    const port=portArg?Number(portArg.slice(7)):19276;
    if (!Number.isInteger(port)||port<1024||port>65535) throw new Error('无效端口');
    const parentArg=process.argv.find(arg=>arg.startsWith('--parent-pid='));
    const parentPid=parentArg?Number(parentArg.slice(13)):null;
    if (parentPid!==null&&(!Number.isInteger(parentPid)||parentPid<=0)) throw new Error('无效父进程');
    const instance = await serve({port,parentPid});
    if(parentPid) setInterval(()=>{try{process.kill(parentPid,0);}catch{instance.stop();}},2000).unref();
    console.log('只读观察器：http://127.0.0.1:'+port);
    for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, () => { instance.stop(); });
  }
}
