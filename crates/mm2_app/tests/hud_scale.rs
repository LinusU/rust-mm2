//! F22-AC06 scaling legs: the HUD instruments laid out by Bevy's real
//! UI layout pass at every text size and common window sizes, none of
//! them clipped by the window edge or covering another.
//!
//! The render-target plumbing (a camera needs a GPU) is the one thing
//! replaced: `feed_target` writes the target's scale factor — window
//! scale factor times the persisted [`UiScale`], exactly what
//! `propagate_ui_target_cameras` computes — and physical size, then the
//! production `ui_layout_system` runs. Image nodes measure through
//! `update_image_content_size_system`. Text nodes (the stat panel's
//! `PLACE`/`CHECKPOINTS` labels) need a font backend this harness does
//! not mount, so their extent is not covered here.

use std::path::Path;

use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use bevy::reflect::structs::Struct;
use bevy::text::FontCx;
use bevy::ui::widget::update_image_content_size_system;
use bevy::ui::{ComputedUiRenderTargetInfo, ui_layout_system, ui_surface::UiSurface};
use mm2_app::hud::HudVisible;
use mm2_app::racestat::spawn_race_stats;
use mm2_app::racetime::{spawn_race_timer, update_race_timer};
use mm2_app::settings::TextSize;
use mm2_app::speedometer::spawn_speedometer;
use mm2_assets::Vfs;
use mm2_game::{
    Checkpoint, CheckpointRule, EventParams, RaceDefinition, RacePhase, RaceStart, RaceState,
    Session, SessionConfig, SessionEntity, SessionPhase,
};

#[derive(Resource)]
struct Target {
    scale_factor: f32,
    physical: UVec2,
}

/// Stand in for `propagate_ui_target_cameras`: its two fields are
/// crate-private, so they are set through reflection.
fn feed_target(
    target: Res<Target>,
    ui: Res<UiScale>,
    mut infos: Query<&mut ComputedUiRenderTargetInfo>,
) {
    for mut info in &mut infos {
        let s: &mut dyn Struct = info.as_mut();
        *s.field_mut("scale_factor")
            .and_then(|f| f.try_downcast_mut::<f32>())
            .expect("scale_factor is an f32") = target.scale_factor * ui.0;
        *s.field_mut("physical_size")
            .and_then(|f| f.try_downcast_mut::<UVec2>())
            .expect("physical_size is a UVec2") = target.physical;
    }
}

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn tga32(w: u16, h: u16) -> Vec<u8> {
    let mut t = vec![0u8; 18];
    t[2] = 2;
    t[12..14].copy_from_slice(&w.to_le_bytes());
    t[14..16].copy_from_slice(&h.to_le_bytes());
    t[16] = 32;
    t[17] = 0x28;
    for _ in 0..(w as usize * h as usize) {
        t.extend_from_slice(&[0, 255, 0, 255]);
    }
    t
}

/// The authored glyph dimensions (41x56 full, 20x27 half) under the
/// stems the timer and stat panel resolve.
fn glyph_mount() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    for d in 0..10 {
        write(
            tmp.path(),
            &format!("texture/digitac_{d}.tga"),
            tga32(41, 56),
        );
        write(
            tmp.path(),
            &format!("texture/digitac_{d}_half.tga"),
            tga32(20, 27),
        );
    }
    write(tmp.path(), "texture/digi_colon.tga", tga32(13, 56));
    write(tmp.path(), "texture/digi_colon_half.tga", tga32(7, 27));
    tmp
}

fn cp(x: f32, z: f32) -> Checkpoint {
    Checkpoint {
        center: Vec3::new(x, 0.0, z),
        radius: 15.0,
        height: mm2_game::DEFAULT_CHECKPOINT_HEIGHT,
        heading_deg: 0.0,
        require_direction: false,
    }
}

/// Ordered, many gates: the tallest, widest stat panel (laps row plus a
/// two-digit gate list).
fn def() -> RaceDefinition {
    RaceDefinition {
        checkpoints: (0..12).map(|i| cp(i as f32 * 50.0, 0.0)).collect(),
        finish: None,
        rule: CheckpointRule::Ordered,
        laps: 3,
        time_limit_ticks: None,
        params: EventParams::default(),
        countdown_ticks: 360,
        start_slots: vec![RaceStart {
            position: Vec3::ZERO,
            yaw_deg: Some(0.0),
        }],
    }
}

/// A HUD with every image-built instrument spawned and the timer
/// composing its widest reading.
fn hud_app() -> App {
    let tmp = glyph_mount();
    let mut vfs = Vfs::new();
    vfs.mount_dir(tmp.path(), 0).unwrap();

    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Countdown).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    let generation = session.generation();

    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), TransformPlugin));
    app.init_asset::<Image>()
        .init_asset::<TextureAtlasLayout>()
        .insert_resource(session)
        .insert_resource(HudVisible(true))
        .init_resource::<UiSurface>()
        .init_resource::<UiScale>()
        .init_resource::<FontCx>()
        .insert_resource(Target {
            scale_factor: 1.0,
            physical: UVec2::new(1280, 720),
        })
        .add_systems(Update, update_race_timer)
        .add_systems(
            PostUpdate,
            (
                feed_target,
                update_image_content_size_system,
                ui_layout_system,
            )
                .chain(),
        );
    let mut race = RaceState::new(def(), generation);
    race.phase = RacePhase::Running;
    // Far past the 999:59:99 cap, so every slot is composed.
    race.clock = 999 * 60 * 100 * 1000;
    app.insert_resource(race);

    let world = app.world_mut();
    world.resource_scope(|w, mut images: Mut<Assets<Image>>| {
        let mut queue = CommandQueue::default();
        let mut commands = Commands::new(&mut queue, w);
        let timer = spawn_race_timer(&mut commands, &vfs, &mut images, SessionEntity(1));
        let stats = spawn_race_stats(&mut commands, &vfs, &mut images, SessionEntity(1), &def());
        spawn_speedometer(&mut commands, &mut images, SessionEntity(1));
        assert!(timer.absent.is_none() && stats.absent.is_none());
        queue.apply(w);
    });
    app
}

/// Every root instrument's rect in physical pixels: `(left, top,
/// right, bottom)`, from the layout's centre and size.
fn root_rects(app: &mut App) -> Vec<(Entity, Rect)> {
    let mut q = app
        .world_mut()
        .query_filtered::<(Entity, &ComputedNode, &UiGlobalTransform), Without<ChildOf>>();
    q.iter(app.world())
        .filter(|(_, c, _)| c.size.x > 0.0 && c.size.y > 0.0)
        .map(|(e, c, t)| (e, Rect::from_center_size(t.translation, c.size)))
        .collect()
}

#[test]
fn hud_instruments_stay_on_screen_and_apart_at_every_text_size() {
    let mut app = hud_app();
    // Logical window sizes; the physical target is the size times the
    // window scale factor.
    let windows = [
        (800u32, 600u32),
        (1024, 768),
        (1280, 720),
        (1920, 1080),
        (2560, 1080),
        (3840, 2160),
    ];
    for (w, h) in windows {
        for sf in [1.0_f32, 2.0] {
            let mut areas = Vec::new();
            for size in TextSize::ALL {
                app.world_mut().resource_mut::<UiScale>().0 = size.factor();
                *app.world_mut().resource_mut::<Target>() = Target {
                    scale_factor: sf,
                    physical: UVec2::new(w, h) * UVec2::splat(sf as u32),
                };
                // Three frames: target info, content size, then layout.
                for _ in 0..3 {
                    app.update();
                }
                let rects = root_rects(&mut app);
                let label = format!("{w}x{h} sf{sf} text {}", size.label());
                assert!(
                    rects.len() >= 3,
                    "{label}: timer, stats and speedometer are laid out ({rects:?})"
                );
                areas.push(
                    rects
                        .iter()
                        .map(|(_, r)| r.size().x * r.size().y)
                        .sum::<f32>(),
                );
                let screen = Rect::new(-0.5, -0.5, w as f32 * sf + 0.5, h as f32 * sf + 0.5);
                for (e, r) in &rects {
                    assert!(
                        screen.contains(r.min) && screen.contains(r.max),
                        "{label}: {e:?} {r:?} is clipped by the window"
                    );
                }
                for (i, (ea, a)) in rects.iter().enumerate() {
                    for (eb, b) in &rects[i + 1..] {
                        let overlap = a.intersect(*b);
                        assert!(
                            overlap.is_empty(),
                            "{label}: {ea:?} {a:?} covers {eb:?} {b:?}"
                        );
                    }
                }
            }
            assert!(
                areas.windows(2).all(|p| p[1] > p[0]),
                "{w}x{h} sf{sf}: a larger text size must lay the HUD out larger ({areas:?})"
            );
        }
    }
}
