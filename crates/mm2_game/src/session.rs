//! Session lifecycle, entity ownership and the fixed-step session clock
//! (F01-A).
//!
//! Phase flow, per the F01 spec
//! (`menu → loading → ready/countdown → playing → paused/results →
//! unloading/menu`):
//!
//! ```text
//!   Menu ─begin─► Loading ─► Ready ─► Countdown ─► Playing ─► Results ─┐
//!    ▲             │           │                       │    │          │
//!    │             ▼           ▼                       ▼    ▼          │
//!    │           Failed      Unloading ◄──────────── Paused ◄──────────┘
//!    │             │           ▲
//!    └─────────────┴───────────┘   (Unloading → Menu; restart = begin)
//! ```
//!
//! Every start goes through `Menu` — restart is `Unloading → Menu →
//! begin`, so session-owned entities are always cleaned between runs.
//! Illegal transitions are rejected and leave the phase unchanged.
//!
//! Ownership: everything a session spawns carries [`SessionEntity`] with
//! the session's generation; [`despawn_session_entities`] removes those
//! roots (children cascade) during `Unloading`. Persistent UI/profile
//! state must never carry the marker. Session-scoped resources
//! (`SpawnPoint`-style) are re-inserted by the next `begin`'s setup —
//! entity cleanup is what the marker exists for.
//!
//! Clock: [`advance_session_tick`] counts fixed simulation steps while
//! `Playing` — the gameplay clock, independent of render FPS.

use bevy::prelude::*;

use crate::config::{ConfigError, SessionConfig};

/// One session's lifecycle phase.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionPhase {
    /// No active session; only persistent UI/profile state exists.
    Menu,
    /// Building the world per the session config. Pause is not
    /// reachable here — quitting during a load is `Unloading`.
    Loading,
    /// World built; the player may exist but control is not released.
    Ready,
    /// Authored-event countdown before control is released. Nothing
    /// enters this phase yet — races land with F12+.
    Countdown,
    /// Live gameplay.
    Playing,
    /// Single-player pause only (MP-6: multiplayer never pauses;
    /// `Playing → Paused` is rejected unless the session authority
    /// `allows_pause`).
    Paused,
    /// Session ended; results shown.
    Results,
    /// Tearing down session-owned entities before `Menu`.
    Unloading,
    /// Load or start-up failure — carries the reason. Reachable only
    /// from `Loading`/`Ready`, so a failed load can never leave a
    /// half-built world pretending to play (AC02).
    Failed(String),
}

impl SessionPhase {
    /// Stable lowercase name for logs and smoke records.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Menu => "menu",
            Self::Loading => "loading",
            Self::Ready => "ready",
            Self::Countdown => "countdown",
            Self::Playing => "playing",
            Self::Paused => "paused",
            Self::Results => "results",
            Self::Unloading => "unloading",
            Self::Failed(_) => "failed",
        }
    }
}

/// A rejected session operation.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionError {
    /// The requested phase transition is not in the lifecycle.
    IllegalTransition {
        /// Current phase name.
        from: &'static str,
        /// Requested phase name.
        to: &'static str,
    },
    /// `begin` was handed an invalid configuration.
    Config(ConfigError),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IllegalTransition { from, to } => {
                write!(f, "illegal session transition {from} → {to}")
            }
            Self::Config(e) => write!(f, "invalid session config: {e}"),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::IllegalTransition { .. } => None,
            Self::Config(e) => Some(e),
        }
    }
}

/// The session resource: phase + validated config + ownership
/// generation + the fixed-step clock.
#[derive(Resource, Debug)]
pub struct Session {
    phase: SessionPhase,
    config: Option<SessionConfig>,
    generation: u64,
    tick: u64,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// A session at the menu phase — no world, no config.
    pub fn new() -> Self {
        Self {
            phase: SessionPhase::Menu,
            config: None,
            generation: 0,
            tick: 0,
        }
    }

    pub fn phase(&self) -> &SessionPhase {
        &self.phase
    }

    /// The config the active (or last-failed) session was started with.
    pub fn config(&self) -> Option<&SessionConfig> {
        self.config.as_ref()
    }

    /// Which session this is — bumps on every `begin`, so
    /// [`SessionEntity`] markers from an older session are
    /// distinguishable from the current one.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Fixed-step count within the session (`advance_session_tick`).
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Whether gameplay input should reach the simulation.
    pub fn is_playing(&self) -> bool {
        self.phase == SessionPhase::Playing
    }

    /// Validate `config` and start loading a new session
    /// (`Menu → Loading`). Illegal from any other phase — restart goes
    /// `Unloading → Menu` first, so a session always tears down before
    /// the next begins. Bumps [`generation`](Self::generation) and
    /// resets the session clock.
    pub fn begin(&mut self, config: SessionConfig) -> Result<(), SessionError> {
        config.validate().map_err(SessionError::Config)?;
        self.transition(SessionPhase::Loading)?;
        self.generation += 1;
        self.tick = 0;
        self.config = Some(config);
        Ok(())
    }

    /// Move to `Failed(reason)` — legal only from `Loading`/`Ready`, so
    /// failure is a load-time concept.
    pub fn fail(&mut self, reason: impl Into<String>) -> Result<(), SessionError> {
        self.transition(SessionPhase::Failed(reason.into()))
    }

    /// Advance the lifecycle. Illegal transitions are rejected and leave
    /// the phase unchanged.
    pub fn transition(&mut self, to: SessionPhase) -> Result<(), SessionError> {
        if self.legal(&to) {
            self.phase = to;
            Ok(())
        } else {
            Err(SessionError::IllegalTransition {
                from: self.phase.name(),
                to: to.name(),
            })
        }
    }

    fn legal(&self, to: &SessionPhase) -> bool {
        use SessionPhase::*;
        // Pause is a single-player concept: a session under host/remote
        // authority may not pause (MP-6). Without a config (Menu) the
        // question cannot arise, so default permissive.
        let pausable = self
            .config
            .as_ref()
            .is_none_or(|c| c.authority.allows_pause());
        match (&self.phase, to) {
            (Menu, Loading) => true,
            (Loading, Ready | Failed(_) | Unloading) => true,
            (Ready, Countdown | Playing | Failed(_) | Unloading) => true,
            (Countdown, Playing | Unloading) => true,
            (Playing, Paused) => pausable,
            (Playing, Results | Unloading) => true,
            (Paused, Playing | Unloading) => true,
            (Results | Failed(_), Unloading) => true,
            (Unloading, Menu) => true,
            _ => false,
        }
    }
}

/// Ownership marker for entities a session spawned; the value is the
/// session [`generation`](Session::generation) they belong to.
/// `despawn_session_entities` removes them wholesale on teardown —
/// persistent UI/profile state must never carry this marker.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionEntity(pub u64);

/// Despawn every session-owned root entity; their children cascade.
/// Only roots are matched — a session entity parented to a persistent
/// entity would survive, which is a caller bug, not something to clean
/// up silently. Schedule while the phase is `Unloading`.
pub fn despawn_session_entities(
    mut commands: Commands,
    roots: Query<Entity, (With<SessionEntity>, Without<ChildOf>)>,
) {
    for entity in &roots {
        commands.entity(entity).despawn();
    }
}

/// The session clock: one tick per fixed simulation step while the
/// phase is `Playing`. Schedule in `FixedUpdate` — the count cannot
/// change with render batching or frame rate.
pub fn advance_session_tick(mut session: ResMut<Session>) {
    if session.phase == SessionPhase::Playing {
        session.tick += 1;
    }
}
