import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { normalize, transition, SNAPSHOT_EXPRESSION } from './tomatodo-observer.mjs';

const raw = changes => ({ compatible: true, running: false, rest: false, paused: false,
  countUp: true, title: '数学', taskId: 1, startMs: 1000, sequence: 1, ...changes });
test('real expression reads frozen state without writing or invoking host APIs', () => {
  const todo = Object.freeze({ id: 7, name: '高数 <script>测试</script>' });
  const core = Object.freeze({ isRunning: true, isPauseTimer: false, startDate: 1234, targetTodo: todo });
  const context = { TimerCoreState: core, currentTodo: todo, isPaused: false, isRestMode: false,
    remainingSeconds: 12, isCountUp: true };
  const keys = Object.keys(context);
  const result = vm.runInNewContext(SNAPSHOT_EXPRESSION, context, { timeout: 100 });
  assert.equal(result.title, todo.name); assert.equal(result.running, true);
  assert.equal(core.startDate, 1234); assert.deepEqual(Object.keys(context), keys);
});
test('missing runtime is incompatible, never idle', () => {
  assert.equal(vm.runInNewContext(SNAPSHOT_EXPRESSION).compatible, false);
  assert.throws(() => normalize({}), /未提供预期状态/);
});
test('rest takes precedence over running flag; stale pause flag cannot activate idle', () => {
  assert.equal(normalize(raw({ running: true, rest: true })).state, 'rest');
  assert.equal(normalize(raw({ paused: true })).state, 'idle');
});
test('same task title in another session has distinct identity', () => {
  const before = normalize(raw({ running: true }));
  const after = normalize(raw({ running: true, startMs: 9000 }));
  assert.equal(transition(before, after), 'session-changed');
});
test('pause and resume do not become new sessions when sequence changes', () => {
  const focus = normalize(raw({ running: true }));
  const paused = normalize(raw({ running: true, paused: true, sequence: 2 }));
  assert.equal(focus.sessionKey, paused.sessionKey);
  assert.equal(transition(focus, paused), 'pause-observed');
  assert.equal(transition(paused, focus), 'resume-observed');
});
test('attach/reconnect only creates a baseline even if currently focusing', () => {
  assert.equal(transition(null, normalize(raw({ running: true }))), 'baseline');
});
test('idle to focus and focus to rest are distinguished', () => {
  const idle = normalize(raw({})), focus = normalize(raw({ running: true }));
  assert.equal(transition(idle, focus), 'focus-observed');
  assert.equal(transition(focus, normalize(raw({ rest: true }))), 'rest-observed');
  assert.equal(transition(focus, idle), 'idle-observed');
});
test('unchanged samples do not duplicate events; unknown start stays unknown', () => {
  assert.throws(() => normalize(raw({ running: true, startMs: 0 })), /快照缺少/);
  const value = normalize(raw({ running: true }));
  assert.equal(transition(value, { ...value, observedAt: 999 }), null);
});

test('incomplete focused snapshot cannot be consumed',()=>{ for(const bad of [{title:''},{startMs:null},{countUp:null},{taskId:null}]) assert.throws(()=>normalize(raw({running:true,...bad})),/快照缺少/); });
