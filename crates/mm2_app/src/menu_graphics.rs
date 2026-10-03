//! Original arcade presentation. Rendering stays separate from menu rules.
use super::*;

use bevy::asset::RenderAssetUsages;

use bevy::camera::{RenderTarget, visibility::RenderLayers};

use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};

use bevy::render::render_resource::TextureFormat;

const GOLD: Color = Color::srgb(1.0, 0.78, 0.16);

const BLUE: Color = Color::srgb(0.25, 0.43, 0.95);

const INK: Color = Color::srgba(0.025, 0.035, 0.12, 0.94);

const GREEN: Color = Color::srgb(0.3, 0.82, 0.38);

const PREVIEW_LAYER: usize = 7;

#[derive(Component)]
struct Showroom;

#[derive(Component)]
pub struct Turntable;

/// Capture runs advance the showroom by a fixed step for repeatable images.
#[derive(Resource, Default)]
pub struct MenuPreviewCapture(pub bool);

#[derive(Default)]
pub struct Presentation {
    art: Handle<Image>,
    london: Handle<Image>,

    target: Handle<Image>,

    selection: Option<(String, usize)>,

    title: String,

    specs: String,

    loading: bool,
}

#[derive(bevy::ecs::system::SystemParam)]
pub struct Graphics<'w, 's> {
    images: ResMut<'w, Assets<Image>>,

    meshes: ResMut<'w, Assets<Mesh>>,

    materials: ResMut<'w, Assets<StandardMaterial>>,

    session: Res<'w, Session>,

    scene: Query<'w, 's, Entity, (With<Showroom>, Without<ChildOf>)>,

    state: Local<'s, Presentation>,
}

fn label(parent: &mut ChildSpawnerCommands, text: impl Into<String>, size: f32, color: Color) {
    parent.spawn((
        MenuUi,
        Text::new(text),
        TextFont {
            font_size: bevy::text::FontSize::Px(size),

            ..default()
        },
        TextColor(color),
        Node {
            width: Val::Percent(100.0),
            max_width: Val::Percent(100.0),
            ..default()
        },
    ));
}

fn panel() -> Node {
    Node {
        flex_direction: FlexDirection::Column,

        padding: UiRect::all(Val::Vw(1.5)),

        border: UiRect::all(Val::Px(2.0)),

        row_gap: Val::Vh(0.6),

        ..default()
    }
}

/// The live garage uses the same mesh/material assembly as the driving car.
/// No physics or session-owned entities are created in the showroom.
fn showroom(commands: &mut Commands, shell: &MenuShell, vfs: &Vfs, g: &mut Graphics) {
    let wanted = match shell.rows.get(shell.focus).map(|r| &r.action) {
        Some(Action::PickVehicle { id }) => Some((id.clone(), 0)),

        Some(Action::PickPaint { car, index }) => Some((car.clone(), *index)),

        _ => shell.vehicle.id.clone().map(|id| (id, shell.vehicle.paint)),
    };

    if wanted.is_some() && wanted == g.state.selection {
        return;
    }

    for entity in &g.scene {
        commands.entity(entity).despawn();
    }

    g.state.selection = wanted.clone();
    // A failed load must not leave the previous vehicle visible in the texture.
    g.state.target = g.images.add(Image::new_target_texture(
        960,
        640,
        TextureFormat::Rgba8Unorm,
        Some(TextureFormat::Rgba8UnormSrgb),
    ));

    g.state.title = "VEHICLE SHOWROOM".into();

    g.state.specs = "Select a vehicle to preview".into();

    let Some((id, paint)) = wanted else {
        return;
    };

    let def = match mm2_content::load_vehicle(vfs, &id, paint) {
        Ok(def) => def,

        Err(error) => {
            g.state.specs = format!("Preview unavailable: {error}");

            return;
        }
    };

    g.state.title = def.display_name.clone();

    g.state.specs = format!(
        "Paint: {}",
        def.paints
            .get(paint)
            .map(String::as_str)
            .unwrap_or("Default")
    );

    let (mut min, mut max) = def
        .model
        .body_aabb
        .map(|(a, b)| (Vec3::from(a), Vec3::from(b)))
        .unwrap_or((Vec3::splat(-2.0), Vec3::splat(2.0)));

    let trailer_offset = def
        .trailer
        .as_ref()
        .map(|trailer| Vec3::from(trailer.car_hitch) - Vec3::from(trailer.trailer_hitch));
    if let Some(trailer) = &def.trailer
        && let Some((a, b)) = trailer.model.body_aabb
        && let Some(offset) = trailer_offset
    {
        min = min.min(Vec3::from(a) + offset);
        max = max.max(Vec3::from(b) + offset);
    }

    let center = (min + max) * 0.5;

    let radius = (max - min).length().max(2.0) * 0.5;

    let root = commands
        .spawn((
            Showroom,
            Turntable,
            Transform::IDENTITY,
            Visibility::Visible,
        ))
        .id();

    let car = commands
        .spawn((Transform::from_translation(-center), Visibility::Inherited))
        .id();

    commands.entity(root).add_child(car);

    let mut missing = crate::car_visual::spawn_vehicle_model(
        commands,
        vfs,
        &def.model,
        paint,
        &mut g.meshes,
        &mut g.images,
        &mut g.materials,
        car,
        None,
    );
    if let Some(trailer) = &def.trailer
        && let Some(offset) = trailer_offset
    {
        let trailer_root = commands
            .spawn((
                Transform::from_translation(offset - center),
                Visibility::Inherited,
            ))
            .id();
        commands.entity(root).add_child(trailer_root);
        missing.extend(crate::car_visual::spawn_vehicle_model(
            commands,
            vfs,
            &trailer.model,
            paint,
            &mut g.meshes,
            &mut g.images,
            &mut g.materials,
            trailer_root,
            None,
        ));
    }

    if !missing.is_empty() {
        g.state.specs.push_str("\nSome textures are unavailable");
    }

    commands.spawn((
        Showroom,
        Camera3d::default(),
        AmbientLight {
            brightness: 200.0,
            ..default()
        },
        Camera {
            order: -1,

            clear_color: Color::srgb(0.035, 0.055, 0.16).into(),

            ..default()
        },
        RenderTarget::Image(g.state.target.clone().into()),
        RenderLayers::layer(PREVIEW_LAYER),
        Transform::from_translation(Vec3::new(1.5, 0.8, 2.0).normalize() * radius * 2.8)
            .looking_at(Vec3::ZERO, Vec3::Y),
    ));

    for (pos, color, power) in [
        (Vec3::new(4.0, 7.0, 5.0), Color::WHITE, 2_000_000.0),
        (
            Vec3::new(-5.0, 3.0, -3.0),
            Color::srgb(0.4, 0.55, 1.0),
            1_000_000.0,
        ),
    ] {
        commands.spawn((
            Showroom,
            PointLight {
                color,

                intensity: power,

                range: radius * 12.0,

                ..default()
            },
            Transform::from_translation(pos * radius / 3.0),
            RenderLayers::layer(PREVIEW_LAYER),
        ));
    }
}

/// Layers do not inherit in Bevy. Stamp every imported mesh descendant,
/// including wheel/fender grandchildren, before the first render pass.
pub fn menu_preview_motion(
    mut commands: Commands,

    time: Res<Time>,

    mut roots: Query<(Entity, &mut Transform), With<Turntable>>,

    children: Query<&Children>,

    capture: Option<Res<MenuPreviewCapture>>,
) {
    for (root, mut transform) in &mut roots {
        let delta = if capture.as_ref().is_some_and(|c| c.0) {
            1.0 / 60.0
        } else {
            time.delta_secs()
        };

        transform.rotate_y(delta * 0.22);

        commands
            .entity(root)
            .insert(RenderLayers::layer(PREVIEW_LAYER));

        for child in children.iter_descendants(root) {
            commands
                .entity(child)
                .insert(RenderLayers::layer(PREVIEW_LAYER));
        }
    }
}

pub fn menu_present(
    mut commands: Commands,

    mut shell: ResMut<MenuShell>,

    mut data: ResMut<MenuData>,

    vfs: Res<Mm2Vfs>,

    roots: Query<Entity, (With<MenuUi>, Without<ChildOf>)>,

    cameras: Query<Entity, With<MenuCamera>>,

    mut g: Graphics,
) {
    let loading = matches!(g.session.phase(), SessionPhase::Loading);

    if !shell.active && !loading {
        for root in &roots {
            commands.entity(root).despawn();
        }

        for camera in &cameras {
            commands.entity(camera).despawn();
        }

        for entity in &g.scene {
            commands.entity(entity).despawn();
        }

        g.state.selection = None;

        g.state.loading = false;

        return;
    }

    let camera = cameras
        .iter()
        .next()
        .unwrap_or_else(|| commands.spawn((MenuCamera, Camera2d)).id());

    if !shell.dirty && loading == g.state.loading {
        return;
    }

    g.state.loading = loading;

    shell.dirty = false;

    if !loading {
        rebuild(&mut shell, &mut data, &vfs.0);
    }

    if g.state.art == Handle::default() {
        if let Ok(image) = Image::from_buffer(
            include_bytes!("../assets/menu/city-night.png"),
            ImageType::Extension("png"),
            CompressedImageFormats::all(),
            true,
            ImageSampler::Default,
            RenderAssetUsages::default(),
        ) {
            g.state.art = g.images.add(image);
        }

        if let Ok(image) = Image::from_buffer(
            include_bytes!("../assets/menu/london-night.png"),
            ImageType::Extension("png"),
            CompressedImageFormats::all(),
            true,
            ImageSampler::Default,
            RenderAssetUsages::default(),
        ) {
            g.state.london = g.images.add(image);
        }
        g.state.target = g.images.add(Image::new_target_texture(
            960,
            640,
            TextureFormat::Rgba8Unorm,
            Some(TextureFormat::Rgba8UnormSrgb),
        ));
    }

    let garage = matches!(shell.screen, Screen::Garage | Screen::Paints { .. });

    if garage && !loading {
        showroom(&mut commands, &shell, &vfs.0, &mut g);
    } else {
        for entity in &g.scene {
            commands.entity(entity).despawn();
        }

        g.state.selection = None;
    }

    for root in &roots {
        commands.entity(root).despawn();
    }

    let title = if loading {
        "GET READY".into()
    } else {
        match shell.screen {
            Screen::Root => "SELECT YOUR DRIVE".into(),
            Screen::Garage => "SELECT VEHICLE".into(),
            Screen::Paints { .. } => "SELECT PAINT".into(),
            Screen::Profiles => "SELECT DRIVER".into(),
            Screen::CruiseCity | Screen::EventCity => "SELECT CITY".into(),
            _ => screen_title(&shell.screen).to_uppercase(),
        }
    };

    commands
        .spawn((
            MenuUi,
            UiTargetCamera(camera),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Vw(3.0)),
                row_gap: Val::Vh(2.0),
                ..default()
            },
            BackgroundColor(INK),
        ))
        .with_children(|parent| {
            backdrop(parent, g.state.art.clone());
            parent
                .spawn((
                    MenuUi,
                    Node {
                        height: Val::Vh(15.0),
                        flex_shrink: 0.0,
                        flex_direction: FlexDirection::Column,
                        justify_content: JustifyContent::Center,
                        border: UiRect::bottom(Val::Px(3.0)),
                        ..default()
                    },
                    BorderColor::all(GOLD),
                ))
                .with_children(|p| {
                    label(p, "RUST / MM2", 18.0, Color::WHITE);
                    label(p, title, 40.0, GOLD);
                });
            if loading {
                draw_loading(parent);
                return;
            }
            parent
                .spawn((
                    MenuUi,
                    Node {
                        flex_grow: 1.0,
                        min_height: Val::Px(0.0),
                        column_gap: Val::Vw(2.0),
                        ..default()
                    },
                ))
                .with_children(|body| {
                    draw_rows(body, &shell, garage);
                    draw_detail(body, &shell, &data, &g, garage);
                });
            if let Some(status) = &shell.status {
                label(parent, status, 16.0, GOLD);
            }
            draw_navigation(parent, &shell.screen);
        });
}

fn backdrop(parent: &mut ChildSpawnerCommands, art: Handle<Image>) {
    let full = Node {
        position_type: PositionType::Absolute,
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        left: Val::Px(0.0),
        top: Val::Px(0.0),
        ..default()
    };
    parent.spawn((MenuUi, ImageNode::new(art), full.clone(), ZIndex(-2)));
    parent.spawn((
        MenuUi,
        full,
        BackgroundColor(Color::srgba(0.015, 0.02, 0.08, 0.35)),
        ZIndex(-1),
    ));
}

fn draw_loading(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn((
            MenuUi,
            Node {
                flex_grow: 1.0,
                justify_content: JustifyContent::End,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Vh(2.0),
                ..default()
            },
        ))
        .with_children(|p| {
            label(p, "LOADING THE STREETS", 30.0, Color::WHITE);
            label(p, "Preparing your vehicle and city...", 18.0, GOLD);
            p.spawn((
                MenuUi,
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(8.0),
                    ..default()
                },
                BackgroundColor(BLUE),
            ));
        });
}

fn draw_rows(body: &mut ChildSpawnerCommands, shell: &MenuShell, garage: bool) {
    body.spawn((
        MenuUi,
        Node {
            width: Val::Percent(44.0),
            ..panel()
        },
        BackgroundColor(INK),
        BorderColor::all(BLUE),
    ))
    .with_children(|p| {
        label(
            p,
            if garage {
                "CHOOSE YOUR MACHINE"
            } else {
                "MAKE YOUR SELECTION"
            },
            15.0,
            GOLD,
        );
        if let Screen::NewProfile { name } = &shell.screen {
            label(p, format!("  Name: {name}_"), 24.0, Color::WHITE);
        }
        let count = shell.rows.len();
        let start = shell.focus.saturating_sub(7).min(count.saturating_sub(9));
        for (index, row) in shell.rows.iter().enumerate().skip(start).take(9) {
            let focused = index == shell.focus;
            draw_row(p, row, index, focused && !shell.side, focused && shell.side);
        }
        if count > 9 {
            label(
                p,
                format!("{} / {}   -   Up / Down to browse", shell.focus + 1, count),
                14.0,
                GOLD,
            );
        }
    });
}

fn row_color(row: &Row, focus: bool) -> Color {
    if row.enabled.is_err() {
        Color::srgb(0.55, 0.59, 0.7)
    } else if focus {
        GOLD
    } else {
        Color::WHITE
    }
}

/// One list row. `focus` highlights the row itself; `side_focus` the
/// side entry drawn at its right edge, which the row then frames
/// rather than fills so the eye lands on the entry.
fn draw_row(
    parent: &mut ChildSpawnerCommands,
    row: &Row,
    index: usize,
    focus: bool,
    side_focus: bool,
) {
    let color = row_color(row, focus);
    parent
        .spawn((
            MenuUi,
            MenuRow { index },
            Node {
                width: Val::Percent(100.0),
                min_height: Val::Vh(4.0),
                flex_shrink: 0.0,
                padding: UiRect::axes(Val::Px(12.0), Val::Px(4.0)),
                align_items: AlignItems::Center,
                border: UiRect::left(Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(if focus {
                Color::srgba(0.2, 0.3, 0.68, 0.95)
            } else {
                Color::srgba(0.04, 0.065, 0.2, 0.8)
            }),
            BorderColor::all(if focus || side_focus { GOLD } else { BLUE }),
        ))
        .with_children(|p| {
            p.spawn((
                MenuUi,
                Text::new(format!(
                    "{} {}",
                    if focus { ">" } else { " " },
                    display_label(&row.text)
                )),
                TextFont {
                    font_size: bevy::text::FontSize::Px(18.0),
                    ..default()
                },
                TextColor(color),
                Node {
                    flex_grow: 1.0,
                    ..default()
                },
            ));
            if let Some(won) = row.won {
                badge(p, won.label(), GREEN);
            }
            if let Some(side) = &row.side {
                draw_side(p, side, index, side_focus);
            }
        });
}

/// A row's side entry: a framed cell at the right edge, its own hover
/// and click target.
fn draw_side(parent: &mut ChildSpawnerCommands, side: &Row, index: usize, focus: bool) {
    parent
        .spawn((
            MenuUi,
            MenuSide { index },
            Node {
                flex_shrink: 0.0,
                margin: UiRect::left(Val::Px(10.0)),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(2.0)),
                border: UiRect::all(Val::Px(2.0)),
                ..default()
            },
            BackgroundColor(if focus {
                Color::srgba(0.2, 0.3, 0.68, 0.95)
            } else {
                Color::NONE
            }),
            BorderColor::all(if focus { GOLD } else { BLUE }),
        ))
        .with_children(|p| {
            p.spawn((
                MenuUi,
                Text::new(format!("{}{}", if focus { "> " } else { "" }, side.text)),
                TextFont {
                    font_size: bevy::text::FontSize::Px(14.0),
                    ..default()
                },
                TextColor(row_color(side, focus)),
            ));
        });
}

/// A small filled tag at a row's right edge.
fn badge(parent: &mut ChildSpawnerCommands, text: &str, fill: Color) {
    parent
        .spawn((
            MenuUi,
            Node {
                flex_shrink: 0.0,
                margin: UiRect::left(Val::Px(8.0)),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(2.0)),
                ..default()
            },
            BackgroundColor(fill),
        ))
        .with_children(|p| {
            p.spawn((
                MenuUi,
                Text::new(text),
                TextFont {
                    font_size: bevy::text::FontSize::Px(13.0),
                    ..default()
                },
                TextColor(INK),
            ));
        });
}

fn draw_detail(
    body: &mut ChildSpawnerCommands,
    shell: &MenuShell,
    data: &MenuData,
    g: &Graphics,
    garage: bool,
) {
    body.spawn((
        MenuUi,
        Node {
            width: Val::Percent(56.0),
            ..panel()
        },
        BackgroundColor(Color::srgba(
            0.025,
            0.035,
            0.12,
            if garage { 0.95 } else { 0.72 },
        )),
        BorderColor::all(BLUE),
    ))
    .with_children(|p| {
        if garage {
            label(p, &g.state.title, 28.0, GOLD);
            p.spawn((
                MenuUi,
                ImageNode::new(g.state.target.clone()),
                Node {
                    width: Val::Percent(100.0),
                    flex_grow: 1.0,
                    min_height: Val::Px(0.0),
                    ..default()
                },
            ));
            if let Some(stats) = g
                .state
                .selection
                .as_ref()
                .and_then(|(id, _)| data.catalog.as_ref()?.find(id).ok())
                .map(|entry| entry.stats)
            {
                stat_bars(p, &stats, &roster_max(data));
            }
            label(p, &g.state.specs, 18.0, Color::WHITE);
            // A locked car or paint names what earns it here, not only
            // on the status line after a refused Enter.
            if let Some(Err(reason)) = shell.focused_row().map(|r| &r.enabled) {
                label(p, upper_first(reason), 18.0, GOLD);
                if reason.contains(" won)") {
                    label(p, WIN_RULE, 14.0, Color::WHITE);
                }
            }
            label(p, "LIVE SHOWROOM   /   360 DEGREE VIEW", 13.0, GOLD);
            return;
        }
        label(p, "DRIVER / SESSION", 15.0, GOLD);
        label(
            p,
            data.bound
                .as_ref()
                .map(|d| d.name.as_str())
                .unwrap_or("GUEST DRIVER"),
            30.0,
            Color::WHITE,
        );
        label(
            p,
            format!("RANK   {:?}", shell.difficulty).to_uppercase(),
            17.0,
            GOLD,
        );
        let vehicle = shell
            .vehicle
            .id
            .as_deref()
            .and_then(|id| {
                data.catalog
                    .as_ref()?
                    .find(id)
                    .ok()
                    .map(|e| e.display_name.as_str())
            })
            .unwrap_or("Development car");
        label(p, format!("VEHICLE   {vehicle}"), 17.0, Color::WHITE);
        if matches!(
            shell.screen,
            Screen::CruiseCity
                | Screen::EventCity
                | Screen::EventTable { .. }
                | Screen::EventList { .. }
                | Screen::Customize { .. }
        ) {
            let london = shell
                .rows
                .get(shell.focus)
                .is_some_and(|row| row.text.to_lowercase().contains("london"))
                || screen_title(&shell.screen)
                    .to_lowercase()
                    .contains("london");
            p.spawn((
                MenuUi,
                ImageNode::new(if london {
                    g.state.london.clone()
                } else {
                    g.state.art.clone()
                }),
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Vh(26.0),
                    margin: UiRect::top(Val::Vh(1.0)),
                    ..default()
                },
            ));
        }

        p.spawn((
            MenuUi,
            Node {
                flex_grow: 1.0,
                ..default()
            },
        ));
        if let Some(row) = shell.focused_row() {
            label(p, display_label(&row.text), 26.0, GOLD);
            if let Some(won) = row.won {
                let on = match (won.amateur, won.professional) {
                    (true, true) => "Amateur and Professional",
                    (false, true) => "Professional",
                    _ => "Amateur",
                };
                label(p, format!("WON on {on}"), 18.0, GREEN);
            }
            label(
                p,
                row.enabled
                    .as_ref()
                    .err()
                    .cloned()
                    .unwrap_or_else(|| screen_hint(&shell.screen).into()),
                18.0,
                Color::WHITE,
            );
        }
    });
}

/// The largest of each figure across the select roster — the scale
/// the original's bars compare cars on.
fn roster_max(data: &MenuData) -> mm2_content::DisplayStats {
    let mut max = mm2_content::DisplayStats::default();
    let entries = data.catalog.iter().flat_map(|c| &c.entries);
    for stats in entries.filter(|e| e.canonical_info).map(|e| e.stats) {
        let bump = |m: &mut Option<f32>, v: Option<f32>| {
            if let Some(v) = v {
                *m = Some(m.map_or(v, |m| m.max(v)));
            }
        };
        bump(&mut max.horsepower, stats.horsepower);
        bump(&mut max.top_speed, stats.top_speed);
        bump(&mut max.durability, stats.durability);
        bump(&mut max.mass, stats.mass);
    }
    max
}

/// The original vehicle-select comparison: one bar per authored
/// figure, scaled to the roster's largest. Horsepower and top speed
/// read as real units and print their number; Durability and Mass are
/// unitless scales (vppanozgt's Mass is 850), so they show as bars.
fn stat_bars(
    parent: &mut ChildSpawnerCommands,
    stats: &mm2_content::DisplayStats,
    max: &mm2_content::DisplayStats,
) {
    let rows = [
        ("HORSEPOWER", stats.horsepower, max.horsepower, Some("hp")),
        ("TOP SPEED", stats.top_speed, max.top_speed, Some("mph")),
        ("DURABILITY", stats.durability, max.durability, None),
        ("MASS", stats.mass, max.mass, None),
    ];
    for (name, value, max, unit) in rows {
        let fill = match (value, max) {
            (Some(v), Some(m)) if m > 0.0 => (v / m).clamp(0.0, 1.0),
            _ => 0.0,
        };
        let text = match (value, unit) {
            (None, _) => format!("{name}  n/a"),
            (Some(v), Some(unit)) => format!("{name}  {v:.0} {unit}"),
            (Some(_), None) => name.to_string(),
        };
        parent
            .spawn((
                MenuUi,
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(22.0),
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(12.0),
                    flex_shrink: 0.0,
                    ..default()
                },
            ))
            .with_children(|p| {
                p.spawn((
                    MenuUi,
                    Text::new(text),
                    // A label measured as wrapping sits above its bar.
                    TextLayout::no_wrap(),
                    TextFont {
                        font_size: bevy::text::FontSize::Px(14.0),
                        ..default()
                    },
                    TextColor(GOLD),
                    Node {
                        width: Val::Px(190.0),
                        flex_shrink: 0.0,
                        ..default()
                    },
                ));
                p.spawn((
                    MenuUi,
                    Node {
                        flex_grow: 1.0,
                        height: Val::Px(10.0),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.25, 0.43, 0.95, 0.25)),
                ))
                .with_children(|track| {
                    track.spawn((
                        MenuUi,
                        Node {
                            width: Val::Percent(fill * 100.0),
                            height: Val::Percent(100.0),
                            ..default()
                        },
                        BackgroundColor(GOLD),
                    ));
                });
            });
    }
}

/// What counts as winning a race toward a reward (the persisted
/// `beaten_*` criterion).
const WIN_RULE: &str =
    "A race counts as won with a top 3 finish on Amateur or 1st place on Professional.";

fn upper_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn display_label(text: &str) -> &str {
    match text {
        "sf" => "San Francisco",
        "london" => "London",
        "Events" => "Races",
        _ => text,
    }
}

fn screen_hint(screen: &Screen) -> &'static str {
    match screen {
        Screen::CruiseCity | Screen::EventCity => {
            "Explore the city. Choose your destination to continue."
        }
        Screen::Profiles => "Select a driver to load their progress and race records.",
        Screen::Customize { .. } => {
            "Use Left / Right to tune your session. Select the start button when ready."
        }
        _ => "Select to continue your drive.",
    }
}

fn draw_navigation(parent: &mut ChildSpawnerCommands, screen: &Screen) {
    parent
        .spawn((
            MenuUi,
            Node {
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                flex_shrink: 0.0,
                ..default()
            },
        ))
        .with_children(|p| {
            for (command, text) in [
                (MenuCommand::Back, "< BACK"),
                (MenuCommand::Up, "PREVIOUS"),
                (MenuCommand::Down, "NEXT"),
                (MenuCommand::Activate, "SELECT >"),
            ] {
                p.spawn((
                    MenuUi,
                    MenuButton(command),
                    Node {
                        padding: UiRect::axes(Val::Vw(1.5), Val::Vh(1.0)),
                        border: UiRect::all(Val::Px(2.0)),
                        ..default()
                    },
                    BackgroundColor(INK),
                    BorderColor::all(BLUE),
                ))
                .with_children(|p| label(p, text, 17.0, GOLD));
            }
        });
    let hint = if matches!(screen, Screen::NewProfile { .. }) {
        "Type a name | Enter create | Esc cancel"
    } else {
        "UP / DOWN  Browse     LEFT / RIGHT  Adjust / Options     ENTER  Select     ESC / Right click  Back"
    };
    label(parent, hint, 14.0, Color::WHITE);
}
