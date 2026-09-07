// Exercise the project adapter callbacks without a model or personal history.
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile, chmod, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const root = await mkdtemp(join(tmpdir(), "sqnic-pi-test-"));
try {
  const fake = join(root, "fake sqnic");
  const mode = join(root, "mode");
  await writeFile(mode, "normal");
  await writeFile(fake, `#!${process.execPath}\nimport fs from 'node:fs';
const payload = JSON.parse(fs.readFileSync(0, 'utf8'));
const mode = fs.readFileSync(${JSON.stringify(mode)}, 'utf8');
if (mode === 'failure') process.exit(1);
if (mode === 'malformed') { console.log('invalid'); process.exit(0); }
console.log(JSON.stringify(mode === 'warning' ? {systemMessage:'fixture warning'} : payload.hook_event_name === 'SessionStart' ? {hookSpecificOutput:{additionalContext:'pi-startup-marker'}} : {}));\n`);
  await chmod(fake, 0o700);
  const source = await readFile(new URL("../adapters/pi-extension.ts", import.meta.url), "utf8");
  const extensionPath = join(root, "sqnic.ts");
  await writeFile(extensionPath, source.replaceAll("__SQNIC_BIN__", JSON.stringify(fake)).replaceAll("__SQNIC_DB__", JSON.stringify(join(root, "db.sqlite"))).replaceAll("__SQNIC_REPO__", JSON.stringify(root)));
  let handlers;
  if (process.argv[2]) {
    const loader = await import(pathToFileURL(resolve(process.argv[2])));
    const loaded = await loader.loadExtensions([extensionPath], root);
    assert.deepEqual(loaded.errors, []);
    assert.equal(loaded.extensions.length, 1);
    handlers = loaded.extensions[0].handlers;
  } else {
    handlers = new Map();
    const extension = await import(pathToFileURL(extensionPath));
    extension.default({ on: (name, fn) => handlers.set(name, [fn]) });
  }
  const notices = [];
  const ctx = {cwd:root, sessionManager:{getSessionId:()=>"pi-fixture",getSessionFile:()=>join(root,"session.jsonl")},ui:{notify:(message,type)=>notices.push({message,type})}};
  async function call(name, event = {}) {
    const callbacks = handlers.get(name);
    assert.equal(callbacks.length, 1);
    return callbacks[0](event, ctx);
  }
  await call("session_start");
  const first = await call("before_agent_start", {prompt:"continue"});
  assert.equal(first.message.content, "pi-startup-marker");
  assert.equal(first.message.customType, "sqnic-context");
  assert.equal(first.message.display, false);
  assert.equal(await call("before_agent_start", {prompt:"next"}), undefined);
  await writeFile(mode, "failure");
  await call("tool_execution_end");
  assert.match(notices.at(-1).message, /context unavailable/);
  await writeFile(mode, "malformed");
  await call("session_shutdown");
  assert.match(notices.at(-1).message, /context unavailable/);
  await writeFile(mode, "warning");
  await call("tool_execution_end");
  assert.equal(notices.at(-1).message, "fixture warning");
  console.log(JSON.stringify({passed:true,loader:process.argv[2]?"installed Pi loader":"Node TypeScript",checks:["startup context retained until first prompt","hidden context injection","no repeat injection","process failure does not throw","malformed output does not throw","hook warnings visible"]}));
} finally {
  await rm(root, {recursive:true,force:true});
}
