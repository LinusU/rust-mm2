//! Presentation smoothing of the own car's reconciliation shift
//! (NETPLAY-07, `docs/netcode-redesign.md`).
//!
//! `netpredict::reconcile_own` restores the recorded world, replays the
//! unacknowledged commands and leaves the car where the corrected
//! simulation puts it. That jump is real simulation state and is never
//! softened: `Position`/`Rotation` (and so every collider and the next
//! prediction step) are untouched by this module. What the *player sees*
//! is another matter. Following Gaffer's "state synchronization" error
//! offset, [`CorrectionOffset`] remembers the difference between where the
//! car was drawn just before a correction and where the corrected body is
//! now, and decays it with elapsed wall time. The offset is applied to the
//! rendered `GlobalTransform` subtree of the car and to the chase-camera
//! target only, after transform propagation, so Avian's transform sync can
//! never read it back.
//!
//! Nothing here delays input or the body: the offset is a pure function of
//! past corrections. The constants are designed values (the original game
//! has no network correction), chosen to bound both the duration and the
//! distance of any visible slide.

use bevy::prelude::*;
use bevy::transform::TransformSystems;

use crate::netplay_timeline::NetplayTimeline;

/// Time constant of the exponential decay (seconds). 99% gone after
/// `4.6 * TAU` = 0.55 s. Designed.
pub const DECAY_TAU: f32 = 0.12;
/// Largest visible displacement kept (metres). A larger correction keeps
/// only this much of its slide, so the camera never trails far behind a
/// body that is somewhere else. Designed.
pub const MAX_DISTANCE: f32 = 1.5;
/// Largest visible rotation kept (radians, ~20 degrees). Designed.
pub const MAX_ANGLE: f32 = 0.35;
/// A correction larger than this is a teleport (authority reset, recovery
/// snap), not a slide: the history is dropped and the view jumps. Designed.
pub const TELEPORT_DISTANCE: f32 = 6.0;
/// Rest thresholds: below both, the offset is cleared exactly.
pub const REST_DISTANCE: f32 = 0.001;
pub const REST_ANGLE: f32 = 0.002;

/// Where the car is drawn relative to where its body is: a world-space
/// translation plus a rotation about the body origin. `rendered = body +
/// translation`, `rendered_rot = rotation * body_rot`.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct CorrectionOffset {
    pub translation: Vec3,
    pub rotation: Quat,
}

impl Default for CorrectionOffset {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl CorrectionOffset {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
    };

    pub fn is_rest(&self) -> bool {
        self.translation == Vec3::ZERO && self.rotation == Quat::IDENTITY
    }

    /// Length of the visible translation.
    pub fn distance(&self) -> f32 {
        self.translation.length()
    }

    /// Shortest-arc angle of the visible rotation, `0..=PI`.
    pub fn angle(&self) -> f32 {
        // atan2 stays exact for the tiny angles `acos(w)` loses to rounding.
        2.0 * self.rotation.xyz().length().atan2(self.rotation.w.abs())
    }

    /// The correction moved the body from `before` to `after` at the same
    /// simulation instant. Keep the *drawn* pose where it was: the new
    /// offset is the old one plus the (negated) body shift. A non-finite
    /// input clears the offset rather than poisoning the view.
    pub fn absorb(&mut self, before: (Vec3, Quat), after: (Vec3, Quat)) {
        let (p0, r0) = before;
        let (p1, r1) = after;
        let shift = p0 - p1;
        if !(shift.is_finite() && r0.is_finite() && r1.is_finite() && self.rotation.is_finite()) {
            *self = Self::IDENTITY;
            return;
        }
        if shift.length() > TELEPORT_DISTANCE {
            *self = Self::IDENTITY;
            return;
        }
        let visible_rot = self.rotation * r0;
        self.translation += shift;
        self.rotation = (visible_rot * r1.inverse()).normalize();
        self.bound();
    }

    /// Clamp to the designed maxima, then clear at rest.
    fn bound(&mut self) {
        if self.rotation.w < 0.0 {
            self.rotation = -self.rotation;
        }
        let len = self.translation.length();
        if len > MAX_DISTANCE {
            self.translation *= MAX_DISTANCE / len;
        }
        let angle = self.angle();
        if angle > MAX_ANGLE {
            self.rotation = Quat::IDENTITY.slerp(self.rotation, MAX_ANGLE / angle);
        }
        if self.translation.length() < REST_DISTANCE && self.angle() < REST_ANGLE {
            *self = Self::IDENTITY;
        }
    }

    /// Decay by `dt` seconds of wall time. Exponential, so two half-steps
    /// equal one whole step at any frame rate.
    pub fn decay(&mut self, dt: f32) {
        if self.is_rest() || !dt.is_finite() || dt <= 0.0 {
            return;
        }
        let keep = (-dt / DECAY_TAU).exp();
        self.translation *= keep;
        self.rotation = Quat::IDENTITY.slerp(self.rotation, keep);
        self.bound();
    }

    /// The rendered pose of a body at `(position, rotation)`.
    pub fn apply(&self, position: Vec3, rotation: Quat) -> (Vec3, Quat) {
        (position + self.translation, self.rotation * rotation)
    }

    /// Carry a world-space transform that belongs to the body whose origin
    /// is `body` (the car's root or any descendant of it).
    pub fn carry(&self, body: Vec3, world: &GlobalTransform) -> GlobalTransform {
        let (scale, rot, pos) = world.to_scale_rotation_translation();
        GlobalTransform::from(Transform {
            translation: body + self.translation + self.rotation * (pos - body),
            rotation: self.rotation * rot,
            scale,
        })
    }

    /// Inverse of [`Self::carry`].
    fn uncarry(&self, body: Vec3, world: &GlobalTransform) -> GlobalTransform {
        let (scale, rot, pos) = world.to_scale_rotation_translation();
        let inv = self.rotation.inverse();
        GlobalTransform::from(Transform {
            translation: body + inv * (pos - body - self.translation),
            rotation: inv * rot,
            scale,
        })
    }
}

/// What the offset was applied to last frame (`None` when nothing was), so
/// that frame's edit can be undone exactly even when propagation skipped
/// the unmoved subtree. Always inserted next to [`CorrectionOffset`].
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct AppliedOffset(Option<(CorrectionOffset, Vec3)>);

/// Decay the offset with the frame's wall-clock delta.
pub fn decay_offsets(time: Res<Time<Real>>, mut offsets: Query<&mut CorrectionOffset>) {
    let dt = time.delta_secs();
    for mut offset in &mut offsets {
        if !offset.is_rest() {
            offset.decay(dt);
        }
    }
}

/// Before propagation: put the subtree's `GlobalTransform`s back to the
/// true values.
pub fn undo_offsets(
    mut roots: Query<(Entity, &mut AppliedOffset)>,
    mut globals: Query<&mut GlobalTransform>,
    children: Query<&Children>,
) {
    for (root, mut applied) in &mut roots {
        let Some((offset, body)) = applied.0.take() else {
            continue;
        };
        for e in std::iter::once(root).chain(children.iter_descendants(root)) {
            if let Ok(mut g) = globals.get_mut(e) {
                *g = offset.uncarry(body, &g);
            }
        }
    }
}

/// After propagation: draw the car (and everything attached to it,
/// including a cockpit camera) at the offset pose.
pub fn apply_offsets(
    mut roots: Query<(Entity, &CorrectionOffset, &mut AppliedOffset)>,
    mut globals: Query<&mut GlobalTransform>,
    children: Query<&Children>,
) {
    for (root, offset, mut applied) in &mut roots {
        if offset.is_rest() {
            continue;
        }
        let Ok(g) = globals.get(root) else {
            continue;
        };
        let body = g.translation();
        for e in std::iter::once(root).chain(children.iter_descendants(root)) {
            if let Ok(mut g) = globals.get_mut(e) {
                *g = offset.carry(body, &g);
            }
        }
        applied.0 = Some((*offset, body));
    }
}

/// The own car, ready to receive an offset.
pub fn ensure_offset(world: &mut World, own: Entity) {
    if world.get::<CorrectionOffset>(own).is_none() {
        world
            .entity_mut(own)
            .insert((CorrectionOffset::IDENTITY, AppliedOffset::default()));
    }
}

/// A reconciliation moved the own body. No-op with the selector off.
pub fn absorb_correction(
    world: &mut World,
    own: Entity,
    before: (Vec3, Quat),
    after: (Vec3, Quat),
) {
    ensure_offset(world, own);
    if let Some(mut offset) = world.get_mut::<CorrectionOffset>(own) {
        offset.absorb(before, after);
    }
}

/// An authority reset or a recovery: the old life must not be animated
/// from.
pub fn clear_correction(world: &mut World, own: Entity) {
    if let Some(mut offset) = world.get_mut::<CorrectionOffset>(own) {
        *offset = CorrectionOffset::IDENTITY;
    }
}

/// Camera target for the own car: the body pose plus the visual offset.
pub fn target_pose(
    position: Vec3,
    rotation: Quat,
    offset: Option<&CorrectionOffset>,
) -> (Vec3, Quat) {
    offset.map_or((position, rotation), |o| o.apply(position, rotation))
}

/// Run condition mirroring the netpredict gate: only a selected timeline
/// produces offsets, so this is inert otherwise.
fn selected(timeline: Option<Res<NetplayTimeline>>) -> bool {
    timeline.is_some_and(|t| t.enabled)
}

pub fn add_systems(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (
            undo_offsets.before(TransformSystems::Propagate),
            apply_offsets.after(TransformSystems::Propagate),
        )
            .run_if(selected),
    )
    .add_systems(First, decay_offsets.run_if(selected));
}

#[cfg(test)]
mod tests {
    use std::f32::consts::PI;

    use super::*;

    fn yaw(a: f32) -> Quat {
        Quat::from_rotation_y(a)
    }

    #[test]
    fn a_correction_keeps_the_drawn_pose_then_converges() {
        let mut o = CorrectionOffset::default();
        let before = (Vec3::new(10.0, 0.0, 0.0), yaw(0.0));
        let after = (Vec3::new(10.4, 0.0, 0.0), yaw(0.0));
        o.absorb(before, after);
        let (drawn, _) = o.apply(after.0, after.1);
        assert!((drawn - before.0).length() < 1e-5, "{drawn:?}");
        for _ in 0..120 {
            o.decay(1.0 / 60.0);
        }
        assert!(o.is_rest());
    }

    #[test]
    fn convergence_time_is_the_same_at_30_60_and_144_fps() {
        let settle = |fps: f32| {
            let mut o = CorrectionOffset::default();
            o.absorb(
                (Vec3::ZERO, Quat::IDENTITY),
                (Vec3::new(0.5, 0.0, 0.0), yaw(0.1)),
            );
            let mut t = 0.0;
            while !o.is_rest() {
                o.decay(1.0 / fps);
                t += 1.0 / fps;
                assert!(t < 2.0, "never settled at {fps} fps");
            }
            t
        };
        let (a, b, c) = (settle(30.0), settle(60.0), settle(144.0));
        assert!((a - c).abs() < 0.05 && (b - c).abs() < 0.05, "{a} {b} {c}");
        assert!(c < 1.0, "{c}");
    }

    #[test]
    fn repeated_tiny_corrections_do_not_accumulate() {
        let mut o = CorrectionOffset::default();
        let mut body = Vec3::ZERO;
        let mut worst = 0.0f32;
        for i in 0..600 {
            // A 2 cm correction every 4 frames, alternating sign.
            if i % 4 == 0 {
                let next = body + Vec3::new(if i % 8 == 0 { 0.02 } else { -0.02 }, 0.0, 0.0);
                o.absorb((body, Quat::IDENTITY), (next, Quat::IDENTITY));
                body = next;
            }
            o.decay(1.0 / 60.0);
            worst = worst.max(o.distance());
        }
        assert!(worst < 0.04, "{worst}");
    }

    #[test]
    fn a_large_correction_is_bounded_and_a_teleport_is_not_animated() {
        let mut o = CorrectionOffset::default();
        o.absorb(
            (Vec3::ZERO, Quat::IDENTITY),
            (Vec3::new(4.0, 0.0, 0.0), yaw(1.5)),
        );
        assert!(o.distance() <= MAX_DISTANCE + 1e-5);
        assert!(o.angle() <= MAX_ANGLE + 1e-4);
        o.absorb(
            (Vec3::ZERO, Quat::IDENTITY),
            (Vec3::new(50.0, 0.0, 0.0), Quat::IDENTITY),
        );
        assert!(o.is_rest());
    }

    #[test]
    fn yaw_crossing_pi_takes_the_short_way() {
        let mut o = CorrectionOffset::default();
        // Body was at yaw pi - 0.05, the correction puts it at -pi + 0.05:
        // a 0.1 rad turn, not a 6.18 rad one.
        o.absorb((Vec3::ZERO, yaw(PI - 0.05)), (Vec3::ZERO, yaw(-PI + 0.05)));
        assert!((o.angle() - 0.1).abs() < 1e-3, "{}", o.angle());
        let (_, drawn) = o.apply(Vec3::ZERO, yaw(-PI + 0.05));
        assert!(drawn.angle_between(yaw(PI - 0.05)) < 1e-3);
        for _ in 0..120 {
            o.decay(1.0 / 60.0);
        }
        assert!(o.is_rest());
    }

    #[test]
    fn alternating_yaw_corrections_settle() {
        let mut o = CorrectionOffset::default();
        let mut body = yaw(0.0);
        for i in 0..40 {
            let next = yaw(if i % 2 == 0 { 0.05 } else { -0.05 });
            o.absorb((Vec3::ZERO, body), (Vec3::ZERO, next));
            body = next;
            o.decay(1.0 / 60.0);
            assert!(o.angle() < MAX_ANGLE);
        }
        for _ in 0..120 {
            o.decay(1.0 / 60.0);
        }
        assert!(o.is_rest());
    }

    #[test]
    fn non_finite_input_clears_the_offset() {
        let mut o = CorrectionOffset::default();
        o.absorb(
            (Vec3::ZERO, Quat::IDENTITY),
            (Vec3::X * 0.3, Quat::IDENTITY),
        );
        o.absorb((Vec3::NAN, Quat::IDENTITY), (Vec3::ZERO, Quat::IDENTITY));
        assert!(o.is_rest());
    }

    #[test]
    fn carry_and_uncarry_are_inverse_and_rotate_about_the_body() {
        let mut o = CorrectionOffset::default();
        o.absorb((Vec3::ZERO, yaw(0.2)), (Vec3::new(0.3, 0.0, 0.1), yaw(0.0)));
        let body = Vec3::new(5.0, 1.0, -2.0);
        let child =
            GlobalTransform::from(Transform::from_translation(body + Vec3::new(1.0, 0.5, 0.0)));
        let drawn = o.carry(body, &child);
        let back = o.uncarry(body, &drawn);
        assert!((back.translation() - child.translation()).length() < 1e-4);
        // The root itself lands on body + translation.
        let root = o.carry(
            body,
            &GlobalTransform::from(Transform::from_translation(body)),
        );
        assert!((root.translation() - (body + o.translation)).length() < 1e-5);
    }

    /// The offset lives on a component the physics never reads: with it
    /// present or absent the body components are bit-identical, and the
    /// subtree's `GlobalTransform` is restored on the next frame.
    #[test]
    fn the_systems_edit_only_global_transforms_and_never_accumulate() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(TransformPlugin)
            .insert_resource(NetplayTimeline { enabled: true });
        app.add_systems(
            PostUpdate,
            (
                undo_offsets.before(TransformSystems::Propagate),
                apply_offsets.after(TransformSystems::Propagate),
            ),
        );
        let root = app
            .world_mut()
            .spawn((
                Transform::from_xyz(10.0, 0.0, 0.0),
                CorrectionOffset::default(),
                AppliedOffset::default(),
            ))
            .id();
        let child = app
            .world_mut()
            .spawn((Transform::from_xyz(0.0, 1.0, 0.0), ChildOf(root)))
            .id();
        app.update();
        let mut o = CorrectionOffset::default();
        o.absorb(
            (Vec3::new(10.0, 0.0, 0.0), Quat::IDENTITY),
            (Vec3::new(10.5, 0.0, 0.0), Quat::IDENTITY),
        );
        *app.world_mut().get_mut::<CorrectionOffset>(root).unwrap() = o;
        // The static subtree is not re-propagated; repeated frames must not
        // push the drawn pose further each time.
        for _ in 0..5 {
            app.update();
            let g = app
                .world()
                .get::<GlobalTransform>(child)
                .unwrap()
                .translation();
            assert!((g - Vec3::new(9.5, 1.0, 0.0)).length() < 1e-4, "{g:?}");
        }
        assert_eq!(
            app.world().get::<Transform>(root).unwrap().translation,
            Vec3::new(10.0, 0.0, 0.0)
        );
        *app.world_mut().get_mut::<CorrectionOffset>(root).unwrap() = CorrectionOffset::IDENTITY;
        app.update();
        let g = app
            .world()
            .get::<GlobalTransform>(child)
            .unwrap()
            .translation();
        assert!((g - Vec3::new(10.0, 1.0, 0.0)).length() < 1e-4, "{g:?}");
    }
}
