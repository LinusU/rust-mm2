# Mod coverage: which consumers honour an override

[modding.md](modding.md) describes the override *model* (the VFS, source
priority, extension preference). This page is the per-family audit behind
F29-AC01 (an override reaches the production consumer) and F29-AC05 (the
override is classified as gameplay or cosmetic): for each content family it
names the consumer, how a winner is chosen, and the test or command that
proves it. A family nothing proves is marked **unproven**, and one with no
consumer **unsupported**; neither is assumed to work.

All tests below use self-authored synthetic data (`cargo test -p mm2_app
--test app <name>`; `net_host.rs` is in `--test network`) so they run in CI. None reads
the retail install, so the evidence level is *synthetic integration*
(specs/QUALITY-GATES.md), not original-content validation.

## Priority rule shared by every row

Every consumer reaches data through `Vfs::resolve*` / `read`, so the winner is
always `mods (300+) > override dirs (200) > loose install (100) > .ar (0)`,
later mount wins on a tie, and extension preference applies only within one
source ([modding.md](modding.md#resolution-rules)). A row's "priority rule"
column therefore only records what is *additional*: the extension list the
consumer asks for, or a name-keyed index. That no production code opens an
original file outside the VFS is held by `tests/vfs_audit.rs` (a source scan;
it proves where the filesystem is reached from, not that a consumer honours an
override). Gameplay/cosmetic classification is `fingerprint::is_gameplay_path`
for every row ([modding.md](modding.md#gameplay-versus-cosmetic-mods)).

Files: `mo` = `crates/mm2_app/tests/mod_override.rs`,
`mor` = `mod_override_race.rs`, `sf` = `surface.rs`, `menu` = `menu.rs`,
`sess` = `session.rs`.

## Proven families

| Family | Logical path(s) | Consumer | Extra priority rule | Class | Proof |
|---|---|---|---|---|---|
| Vehicle texture | `texture/<stem>.{png,tga,ktx2,tex}` | `city::MaterialCache::get` (car bodies and props alike) | `TEXTURE_EXTS` preference within a source; a mod's `.tex` beats the install's `.png` | cosmetic | override, bystander car unmoved, revert: `mo::a_mod_replaces_texture_handling_prop_and_cue_through_their_consumers`; malformed/undecodable `.png`/`.tex`, install skin does not stand in: `mo::a_malformed_selected_override_is_reported_not_papered_over` |
| Vehicle handling / tune | `tune/vehicle/<id>.vehcarsim` | `mm2_content::load_vehicle` | exact path | gameplay | same first test (retuned car only); malformed tune is an error, sibling loads: second test; provenance: `mo::a_traced_load_names_the_mod_files_each_consumer_pulled_in`; `mm2-inspect deps <install> <car> --expect-mod` |
| World prop model | `geometry/<prop>.pkg` | `city::load_city` prop stamping | exact path | gameplay | city vertices change, rest of city untouched, revert: first test; broken PKG counted in `props_failed`, base prop does not stand in: `mo::a_malformed_selected_override_…` |
| City geometry | `city/<name>.psdl` | `city::load_city`, `city::trace_city_reads` | exact path; sibling files use the case-insensitive rule in modding.md | gameplay | `mo::a_traced_city_load_credits_the_mod_that_replaced_its_geometry` (replace, garbage refused, missing city); process level: `mo::the_trace_deps_flag_reports_and_checks_which_mod_serves_a_city`, `…trace_deps_refuses_a_mods_directory_that_did_not_mount`; `mm2 --city <stem> --trace-deps [--expect-mod]` |
| Audio cue (car, surface, siren waves) | `aud/aud22/**/<stem>.22k.wav` and siblings | `audio::WaveBank::{index,load,load_siren}` | stem index, 22 kHz tier first (`audio.rs` tier preference); sirens go through the `sirens/`-scoped map | cosmetic | replace/revert, bystander cue unmoved: first test; truncated and non-PCM encodings refused: `mo::a_malformed_selected_override_…`; stale index after remount refused: `mo::a_wave_index_built_before_a_remount_is_refused_not_answered_stale`; siren scoping: `mo::a_traced_siren_cue_follows_the_sirens_scoped_map`; `mm2 --trace-deps --cue <stem> [--siren-cue <stem>]` (`mo::a_traced_cue_load_…`, `mo::the_trace_deps_flag_reports_and_checks_which_mod_serves_a_cue`) |
| Race rules: Checkpoint table row, waypoints | `race/<city>/mmracedata.csv`, `race<N>waypoints.csv` | `race::event_race_setup` (catalog scan → `race_definition`) | exact path | gameplay | `mor::a_mod_replaces_a_races_table_row_and_course_through_the_race_consumer`, `…race_rule_mods_are_gameplay_and_move_the_fingerprint`, `…a_conflicting_race_record_names_the_mod_the_consumer_loaded`, `…a_malformed_race_override_is_refused_not_replaced_by_the_stock_course`; `mm2-inspect deps <install> --city <c> --event <table>:<row>` |
| Race rules: Blitz / Circuit tables, start grids | `race/<city>/mmblitzdata.csv`, `mmcircuitdata.csv`, `circuit<N>waypoints.csv`, `cir<N>_strtpnts` | `race::event_race_setup` | exact path; out-of-range values refused | gameplay | `mor::a_mod_replaces_a_blitz_time_limit_a_circuit_lap_count_and_a_start_grid`, `…blitz_and_circuit_record_mods_are_gameplay_…`, `…an_out_of_range_blitz_or_circuit_value_is_refused_…` |
| Opponent roster and routes | `race/<city>/*.aimap` `[Opponent]`, `*.opp` | `race::event_race_setup` roster build (`mm2_content::opponents`) | exact path | gameplay | `mor::a_mod_replaces_who_drives_and_where_through_the_roster_consumer`, `…roster_mods_are_gameplay_…`, `…a_roster_override_naming_a_dead_route_is_reported_…`, `…a_malformed_roster_override_is_refused_…` |
| Crash Course lessons (all difficulties) | `race/<city>/mmcrashdata.csv`, `crash<N>data[_p].csv`, `crash<N>.aimap`, lesson routes | `race::lesson_race_setup` / `LessonDriver` | exact path; Professional reads the `_p` files, Amateur does not | gameplay | `mor::a_mod_replaces_a_lessons_legs_lead_car_and_route_…`, `…lesson_mods_are_gameplay_…`, `…a_mod_for_one_lesson_difficulty_moves_only_that_difficulty`, `…professional_lesson_mods_…`, `…a_malformed_professional_lesson_aimap_is_refused_…`; launch through the session: `lesson_launch.rs` |
| Surface / material tables | `city/materials.mtl`, `city/materials.csv` (`surface::MTL_PATH`/`CSV_PATH`) | `mm2_content::surface` via the city collider build | pair must be both present and parse | gameplay | `sf::a_surface_override_moves_physics_without_touching_any_texture`, `sf::a_texture_swap_does_not_move_the_physics_surface`, `sf::a_malformed_surface_override_is_refused_not_papered_over`; session level: `sess::a_broken_surface_pair_fails_the_dev_world_session`. The `sf` override test mounts its mod with `mount_dir` at 100, not `mount_mod`, so the mod-classification leg is `sf::a_malformed_…` |
| Menu art | `menu/city-night.png`, `menu/london-night.png` | `menu_graphics::menu_art_image` | embedded art is the fallback only when no override is selected; a selected one that fails to decode shows nothing | cosmetic (`menu/` is not a gameplay prefix) | `mo::a_mod_replaces_menu_art_and_a_malformed_one_is_refused` |
| City/race display names | `tune/<city>.cinfo` | `mm2_content::city_info`, read by the menu | exact path | cosmetic (the one `tune/` exception) | `menu::a_mods_cinfo_renames_the_races_and_is_cosmetic`, `menu::a_malformed_cinfo_override_is_not_papered_over_by_the_stocks_names` |
| Vehicle model and bound | `geometry/<id>.pkg`, `bound/<id>_bound.bnd` | `mm2_content::load_vehicle` (`VehicleModel`, chassis size) | exact path | gameplay | `mo::a_mod_replaces_a_vehicles_model_and_bound_through_load_vehicle` (override, bystander, revert, fingerprint); broken PKG refused, base does not stand in: `mo::a_malformed_vehicle_pkg_override_is_refused_and_the_base_does_not_stand_in`. a malformed `.bnd` is a `ConversionReport` warning (trailer likewise; ambient `ambient_vehicle` errors): `mo::a_malformed_vehicle_bound_override_is_reported_and_the_base_does_not_stand_in`. **Known gap:** `.mtx` wheel placement is not covered |
| Vehicle audio table (`<id>.csv`) | `aud/cardata/player/<id>.csv` | `mm2_content::load_vehicle` → `VehicleDef::audio` | exact path | cosmetic (`aud/`) | `mo::a_mod_replaces_a_vehicles_audio_table_and_it_is_cosmetic` (override, bystander, revert, fingerprint, malformed table drops audio and the base's does not stand in). The `default_*surface`/`impacts` and engine tables of `SurfaceAudio::load` remain unproven |
| Ambient and music cue tables | `aud/dmusic/csv_files/singleroam.csv`, `<city>ambience.csv` | `music::music_state` → `MusicReport` | exact path | cosmetic (`aud/`) | `mo::a_mod_replaces_the_music_cue_tables_and_it_is_cosmetic` (override, city-ambience bystander, revert, fingerprint; a header-less override is reported `missing`, the base's rows do not stand in. A table with bad columns or ragged rows only yields a `debug!` diagnostic, so it is counted, not refused) |
| Siren programs | `aud/cardata/player/<city>policesiren.csv`, `opponent/policesiren.csv` | `audio::SirenAudio::load` | exact path, city stem keys the player file | cosmetic (`aud/`) | `mo::a_mod_replaces_a_siren_program_through_siren_audio` (override, other-city and opponent bystanders, revert, fingerprint; malformed table warns and leaves no player program, the base's does not stand in) |
| Banger records | `tune/banger/<name>.dgbangerdata` | `banger::BangerDefs::get` | exact path, name lowercased | gameplay | `mo::a_mod_replaces_a_banger_record_through_banger_defs` (override, bystander, unbound name, revert, fingerprint, undecodable record counted in `failed` and stamps unbound) |
| Speech: environmental and Cops & Robbers commentary | `aud/spchdata/<city>.csv`, `aud/spchdata/<speaker>/<table>_prerace.csv`, `aud/spchdata/cnr<city>.csv` | `audio::CommentaryAudio` via `commentary_voices` (registry → speaker → cue table → wave); `cnrvoice::CnrCommentary::{load,draw}` | exact path | cosmetic (`aud/`) | `mo::a_mod_replaces_the_speech_tables_through_the_commentary_consumer_and_it_is_cosmetic` (spoken stems change for the replaced table only, time-of-day bystander, revert, fingerprint, mod report), `mo::a_malformed_speech_override_is_counted_and_the_base_does_not_stand_in` (unusable table and zero-announcer registry count `failed`), `mo::a_mod_replaces_the_cnr_commentary_table_and_a_malformed_one_is_refused` (override, other-city bystander, revert, fingerprint; empty table binds no announcer, a table without the family draws an error). Lesson tables (`ccl/*.csv`, call-outs) share the loader but are not run as a mod |
| Pedestrian walker screams | `aud/creaturedata/default_{f,m}pedvoice1.csv` | `audio::PedVoiceAudio::load` → `pedestrian_voices` (table → sample → `WaveBank`) | exact path, one table per gender | cosmetic (`aud/`) | `mo::a_mod_replaces_a_pedestrian_voice_table_through_the_walker_scream_consumer` (override of the female table, male bystander, revert, fingerprint, mod report); `mo::a_malformed_pedestrian_voice_override_is_silent_and_the_base_does_not_stand_in` (a table with no usable row, or naming a missing wave, silences that gender and the base's sample does not stand in; a missing wave counts `AudioReport::failed`, a table with no usable row only warns). The numbered voice files (`…pedvoice2+`) are not read by the consumer |
| Pedestrian archetype: animation, mesh, rig, states, paints | `anim/<stem>.{skel,mod,csv,shaders}`, `anim/<clip>.anim` | `mm2_content::ped::PedArchetype::load` (the loader behind `pedestrian` and `crowd`) | exact path | gameplay (`anim/`) | `mo::a_mod_replaces_pedestrian_animation_through_the_archetype_loader_and_it_is_gameplay` (replaced clip and shader table, bystander clip, revert, fingerprint moves, mod report); broken clip, mesh, skeleton, state model and shader table each refuse the archetype with an error and the base member does not stand in. The app-level actors (`spawn_ped_lab`, `crowd`) are not run as a mod |
| Chase and cockpit camera records | `tune/camera/<car>_{near,far}.camtrackcs`, `tune/camera/<car>_dash.campovcs` | `camera::load_track_cams`, `dash::load_pov_cam` | exact path, one record per lens | gameplay (the whole `tune/` prefix except `.cinfo`, although these records only steer presentation; classification question filed) | `mo::a_mod_replaces_camera_tuning_through_the_camera_and_dash_loaders` (near lens and cockpit eye replaced, far lens and other car bystanders, revert, fingerprint, mod report); `mo::a_malformed_camera_override_does_not_let_the_base_stand_in` (the record reads as absent and the base's does not stand in; an unparseable record gives no diagnostic) |
| Wheel-effect and rain particle rules | `tune/effects/<name>.asbirthrule` (eight `PTX_RULE_NAMES`), `tune/rain.asbirthrule` | `wheel_fx::wheel_fx_session`, `precip::precip_session` | exact path | gameplay (the whole `tune/` prefix except `.cinfo`, although these records only steer presentation; classification question filed) | `mo::a_mod_replaces_wheel_effect_and_rain_rules_through_their_session_binders` (dust rule and rain rule replaced, the other seven effects bystanders, revert, fingerprint, mod report); `mo::a_malformed_effect_override_is_counted_and_the_base_does_not_stand_in` (a bad wheel rule counts `failed` and leaves its slot dark; a bad rain rule reports `absent: rule unparseable`). The `ptx_wheel`/`ptx_rain` atlases go through `city::load_image` (texture row) and are not run here |
| Mod conflict / classification plumbing | any | `Vfs::explain`, `fingerprint::{gameplay,mod_reports}` | later mount wins | n/a | `mo::the_later_of_two_conflicting_mods_wins`, `mo::a_conflict_explanation_names_the_mod_the_consumer_loaded`, `mo::the_gameplay_fingerprint_separates_physics_edits_from_cosmetic_ones`, `mo::each_mod_is_classified_the_way_its_consumer_and_the_fingerprint_see_it`, `mo::the_shipped_example_mods_move_the_fingerprint_as_their_manifests_claim`; host/inspector parity in `net_host.rs` |

## Read through the VFS, but no mod-override run proves it (unproven)

These have a production consumer that calls `Vfs::resolve*`/`read`, so a mod
*should* win by the shared priority rule, and `vfs_audit.rs` shows nothing
bypasses the VFS. No test mounts a mod and observes the consumer's output, so
AC01 is **not** claimed for them. Each is a candidate for the next
synthetic-mod run (add it to `mod_override.rs`, not a new harness).

| Family | Logical path(s) | Consumer |
|---|---|---|
| Traffic, police and parked-car routes | `race/<city>/*.aimap` city aimap, police scan, `<city>/props.pathset`, `*parkedcar*` pathsets | `traffic::load_city_aimap`, `police::scan`, `traffic.rs` loaders. (Event-specific `.aimap` rosters are proven above.) |
| City companions | `.inst`, `.cpvs`, `.water`, `.chunks`, `_fog.csv`, `.ltNN`, `.sky`, `decals.pathset`, HUD map and sky-dome geometry | `city::load_city` (the PSDL itself is proven; its companions are read in the same call but no test replaces them) |
| Dash cluster and HUD map | `tune/<car>_dash.asnode`, `geometry/<car>_dash*.{pkg,mtx}` read by `dash::spawn_dash`; `tune/<city>.mmhudmap` and its tile packages read by `hudmap` | `dash::spawn_dash`, `hudmap` (both need a Bevy `Commands` world) |

## Unsupported (no consumer, by design)

- **Localization strings (`mmlang.dll`)**: PE resources in a native DLL, not
  VFS data; see [modding.md](modding.md#what-can-be-replaced-today). The
  engine keeps its own labels, so there is nothing to override. The only
  localization surface is `tune/<city>.cinfo` above.
- **glTF models (`.glb`)** and any importer for a modern geometry format: the
  VFS resolves such a file, but no importer exists, so a `geometry/*.glb` mod
  file is ignored.
- **Hot reload**: mounting is restart-only.

## Reading this table

"Proven" means a synthetic mod was mounted over a synthetic base, the
production consumer's *output* changed for that family only, a bystander
stayed put and (where listed) the base returned once the mod was unmounted.
It does not mean the retail data's version of the same family behaves
identically: retail-data confirmation of a mod override is not claimed
anywhere above. F29-AC01 is therefore met for the "proven" rows and open for
the "unproven" ones; F29-AC05 is met wherever the fingerprint assertion is
listed (`mo::the_gameplay_fingerprint_…`, `mor::*_are_gameplay_and_move_the_fingerprint`).
