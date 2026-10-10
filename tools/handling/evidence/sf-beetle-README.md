# SF Beetle stutter investigation

The keyboard playtest of the yellow Beetle exposed continuous visual jitter
on straight roads. The chase camera ran in `Update` using `GlobalTransform`,
while Avian had already interpolated the root `Transform` for rendering.
Global propagation follows in `PostUpdate`. The camera therefore targeted a
stale pose, producing varying camera-relative displacement.

The fix samples the current root `Transform` with disjoint player/camera
queries. Player vehicles are root entities. The regression
`chase_follows_current_render_pose_before_global_propagation` deliberately
keeps the global pose stale and verifies current translation and rotation
are used. Physics, ride height, keyboard inputs and graphics settings are
unchanged.

## Rendered measurement

`sf-beetle-camera-timing.json` retains the per-frame target observations from
two native Metal runs at the same default Retina window settings, using
`--city sf --car vpbug --seq --no-profile`. A temporary observer ordered after
`chase_follow` logged the root render pose, global pose and camera's
`last_pos`; it was removed after measurement. Over simulation seconds 6–12,
268 baseline frames had a maximum target/render displacement of 0.4865 m;
258 corrected frames had exactly zero displacement. This measures the camera
pose dependency, not subjective perception or a claim of constant 60 FPS.
Frame times and raw-log hashes are included. The native renderer remains
roughly 43–45 FPS over the measured window at these settings; this change
does not improve rendering throughput.

## Original city measurement

`sf-beetle-hill-samples.json` retains all 901 decoded paired observations from
a 60 Hz original-game SF launch: 600 explicit held-handbrake settlement
updates, then 900 full-throttle updates. The source player was placed at
COM `(-1319.45, 68.05, 221.2775)`, yaw 180°, in room 687. Its total cold
constructor-to-settle count is unknown; no engine fields were forced. The
native model origin is 0.1 m lower to account for authored center of mass.
The native baseline is tick 599, hence source row n pairs with tick n+599.
Coordinates are the same in both runtimes (`MIRROR_Z=false`).

Settled COM height differs by 0.0000232 m. Through source row 666, vertical
position differs by at most 0.00415 m and speed by at most 0.0938 m/s.
Horizontal drift reaches 0.86 m, so this does not prove exact lateral
trajectory equivalence. The original also applies suspension positional
pushes up to 0.0825 m at hill transitions (native maximum 0.0656 m).
These measurements support preserving authored suspension and ride height.

From source row 667 the original has substantial body-contact impulses near
Z=490 that the native run does not reproduce. The obstruction's identity is
unknown. All 901 samples are retained, and the investigation reports both
the 667-row unobstructed prefix and the entire 901-row recording, including
the divergent impact suffix. No full-route collision equivalence is claimed.

`sf-beetle-stutter-investigation.json` includes executable/archive hashes,
comparison statistics and restoration proof. The original guest's hooks,
clock, held keys, selection and renderer were restored, then the guest was
freshly reloaded. Its archive was not modified. Only decoded numeric
observations are retained here; retail geometry and raw process memory stay
outside the repository.
