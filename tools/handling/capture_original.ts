/**
 * Capture the retail original through a running BottleShip cruise.
 * BOTTLESHIP_ROOT=/path/to/bottleship bun tools/handling/capture_original.ts launch out.json
 * ORIGINAL_ROUTE=/path/route.json selects a city route with a MODEL-origin pose.
 * Route frames are integration steps: output includes sample0 plus the final endpoint.
 * BS_URL_MATCH chooses an existing guest; this script never boots another emulator.
 * Research addresses apply to the documented retail executable, not MM2Trial.exe.
 */
import { resolve } from "node:path";
const root = process.env.BOTTLESHIP_ROOT;
if (!root) throw new Error("Set BOTTLESHIP_ROOT to the BottleShip checkout");
const { connect, pageEval } = await import(resolve(root, "tools/cdp-core.ts"));
type Schedule = [number, [number, boolean][]][];
type CityRoute = {
    car: string;
    city: string;
    initial_model_pose: [number, number, number, number];
    initial_room: number;
    frames: number;
    settle_frames: number;
    schedule: Schedule;
};
const routePath = process.env.ORIGINAL_ROUTE;
const routeText = routePath ? await Bun.file(routePath).text() : null;
const route: CityRoute | null = routeText === null ? null : JSON.parse(routeText);
if (routePath) {
    if (!route || typeof route !== "object" || Array.isArray(route)
        || typeof route.car !== "string" || !/^vp[a-z0-9]+$/.test(route.car)
        || !["sf", "london"].includes(route.city)
        || !Array.isArray(route.initial_model_pose) || route.initial_model_pose.length !== 4
        || route.initial_model_pose.some(x => typeof x !== "number" || !Number.isFinite(x))
        || !Number.isInteger(route.initial_room) || route.initial_room < 1 || route.initial_room > 32767
        || !Number.isInteger(route.frames) || route.frames < 1800 || route.frames > 3600
        || route.settle_frames !== 600 || !Array.isArray(route.schedule) || route.schedule.length === 0) {
        throw new Error("City route requires a verified car/city, finite model pose, positive room, 1800..3600 frames, 600 settle frames and a schedule");
    }
    let previousFrame = -1;
    for (const event of route.schedule) {
        if (!Array.isArray(event) || event.length !== 2 || !Number.isInteger(event[0])
            || event[0] <= previousFrame || event[0] < 0 || event[0] >= route.frames
            || !Array.isArray(event[1]) || event[1].length === 0
            || event[1].some(key => !Array.isArray(key) || key.length !== 2
                || ![37, 38, 39, 40, 32].includes(key[0]) || typeof key[1] !== "boolean")) {
            throw new Error("City route schedule must contain chronological in-range frame events with driving virtual keys and boolean states");
        }
        previousFrame = event[0];
    }
    if (process.env.ORIGINAL_FIXTURE_SHA256 || process.env.ORIGINAL_POSE || process.env.ORIGINAL_CAR_ID && process.env.ORIGINAL_CAR_ID !== route.car
        || process.env.ORIGINAL_SETTLE_FRAMES && Number(process.env.ORIGINAL_SETTLE_FRAMES) !== route.settle_frames
        || process.env.ORIGINAL_FRAMES && Number(process.env.ORIGINAL_FRAMES) !== route.frames) {
        throw new Error("City route conflicts with ORIGINAL_FIXTURE_SHA256, ORIGINAL_POSE, ORIGINAL_CAR_ID, ORIGINAL_SETTLE_FRAMES or ORIGINAL_FRAMES");
    }
}
const routeSha256 = routeText === null ? null : new Bun.CryptoHasher("sha256").update(routeText).digest("hex");
const scenario = route ? "city_route" : process.argv[2] ?? "launch";
const scene = route ? `retail-${route.city}-route` : process.env.ORIGINAL_SCENE ?? "unverified";
const fixtureHash = process.env.ORIGINAL_FIXTURE_SHA256 ?? null;
const carId = route?.car ?? process.env.ORIGINAL_CAR_ID ?? "unknown";
const baseline = process.env.ORIGINAL_BASELINE ?? "none";
const wheelDiagnostics = !!route || process.env.ORIGINAL_WHEEL_DIAGNOSTICS === "1";
const captureFrames = route?.frames ?? Number(process.env.ORIGINAL_FRAMES ?? "900");
if (!Number.isInteger(captureFrames) || captureFrames < 1 || captureFrames > 3600) throw new Error("ORIGINAL_FRAMES must be an integer of 1..3600 integration steps");
const expectedTuning = process.env.ORIGINAL_EXPECTED_TUNING
    ? JSON.parse(await Bun.file(process.env.ORIGINAL_EXPECTED_TUNING).text()) : null;
if (process.env.ORIGINAL_EXPECTED_TUNING && (!expectedTuning || typeof expectedTuning !== "object" || Array.isArray(expectedTuning)
    || expectedTuning.car !== carId || !expectedTuning.live_tuning
    || typeof expectedTuning.live_tuning !== "object" || Array.isArray(expectedTuning.live_tuning))) {
    throw new Error("Expected tuning must contain the matching car and a live_tuning object");
}
const settleFrames = route?.settle_frames ?? Number(process.env.ORIGINAL_SETTLE_FRAMES ?? "600");
if (!Number.isInteger(settleFrames) || settleFrames < 180) throw new Error("ORIGINAL_SETTLE_FRAMES must be an integer of at least 180");
const expectedInitialUp = process.env.ORIGINAL_EXPECTED_INITIAL_UP?.split(",").map(Number);
if (expectedInitialUp && (expectedInitialUp.length !== 3 || expectedInitialUp.some(x => !Number.isFinite(x))
    || Math.abs(Math.hypot(...expectedInitialUp) - 1) > 0.001)) {
    throw new Error("ORIGINAL_EXPECTED_INITIAL_UP must be a finite unit vector");
}
const pose = route?.initial_model_pose ?? process.env.ORIGINAL_POSE?.split(",").map(Number);
if (pose && (pose.length !== 4 || pose.some(x => !Number.isFinite(x)))) throw new Error("ORIGINAL_POSE must be x,y,z,yawDegrees");
if (pose && baseline === "restore") throw new Error("Choose an explicit pose or a saved baseline");
if (!["none", "save", "restore"].includes(baseline)) throw new Error("ORIGINAL_BASELINE must be none, save, or restore");
const expectedDisplay = process.env.ORIGINAL_EXPECTED_DISPLAY?.split(",").map(Number);
const expectedTraffic = process.env.ORIGINAL_EXPECTED_TRAFFIC === undefined ? null : Number(process.env.ORIGINAL_EXPECTED_TRAFFIC);
const expectedCops = process.env.ORIGINAL_EXPECTED_COPS === undefined ? null : Number(process.env.ORIGINAL_EXPECTED_COPS);
if ([expectedTraffic, expectedCops].some(x => x !== null && (!Number.isFinite(x) || x < 0 || x > 1))) {
    throw new Error("Expected traffic and cops must be finite densities between zero and one");
}
if (expectedDisplay && (expectedDisplay.length !== 2 || expectedDisplay.some(x => !Number.isInteger(x) || x <= 0))) {
    throw new Error("ORIGINAL_EXPECTED_DISPLAY must be positive integer width,height");
}
const expectedExeHash = process.env.ORIGINAL_EXPECTED_EXE_SHA256 ?? "93afb6c00be3d3b12a6e5d88d8e4f711a13f5a4100dbdfab77943c30083f98a2";
const captureId = `${Date.now()}-${Math.random().toString(16).slice(2)}`;
const output = process.argv[3] ?? `/tmp/original-${scenario}.json`;
const schedules: Record<string, Schedule> = {
    launch: [[0, [[38, true]]]],
    coast: [[0, [[38, true]]], [300, [[38, false]]]],
    brake: [[0, [[38, true]]], [300, [[38, false], [40, true]]]],
    turn: [[0, [[38, true]]], [300, [[39, true]]], [420, [[39, false]]]],
    slalom: [[0, [[38, true]]], [300, [[39, true]]], [360, [[39, false], [37, true]]], [420, [[37, false], [39, true]]], [480, [[39, false], [37, true]]], [540, [[37, false]]]],
    lift_turn: [[0, [[38, true]]], [300, [[39, true]]], [330, [[38, false]]], [390, [[38, true]]], [450, [[39, false]]]],
    brake_turn: [[0, [[38, true]]], [300, [[39, true]]], [330, [[38, false], [40, true]]], [390, [[40, false], [38, true]]], [420, [[39, false], [37, true]]], [480, [[37, false]]]],
    powerslide: [[0, [[38, true]]], [300, [[39, true]]], [330, [[32, true]]], [348, [[32, false]]], [375, [[39, false], [37, true]]], [435, [[37, false]]]],
    handbrake: [[0, [[38, true]]], [300, [[38, false], [39, true], [32, true]]], [420, [[39, false], [32, false]]]],
};
const schedule = route?.schedule ?? schedules[scenario];
if (!schedule) throw new Error(`Unknown scenario ${scenario}`);
const { session } = await connect({ urlMatch: process.env.BS_URL_MATCH ?? "http://localhost:5175/?game=dev" });
const worker = (code: string) => pageEval(session, `__BS__.harness.evalWorker(${JSON.stringify(code)})`, { timeoutMs: 15000 });
const readResultCode = "const o=state.oracle;return {car:o.carId,car_identity_verified:o.carIdentityVerified??false,expected_tuning:o.expectedTuning,vehicle_selection:o.vehicleSelection,scene:o.scene,fixture_sha256:o.fixtureHash,provenance:o.provenance,city:o.city,route:o.route,route_sha256:o.routeSha256,integration_frames:o.captureFrames,initial_pose_override:o.pose,initial_pose_convention:o.poseConvention,initial_model_pose:o.initialModelPose,initial_com_pose:o.initialComPose,live_center_of_gravity:o.liveCenterOfGravity,settle_handbrake_held:!!o.route,baseline:o.baseline,settle_frames:o.settleFrames,wheel_diagnostics:o.wheelDiagnostics,total_cold_settle_frames:o.totalColdSettleFrames,initial_gate:o.initialGateObserved,pre_warm_state:o.preWarmState,renderer_diagnostic:o.rendererDiagnostic,renderer_events:o.rendererEvents??[],live_tuning:o.liveTuning,scenario:o.scenario,hz:60,error:o.error,complete:o.done===true&&!o.error,rows:o.rows};";
try {
    const provenance = await worker(`
const sys=System.getInstance(),p=sys.process,m=p.getCurrentMemory();
const display={width:sys.ddrawContext?.display?.width??null,height:sys.ddrawContext?.display?.height??null},expectedDisplay=${JSON.stringify(expectedDisplay ?? null)};
if(expectedDisplay&&(display.width!==expectedDisplay[0]||display.height!==expectedDisplay[1]))throw new Error('Original display differs from expected cruise display: '+JSON.stringify(display));
if(!p.moduleRegistry.modules.has('midtown2.exe'))throw new Error('Requires the documented retail original');
const signatures=[[0x405760,[0x55,0x8b,0xec,0x81,0xec,0xe4,0,0,0,0x53,0x56,0x8b,0xf1,0x57]], [0x4cc8d0,[0x55,0x8b,0xec,0x83,0xec,0x0c,0x53,0x56,0x8b,0xf1,0x57]]];
for(const[at,bytes]of signatures)if(bytes.some((v,i)=>m[at+i]!==v))throw new Error('Retail executable code signature mismatch at '+at.toString(16));
const path='C:\\\\Midtown2.exe',fs=sys.fileSystem,h=fs.openSync(path,0x80000000,3);if(!h)throw new Error('Cannot read retail executable for fingerprint');const bytes=await fs.read(h,fs.getFileSize(path));
const sha256=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',bytes)),b=>b.toString(16).padStart(2,'0')).join('');
if(sha256!==${JSON.stringify(expectedExeHash)})throw new Error('Retail executable SHA-256 differs from documented address version: '+sha256);
let archiveSha256=null,selectedCar=null,cityGlobals=null;if(${!!route}){let carEnd=0x6b0700;while(carEnd<0x6b0720&&m[carEnd])carEnd++;selectedCar=new TextDecoder().decode(m.slice(0x6b0700,carEnd));cityGlobals=[0x6b065c,0x6b0684].map(address=>{let end=address;while(end<address+32&&m[end])end++;return{address,value:new TextDecoder().decode(m.slice(address,end))};});if(cityGlobals.some(x=>x.value!==${JSON.stringify(route?.city ?? null)}))throw new Error('Original city globals differ from route');if(selectedCar!==${JSON.stringify(carId)})throw new Error('Selected original car differs from route: '+selectedCar);const archivePath='C:\\\\mm2core.ar',archive=fs.openSync(archivePath,0x80000000,3);if(!archive)throw new Error('Cannot read retail core archive');const archiveBytes=await fs.read(archive,fs.getFileSize(archivePath));archiveSha256=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',archiveBytes)),b=>b.toString(16).padStart(2,'0')).join('');if(archiveSha256!=='d4768421c7435faba1e17773a39053227deb87ffbcbba2931cf47b061c21505e')throw new Error('City route requires unchanged retail core archive');}
return {display,expected_display:expectedDisplay,executable_sha256:sha256,signatures_verified:true,archive_sha256:archiveSha256,requested_city:${JSON.stringify(route?.city ?? null)},city_globals:cityGlobals,city_global_roles_verified:false,selected_car:selectedCar,loaded_city_verified:false};`);
    const hit = await pageEval(session, `(async()=>{await __BS__.harness.clearBreaks();await __BS__.harness.resume();return await __BS__.harness.breakOn(0x405760,{fast:true,pause:true,capture:{backtrace:false,stack:0}});})()`, { timeoutMs: 15000 });
    const player = Number(hit.hit.callsite.regs.ecx);
    await pageEval(session, "__BS__.harness.clearBreaks()", { timeoutMs: 15000 });
    await worker(`
const sys=System.getInstance(),p=sys.process,m=p.getCurrentMemory(),d=new DataView(m.buffer,m.byteOffset,m.byteLength);
if(!p.moduleRegistry.modules.has('midtown2.exe'))throw new Error('Requires the documented retail original');
state.oracle={id:${JSON.stringify(captureId)},process:p,player:${player},car:d.getUint32(${player}+0x284,true),rows:[],ticks:0,lastFrame:-1,scenario:${JSON.stringify(scenario)},carId:${JSON.stringify(carId)},scene:${JSON.stringify(scene)},fixtureHash:${JSON.stringify(fixtureHash)},provenance:${JSON.stringify(provenance)},pose:${JSON.stringify(pose ?? null)},expectedInitialUp:${JSON.stringify(expectedInitialUp ?? null)},baseline:${JSON.stringify(baseline)},settleFrames:${settleFrames},wheelDiagnostics:${wheelDiagnostics},city:${JSON.stringify(route?.city ?? null)},route:${JSON.stringify(route)},routeSha256:${JSON.stringify(routeSha256)},captureFrames:${captureFrames},schedule:${JSON.stringify(schedule)}};
const o=state.oracle;const a=o.car;
if(o.route||${!!expectedDisplay}){const display={width:sys.ddrawContext?.display?.width??null,height:sys.ddrawContext?.display?.height??null},expected=${JSON.stringify(expectedDisplay ?? null)},instance=d.getUint32(a+0x1d0,true);o.provenance.pre_pose={player:o.player,car:a,instance,pos:[0x90,0x94,0x98].map(x=>d.getFloat32(a+x,true)),pos_convention:'center_of_mass',up:[0x78,0x7c,0x80].map(x=>d.getFloat32(a+x,true)),instance_room:d.getInt16(instance+6,true),display,traffic:{address:0x6b05f8,value:d.getFloat32(0x6b05f8,true)},cops:{address:0x6b0600,value:d.getFloat32(0x6b0600,true)}};if(expected&&(display.width!==expected[0]||display.height!==expected[1]))throw new Error('Original display changed before capture pose: '+JSON.stringify(display));}
const expectedDensities={traffic:${JSON.stringify(expectedTraffic)},cops:${JSON.stringify(expectedCops)}};for(const[name,target]of Object.entries(expectedDensities)){const address=name==='traffic'?0x6b05f8:0x6b0600,actual=d.getFloat32(address,true);if(target!==null&&actual!==target)throw new Error('Original '+name+' density mismatch before pose: expected '+target+', got '+actual);}const launch=state.polishLaunchScene;if(launch?.process===p)o.provenance.launch_scene_setup={boundary:launch.boundary,trafficBefore:launch.trafficBefore,copsBefore:launch.copsBefore};
o.expectedTuning=${JSON.stringify(expectedTuning)};o.totalColdSettleFrames=o.settleFrames;const selection=state.originalTemporarySelection;o.vehicleSelection=selection?.process===p?{additionalRegistryFlags:selection.additionalRegistryFlags??[],name:selection.name,originalRegistryName:selection.originalRegistryName??selection.name,selectionMethod:selection.selectionMethod??"model_buffer",registryAddress:selection.address,originalLocked:selection.locked,originalPaintLocked:selection.paintLocked,selectedPaint:0}:null;o.rendererDiagnostic=sys.ddrawContext?.executor?.getDebugFlags?.()??null;
if(o.baseline==='restore'){const b=state.originalOracleBaseline;if(!b||b.process!==p||b.carId!==o.carId||b.car!==a||b.player!==o.player)throw new Error('Saved baseline must belong to this same guest player and car');o.totalColdSettleFrames=b.totalColdSettleFrames==null?null:b.totalColdSettleFrames+o.settleFrames;m.set(b.bytes,a);d.setInt16(d.getUint32(a+0x1d0,true)+6,b.room,true);const input=d.getUint32(0x6b0cd0,true);d.setUint32(input+0x1d4,b.pedalsSwapped,true);}
if(o.pose){const[x,y,z,degrees]=o.pose,t=degrees*Math.PI/180,c=Math.cos(t),s=Math.sin(t),rows=[c,0,-s,0,1,0,s,0,c];const cog=[0,4,8].map(i=>d.getFloat32(a+0x204+i,true)),model=[x,y,z],rotatedCog=[c*cog[0]+s*cog[2],cog[1],-s*cog[0]+c*cog[2]],com=o.route?model.map((v,i)=>v-rotatedCog[i]):model;if(cog.some(v=>!Number.isFinite(v)))throw new Error('Live center of gravity is not finite');rows.forEach((v,i)=>d.setFloat32(a+0x6c+i*4,v,true));o.poseConvention=o.route?'model_origin':'center_of_mass';o.initialModelPose=o.route?o.pose:null;o.initialComPose=[...com,degrees];o.liveCenterOfGravity=cog;com.forEach((v,i)=>d.setFloat32(a+0x90+i*4,v,true));for(const base of[0x54,0x60,0x9c,0xa8])for(let i=0;i<3;i++)d.setFloat32(a+base+i*4,0,true);const inst=d.getUint32(a+0x1d0,true);d.setInt16(inst+6,o.route?.initial_room??1,true);const instancePos=o.route?model:com;instancePos.forEach((v,i)=>d.setFloat32(inst+0x90+i*4,v,true));d.setUint32(a+0x20,1,true);}
const f=x=>d.getFloat32(a+x,true);o.preWarmState={rpm:f(0x2c4),engineOmega:f(0x2c0),gear:d.getUint32(a+0x304,true),pos:[f(0x90),f(0x94),f(0x98)],vel:[f(0x9c),f(0xa0),f(0xa4)]};if(o.baseline!=='restore'&&o.preWarmState.engineOmega!==0)o.totalColdSettleFrames=null;o.liveTuning={mass:f(0x244),inertiaBox:[f(0x228),f(0x22c),f(0x230)],horsepower:f(0x274),idleRpm:f(0x278),optimalRpm:f(0x27c),maxRpm:f(0x280),frontRadius:f(0x4b8+0x1bc),rearRadius:f(0x990+0x1bc)};if(o.expectedTuning){const expected=o.expectedTuning.live_tuning;for(const[key,actual]of Object.entries(o.liveTuning)){const target=expected[key];if(target===undefined)throw new Error('Expected tuning lacks '+key);const values=Array.isArray(actual)?actual:[actual],targets=Array.isArray(target)?target:[target];if(values.length!==targets.length||values.some((v,i)=>typeof v!=='number'||!Number.isFinite(v)||typeof targets[i]!=='number'||!Number.isFinite(targets[i])||Math.abs(v-targets[i])>Math.max(0.001,Math.abs(targets[i])*1e-4)))throw new Error('Live tuning fingerprint mismatch at '+key);}o.carIdentityVerified=true;}o.initialLimits=[d.getFloat32(0x5cd830,true),d.getFloat32(0x5cd834,true),d.getFloat32(0x5cd838,true),d.getUint32(0x6a2c38,true)];
d.setFloat32(0x5cd830,1/60,true);d.setFloat32(0x5cd834,1/60,true);d.setFloat32(0x5cd838,1/60,true);d.setUint32(0x6a2c38,0,true);
for(const k of [37,38,39,40,32])sys.inputManager.injectKey(k,false);if(o.route)sys.inputManager.injectKey(32,true);
o.hooks=p.dispatcher.findStubsByName('dinput','IDirectInputDevice2A_GetDeviceState').map(s=>({id:s.functionId,fn:p.dispatcher.fastPathTable[s.functionId]}));
for(const h of o.hooks)p.dispatcher.fastPathTable[h.id]=function(esp,dv,mem8){
 const frame=dv.getUint32(0x6a2c30,true);
 if(dv.getUint32(esp+8,true)===256&&!o.done&&frame!==o.lastFrame){
  o.lastFrame=frame;
  const n=o.ticks++-o.settleFrames;
  if(n===0){if(o.route)sys.inputManager.injectKey(32,false);const a=o.car;const v=Math.hypot(...[0x9c,0xa0,0xa4].map(x=>dv.getFloat32(a+x,true)));const up=[0x78,0x7c,0x80].map(x=>dv.getFloat32(a+x,true)),ground=[0x4b8,0x724,0x990,0xbfc].map(x=>dv.getUint8(a+x+0x226)),normals=[0x4b8,0x724,0x990,0xbfc].map(x=>[0x18c,0x190,0x194].map(y=>dv.getFloat32(a+x+y,true)));const ref=o.expectedInitialUp,postureOk=ref?up.reduce((dot,x,i)=>dot+x*ref[i],0)/(Math.hypot(...ref)*Math.hypot(...up))>=Math.cos(Math.PI/180):up[1]>=0.99;const normalsOk=o.scene!=='flat-track-archive-replacement'||normals.every(x=>Math.abs(x[0])<0.001&&x[1]>0.999&&Math.abs(x[2])<0.001);o.initialGateObserved={speed:v,up,ground,normals,expectedInitialUp:ref,angularToleranceDegrees:ref?1:null};if(!Number.isFinite(v)||up.some(x=>!Number.isFinite(x))||v>0.1||!postureOk||!normalsOk||ground.some(x=>x!==1)){o.done=true;o.error='Initial state must be stationary with expected settled posture, four grounded wheels and verified course normals; reset the cruise before capturing';globalThis.__harnessPause();}else if(o.baseline==='save'){const inst=dv.getUint32(a+0x1d0,true);state.originalOracleBaseline={totalColdSettleFrames:o.totalColdSettleFrames,process:p,carId:o.carId,car:a,player:o.player,bytes:new Uint8Array(mem8.slice(a,a+0x1560)),room:dv.getInt16(inst+6,true),pedalsSwapped:dv.getUint32(dv.getUint32(0x6b0cd0,true)+0x1d4,true)};}}
  if(n>=0&&!o.error){const a=o.car,f=x=>dv.getFloat32(a+x,true),v=x=>[f(x),f(x+4),f(x+8)];
   const diagnostics=o.wheelDiagnostics?{ics:{force:v(0xb4),torque:v(0xc0),force2:v(0xcc),torque2:v(0xd8),impulse:v(0xe8),angularImpulse:v(0xf4),netPush:v(0x100),netTurn:v(0x10c),totalPush:v(0x118),lastTotalPush:v(0x124)},wheels:[0x4b8,0x724,0x990,0xbfc].map(w=>({x:f(w+0x1f8),Fs:f(w+0x1fc),N:f(w+0x200),xdot:f(w+0x204),D:f(w+0x208),vLat:f(w+0x170),vFwd:f(w+0x174),vN:f(w+0x178),vSlip:f(w+0x17c),dLat:f(w+0x228),dLong:f(w+0x22c),FLat:f(w+0x230),FLong:f(w+0x234),wheelMatrix:Array.from({length:12},(_,i)=>f(w+0x24+i*4)),center:v(w+0x1b0),lastHit:v(w+0x1a4),contactPoint:v(w+0xc0),probeNormal:v(w+0xcc),probeFraction:f(w+0xd8),rawScratch15cThrough170:Array.from({length:6},(_,i)=>f(w+0x15c+i*4)),lateralDir:v(w+0x180),rearDir:v(w+0x198),radius:f(w+0x1bc),slipVisual:f(w+0x220),bottomedOut:dv.getUint8(a+w+0x227)}))}:undefined;
   const cog=v(0x204),modelPos=v(0x90).map((x,i)=>x+f(0x6c+i*4)*cog[0]+f(0x78+i*4)*cog[1]+f(0x84+i*4)*cog[2]),instance=dv.getUint32(a+0x1d0,true);
   let sourceSkidAudio;
   const captureSkidBackend=sound=>{try{
    const valid=(ptr,size)=>Number.isInteger(ptr)&&ptr>=0x10000&&ptr+size<=mem8.byteLength;
    if(!valid(sound,0x30))return{available:false,reason:'Unreadable sound'};
    const manager=dv.getUint32(sound+0x20,true),handles=dv.getUint32(sound+0x28,true),index=dv.getInt32(sound+0x2c,true);
    if(!valid(manager,0xcc)||index<0||index>1024||!valid(handles+index*4,4))return{available:false,reason:'Unreadable current handle'};
    const handle=dv.getInt32(handles+index*4,true);
    for(const listOffset of[0xc4,0xc8]){let node=dv.getUint32(manager+listOffset,true);const visited=new Set();for(let steps=0;steps<1024&&valid(node,0x4c)&&!visited.has(node);steps++){
      visited.add(node);if(dv.getInt32(node+0x48,true)===handle){const callback=dv.getUint32(node+0x28,true),wrapper=dv.getUint32(node+0x14,true);
       if(callback!==0x5a41c0||!valid(wrapper,12))return{available:false,handle,callback,reason:'Not a readable DirectSound voice'};
       const bufferAddress=dv.getUint32(wrapper+8,true),dsound=p.modules.get('dsound'),buffer=dsound?.objects?.get(bufferAddress)?.buffer??(typeof dsound?.getBuffer==='function'?dsound.getBuffer(bufferAddress):null);
       if(!buffer)return{available:false,handle,callback,bufferAddress,reason:'DirectSound backend voice not found'};
       return{available:true,handle,callback,bufferAddress,isPlaying:buffer.isPlaying,isLooping:buffer.isLooping,volumeDb:buffer.volumeDb,linearVolume:buffer.volume,frequency:buffer.frequency};
      }node=dv.getUint32(node,true);
    }}return{available:false,handle,reason:'Handle not in bounded manager lists'};
   }catch(error){return{available:false,reason:'Backend diagnostic failed: '+String(error)};}};
   if(o.route){const readable=(ptr,length)=>Number.isInteger(ptr)&&ptr>=0x10000&&ptr+length<=mem8.byteLength,container=dv.getUint32(o.player+0x280,true),audio=readable(container,12)?dv.getUint32(container+8,true):0,surface=audio+0x80,q=Math.max(...[0x4b8,0x724,0x990,0xbfc].map(w=>f(w+0x220)));sourceSkidAudio={available:false,q,reason:'No readable regular vehCarAudio'};if(audio&&readable(surface,0x84)){const surfaceClass=dv.getInt32(surface,true),previousClass=dv.getInt32(surface+4,true),playingGate=dv.getInt32(surface+8,true),count=dv.getInt32(surface+0xc,true),dataArray=dv.getUint32(surface+0x5c,true),data=surfaceClass>=0&&surfaceClass<count&&count<=64&&readable(dataArray,count*4)?dv.getUint32(dataArray+surfaceClass*4,true):0;sourceSkidAudio={available:true,q,qGate:q>0,surfaceClass,previousClass,playingGate,backendPlaybackSampling:true,requestedGainIsPrediction:true};if(data&&readable(data,0x2c)){const waveData=dv.getUint32(data+4,true),sampleCount=dv.getInt32(data+0xc,true),minVolume=dv.getFloat32(data+0x20,true),maxVolume=dv.getFloat32(data+0x24,true),volumeSlope=dv.getFloat32(data+0x28,true);Object.assign(sourceSkidAudio,{minVolume,maxVolume,volumeSlope,requestedGainIfCalled:q*volumeSlope+minVolume,bands:sampleCount>=0&&sampleCount<=64&&readable(waveData,sampleCount*0x10)?Array.from({length:sampleCount},(_,i)=>{const at=waveData+i*0x10,minimumSlip=dv.getFloat32(at,true),maximumSlip=dv.getFloat32(at+4,true);return{minimumSlip,maximumSlip,soundAddress:dv.getUint32(at+8,true),backend:captureSkidBackend(dv.getUint32(at+8,true)),sampleIndex:dv.getInt32(at+0xc,true),inclusiveSelection:q>0&&q>=minimumSlip&&q<=maximumSlip};}):null});}}}
   o.rows.push({sourceSkidAudio,instanceRoom:dv.getInt16(instance+6,true),modelPos,slipVisual:[0x4b8,0x724,0x990,0xbfc].map(w=>f(w+0x220)),diagnostics,n,t:n/60,frame:dv.getUint32(0x6a2c30,true),dt:dv.getFloat32(0x5cd820,true),pos:v(0x90),vel:v(0x9c),omega:v(0xa8),forward:v(0x84),right:v(0x6c),up:v(0x78),speed:f(0x248),steer:f(0x1554),brake:f(0x154c),hb:f(0x1550),throttle:f(0x2bc),rpm:f(0x2c4),gear:dv.getUint32(a+0x304,true),spin:f(0x40c),ground:[0x4b8,0x724,0x990,0xbfc].map(x=>dv.getUint8(a+x+0x226)),surfaceFriction:[0x4b8,0x724,0x990,0xbfc].map(x=>f(x+0x1d0)),contactNormal:[0x4b8,0x724,0x990,0xbfc].map(x=>v(x+0x18c))});
   for(const [at,keys]of o.schedule)if(at===n)for(const[k,down]of keys)sys.inputManager.injectKey(k,down);
   if(n>=o.captureFrames){o.done=true;for(const k of [37,38,39,40,32])sys.inputManager.injectKey(k,false);globalThis.__harnessPause();}
  }
 }
 return h.fn(esp,dv,mem8);
};
globalThis.__harnessResume();return {car:o.car};`);
    let finished = false;
    const deadline = Date.now() + (route ? 45 : 15) * 60_000;
    while (Date.now() < deadline) {
        await Bun.sleep(2000);
        const progress = await worker("return {done:state.oracle.done===true,frames:state.oracle.rows.length};");
        if (progress.done) { finished = true; break; }
    }
    const result = await worker(readResultCode);
    await Bun.write(output, JSON.stringify(result, null, 2));
    if (result.error) throw new Error(result.error);
    if (!finished) throw new Error(`Capture timed out; partial data saved to ${output}`);
    console.log(`Captured ${result.rows.length} frames to ${output}; original is paused.`);
} catch (error) {
    try {
        const partial = await worker(`if(state.oracle?.id!==${JSON.stringify(captureId)})return null;${readResultCode}`);
        if (partial) await Bun.write(output, JSON.stringify({...partial,complete:false,error:String(error)}, null, 2));
    } catch (saveError) {
        console.error("Original partial capture could not be saved:", saveError);
    }
    throw error;
} finally {
    await worker(`const o=state.oracle;if(o?.id===${JSON.stringify(captureId)}){globalThis.__harnessPause();const p=System.getInstance().process,m=p.getCurrentMemory(),d=new DataView(m.buffer,m.byteOffset,m.byteLength);for(const h of o.hooks??[])p.dispatcher.fastPathTable[h.id]=h.fn;for(const k of [37,38,39,40,32])System.getInstance().inputManager.injectKey(k,false);if(o.initialLimits){[0x5cd830,0x5cd834,0x5cd838].forEach((a,i)=>d.setFloat32(a,o.initialLimits[i],true));d.setUint32(0x6a2c38,o.initialLimits[3],true);}}return {restored:true};`).catch((error: unknown) => { console.error('Original capture cleanup failed:', error); process.exitCode = 1; });
    session.close();
    process.exitCode = process.exitCode ?? 0;
}
process.exit();
