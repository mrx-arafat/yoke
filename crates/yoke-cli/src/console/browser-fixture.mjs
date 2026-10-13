import { spawn, spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { DatabaseSync } from 'node:sqlite';

const root = realpathSync(mkdtempSync(join(tmpdir(), 'yoke-console-w5-')));
const home = join(root, 'home');
const project = join(root, 'project');
mkdirSync(home); mkdirSync(project);
const binary = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..', 'target/debug/yoke');
const env = { ...process.env, HOME: home, YOKE_OFF: '0' };
const start = spawnSync(binary, ['run-hook', 'SessionStart', '--agent', 'claude'], {
  cwd: project, env, input: JSON.stringify({ session_id: 'fixture-session', cwd: project, hook_event_name: 'SessionStart' }), encoding: 'utf8',
});
if (start.status !== 0) { rmSync(root,{recursive:true,force:true}); throw new Error(start.stderr || start.stdout || start.error?.message); }
for(const args of [['plan','new','browser-review','--title','Browser evidence review'],['remember','Use explicit temporary paths for every console fixture.'],['remember','Approval freezes the exact reviewed header.','--pinned']]) {
  const result=spawnSync(binary,args,{cwd:project,env,encoding:'utf8'});
  if(result.status!==0){rmSync(root,{recursive:true,force:true});throw new Error(result.stderr);}
}
writeFileSync(join(project,'.yoke/plans/browser-review.md'),'+++\ntitle = "Browser evidence review"\n[[check]]\nname = "Focused verification"\nrun = "printf verified"\nblocking = true\n+++\n\n# Browser evidence review\n\nReview the exact contract before approval.\n\n```rust\nlet approved = true;\n```\n');
const db = new DatabaseSync(join(home, '.yoke/state.db'));
db.exec('BEGIN');
const insert = db.prepare('INSERT INTO decisions(ts,session_id,agent,project,event,tool,mode,outcome,reason,gates,payload) VALUES (?,?,?,?,?,?,?,?,?,?,?)');
const now = Math.floor(Date.now()/1000);
for (let i = 0; i < 100_005; i++) {
  const outcome = i % 2 ? ['allow','would_deny'][i % 3 % 2] : ['allow','deny','rewrite'][i % 3];
  insert.run(now - 100_005 + i, 'fixture-session', i % 2 ? 'codex' : 'claude', project, 'pre_tool_use', i % 3 ? 'Bash' : 'Edit', i % 2 ? 'observe' : 'enforce', outcome,
    outcome === 'allow' ? null : 'The approved check requires a passing result before this action.', JSON.stringify([{name:'protected-work',result:outcome,ms:4}]), JSON.stringify({tool_input:{command:'cargo test -p example focused_case'}}));
}
const usage=db.prepare('INSERT INTO usage_samples(agent,session_id,project,record_key,model,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,cost_nano_usd) VALUES(?,?,?,?,?,?,?,?,?,?)');
usage.run('claude','fixture-session',project,'fixture-sonnet','claude-sonnet-4-5',120000,14000,0,0,570000000);
usage.run('codex','fixture-session',project,'fixture-codex','unknown-fixture-model',85000,9000,12000,0,null);
db.exec('COMMIT'); db.close();
console.log(`Fixture root: ${root}`);
const child = spawn(binary, ['console', '--no-open'], { cwd: project, env, stdio: ['ignore','pipe','inherit'] });
child.stdout.pipe(process.stdout);
let stopping = false;
function stop() { if(stopping)return;stopping=true;child.kill('SIGTERM'); }
process.on('SIGINT',stop); process.on('SIGTERM',stop);
child.on('exit', code => { rmSync(root,{recursive:true,force:true});process.exit(stopping?0:code??1); });
