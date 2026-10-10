/**
 * Capture the retail original through a running BottleShip cruise.
 * BOTTLESHIP_ROOT=/path/to/bottleship bun tools/handling/capture_original.ts launch out.json
 * BS_URL_MATCH chooses an existing guest; this script never boots another emulator.
 * Research addresses apply to the documented retail executable, not MM2Trial.exe.
 */
import { resolve } from "node:path";
const root = process.env.BOTTLESHIP_ROOT;
if (!root) throw new Error("Set BOTTLESHIP_ROOT to the BottleShip checkout");
const { connect, pageEval } = await import(resolve(root, "tools/cdp-core.ts"));
const scenario = process.argv[2] ?? "launch";
const scene = process.env.ORIGINAL_SCENE ?? "unverified";
const fixtureHash = process.env.ORIGINAL_FIXTURE_SHA256 ?? null;
const carId = process.env.ORIGINAL_CAR_ID ?? "unknown";
const baseline = process.env.ORIGINAL_BASELINE ?? "none";
const settleFrames = Number(process.env.ORIGINAL_SETTLE_FRAMES ?? "600");
if (!Number.isInteger(settleFrames) || settleFrames < 180) throw new Error("ORIGINAL_SETTLE_FRAMES must be an integer of at least 180");
const pose = process.env.ORIGINAL_POSE?.split(",").map(Number);
if (pose && (pose.length !== 4 || pose.some(x => !Number.isFinite(x)))) throw new Error("ORIGINAL_POSE must be x,y,z,yawDegrees");
if (pose && baseline === "restore") throw new Error("Choose an explicit pose or a saved baseline");
if (!["none", "save", "restore"].includes(baseline)) throw new Error("ORIGINAL_BASELINE must be none, save, or restore");
const expectedExeHash = process.env.ORIGINAL_EXPECTED_EXE_SHA256 ?? "93afb6c00be3d3b12a6e5d88d8e4f711a13f5a4100dbdfab77943c30083f98a2";
const captureId = `${Date.now()}-${Math.random().toString(16).slice(2)}`;
const output = process.argv[3] ?? `/tmp/original-${scenario}.json`;
const schedules: Record<string, [number, [number, boolean][]][]> = {
    launch: [[0, [[38, true]]]],
    coast: [[0, [[38, true]]], [300, [[38, false]]]],
    brake: [[0, [[38, true]]], [300, [[38, false], [40, true]]]],
    turn: [[0, [[38, true]]], [300, [[39, true]]], [420, [[39, false]]]],
    powerslide: [[0, [[38, true]]], [300, [[39, true]]], [330, [[32, true]]], [348, [[32, false]]], [375, [[39, false], [37, true]]], [435, [[37, false]]]],
    handbrake: [[0, [[38, true]]], [300, [[38, false], [39, true], [32, true]]], [420, [[39, false], [32, false]]]],
};
if (!schedules[scenario]) throw new Error(`Unknown scenario ${scenario}`);
const { session } = await connect({ urlMatch: process.env.BS_URL_MATCH ?? "http://localhost:5175/?game=dev" });
const worker = (code: string) => pageEval(session, `__BS__.harness.evalWorker(${JSON.stringify(code)})`, { timeoutMs: 15000 });
try {
    const provenance = await worker(`
const sys=System.getInstance(),p=sys.process,m=p.getCurrentMemory();
if(!p.moduleRegistry.modules.has('midtown2.exe'))throw new Error('Requires the documented retail original');
const signatures=[[0x405760,[0x55,0x8b,0xec,0x81,0xec,0xe4,0,0,0,0x53,0x56,0x8b,0xf1,0x57]], [0x4cc8d0,[0x55,0x8b,0xec,0x83,0xec,0x0c,0x53,0x56,0x8b,0xf1,0x57]]];
for(const[at,bytes]of signatures)if(bytes.some((v,i)=>m[at+i]!==v))throw new Error('Retail executable code signature mismatch at '+at.toString(16));
const path='C:\\\\Midtown2.exe',fs=sys.fileSystem,h=fs.openSync(path,0x80000000,3);if(!h)throw new Error('Cannot read retail executable for fingerprint');const bytes=await fs.read(h,fs.getFileSize(path));
const sha256=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',bytes)),b=>b.toString(16).padStart(2,'0')).join('');
if(sha256!==${JSON.stringify(expectedExeHash)})throw new Error('Retail executable SHA-256 differs from documented address version: '+sha256);
return {executable_sha256:sha256,signatures_verified:true};`);
    const hit = await pageEval(session, `(async()=>{await __BS__.harness.clearBreaks();await __BS__.harness.resume();return await __BS__.harness.breakOn(0x405760,{fast:true,pause:true,capture:{backtrace:false,stack:0}});})()`, { timeoutMs: 15000 });
    const player = Number(hit.hit.callsite.regs.ecx);
    await pageEval(session, "__BS__.harness.clearBreaks()", { timeoutMs: 15000 });
    await worker(`
const sys=System.getInstance(),p=sys.process,m=p.getCurrentMemory(),d=new DataView(m.buffer,m.byteOffset,m.byteLength);
if(!p.moduleRegistry.modules.has('midtown2.exe'))throw new Error('Requires the documented retail original');
state.oracle={id:${JSON.stringify(captureId)},process:p,player:${player},car:d.getUint32(${player}+0x284,true),rows:[],ticks:0,lastFrame:-1,scenario:${JSON.stringify(scenario)},carId:${JSON.stringify(carId)},scene:${JSON.stringify(scene)},fixtureHash:${JSON.stringify(fixtureHash)},provenance:${JSON.stringify(provenance)},pose:${JSON.stringify(pose ?? null)},baseline:${JSON.stringify(baseline)},settleFrames:${settleFrames},schedule:${JSON.stringify(schedules[scenario])}};
const o=state.oracle;const a=o.car;
if(o.baseline==='restore'){const b=state.originalOracleBaseline;if(!b||b.process!==p||b.carId!==o.carId||b.car!==a||b.player!==o.player)throw new Error('Saved baseline must belong to this same guest player and car');m.set(b.bytes,a);d.setInt16(d.getUint32(a+0x1d0,true)+6,b.room,true);const input=d.getUint32(0x6b0cd0,true);d.setUint32(input+0x1d4,b.pedalsSwapped,true);}
if(o.pose){const[x,y,z,degrees]=o.pose,t=degrees*Math.PI/180,c=Math.cos(t),s=Math.sin(t),rows=[c,0,-s,0,1,0,s,0,c];rows.forEach((v,i)=>d.setFloat32(a+0x6c+i*4,v,true));[x,y,z].forEach((v,i)=>d.setFloat32(a+0x90+i*4,v,true));for(const base of[0x54,0x60,0x9c,0xa8])for(let i=0;i<3;i++)d.setFloat32(a+base+i*4,0,true);const inst=d.getUint32(a+0x1d0,true);d.setInt16(inst+6,1,true);[x,y,z].forEach((v,i)=>d.setFloat32(inst+0x90+i*4,v,true));d.setUint32(a+0x20,1,true);}
const f=x=>d.getFloat32(a+x,true);o.preWarmState={rpm:f(0x2c4),engineOmega:f(0x2c0),gear:d.getUint32(a+0x304,true),pos:[f(0x90),f(0x94),f(0x98)],vel:[f(0x9c),f(0xa0),f(0xa4)]};o.liveTuning={mass:f(0x244),inertiaBox:[f(0x228),f(0x22c),f(0x230)],horsepower:f(0x274),idleRpm:f(0x278),optimalRpm:f(0x27c),maxRpm:f(0x280),frontRadius:f(0x4b8+0x1bc),rearRadius:f(0x990+0x1bc)};o.initialLimits=[d.getFloat32(0x5cd830,true),d.getFloat32(0x5cd834,true),d.getFloat32(0x5cd838,true),d.getUint32(0x6a2c38,true)];
d.setFloat32(0x5cd830,1/60,true);d.setFloat32(0x5cd834,1/60,true);d.setFloat32(0x5cd838,1/60,true);d.setUint32(0x6a2c38,0,true);
for(const k of [37,38,39,40,32])sys.inputManager.injectKey(k,false);
o.hooks=p.dispatcher.findStubsByName('dinput','IDirectInputDevice2A_GetDeviceState').map(s=>({id:s.functionId,fn:p.dispatcher.fastPathTable[s.functionId]}));
for(const h of o.hooks)p.dispatcher.fastPathTable[h.id]=function(esp,dv,mem8){
 const frame=dv.getUint32(0x6a2c30,true);
 if(dv.getUint32(esp+8,true)===256&&!o.done&&frame!==o.lastFrame){
  o.lastFrame=frame;
  const n=o.ticks++-o.settleFrames;
  if(n===0){const a=o.car;const v=Math.hypot(...[0x9c,0xa0,0xa4].map(x=>dv.getFloat32(a+x,true)));const up=dv.getFloat32(a+0x7c,true),ground=[0x4b8,0x724,0x990,0xbfc].map(x=>dv.getUint8(a+x+0x226));if(v>0.1||up<0.99||ground.some(x=>x!==1)){o.done=true;o.error='Initial state must be stationary, upright, with all four wheels grounded; reset the cruise before capturing';globalThis.__harnessPause();}else if(o.baseline==='save'){const inst=dv.getUint32(a+0x1d0,true);state.originalOracleBaseline={process:p,carId:o.carId,car:a,player:o.player,bytes:new Uint8Array(mem8.slice(a,a+0x1560)),room:dv.getInt16(inst+6,true),pedalsSwapped:dv.getUint32(dv.getUint32(0x6b0cd0,true)+0x1d4,true)};}}
  if(n>=0&&!o.error){const a=o.car,f=x=>dv.getFloat32(a+x,true),v=x=>[f(x),f(x+4),f(x+8)];
   o.rows.push({n,t:n/60,frame:dv.getUint32(0x6a2c30,true),dt:dv.getFloat32(0x5cd820,true),pos:v(0x90),vel:v(0x9c),omega:v(0xa8),forward:v(0x84),speed:f(0x248),steer:f(0x1554),brake:f(0x154c),hb:f(0x1550),throttle:f(0x2bc),rpm:f(0x2c4),gear:dv.getUint32(a+0x304,true),spin:f(0x40c),ground:[0x4b8,0x724,0x990,0xbfc].map(x=>dv.getUint8(a+x+0x226)),surfaceFriction:[0x4b8,0x724,0x990,0xbfc].map(x=>f(x+0x1d0)),contactNormal:[0x4b8,0x724,0x990,0xbfc].map(x=>v(x+0x18c))});
   for(const [at,keys]of o.schedule)if(at===n)for(const[k,down]of keys)sys.inputManager.injectKey(k,down);
   if(n>=900){o.done=true;for(const k of [37,38,39,40,32])sys.inputManager.injectKey(k,false);globalThis.__harnessPause();}
  }
 }
 return h.fn(esp,dv,mem8);
};
globalThis.__harnessResume();return {car:o.car};`);
    let finished = false;
    const deadline = Date.now() + 15 * 60_000;
    while (Date.now() < deadline) {
        await Bun.sleep(2000);
        const progress = await worker("return {done:state.oracle.done===true,frames:state.oracle.rows.length};");
        if (progress.done) { finished = true; break; }
    }
    const result = await worker("const o=state.oracle;return {car:o.carId,scene:o.scene,fixture_sha256:o.fixtureHash,provenance:o.provenance,initial_pose_override:o.pose,baseline:o.baseline,settle_frames:o.settleFrames,pre_warm_state:o.preWarmState,live_tuning:o.liveTuning,scenario:o.scenario,hz:60,error:o.error,complete:o.done===true&&!o.error,rows:o.rows};");
    await Bun.write(output, JSON.stringify(result, null, 2));
    if (result.error) throw new Error(result.error);
    if (!finished) throw new Error(`Capture timed out; partial data saved to ${output}`);
    console.log(`Captured ${result.rows.length} frames to ${output}; original is paused.`);
} finally {
    await worker(`const o=state.oracle;if(o?.id===${JSON.stringify(captureId)}){const p=System.getInstance().process,m=p.getCurrentMemory(),d=new DataView(m.buffer,m.byteOffset,m.byteLength);for(const h of o.hooks??[])p.dispatcher.fastPathTable[h.id]=h.fn;for(const k of [37,38,39,40,32])System.getInstance().inputManager.injectKey(k,false);if(o.initialLimits){[0x5cd830,0x5cd834,0x5cd838].forEach((a,i)=>d.setFloat32(a,o.initialLimits[i],true));d.setUint32(0x6a2c38,o.initialLimits[3],true);}}return {restored:true};`).catch((error: unknown) => { console.error('Original capture cleanup failed:', error); process.exitCode = 1; });
    session.close();
    process.exitCode = process.exitCode ?? 0;
}
process.exit();
