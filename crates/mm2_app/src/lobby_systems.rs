//! The lobby's per-update system sets, shared by the `mm2` binary and the
//! menu tests so a test drives the schedule a player's process runs, not
//! a hand-copied subset of it. Each set only runs while its link
//! resource exists, so a menu app (whose rows open a link at run time)
//! registers both up front.
//!
//! `capturing` is the binary's "a screenshot capture owns the input"
//! condition; the library does not know the smoke record, so the caller
//! passes it in.

use bevy::ecs::schedule::SystemCondition;
use bevy::prelude::*;

use crate::{
    audio, cnr, cnrnet, input, net, netdrive, scripted, sequence, session, worldclock, worldprops,
    worldtraffic,
};

/// Every system of the joined lobby's set, so the close that removes its
/// link can be ordered after all of them.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
struct JoinedLobby;

/// Every system of the hosted lobby's set, likewise.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
struct HostedLobby;

/// The joined lobby's per-update systems. Registered for a `--join`
/// launch and for the menu (whose *Join lobby* row opens a link at run
/// time); every one only runs while a [`net::LobbyLink`] exists.
pub fn add_client_systems<M>(app: &mut App, capturing: impl SystemCondition<M> + Clone) {
    app.add_systems(
        Update,
        (
            // Frozen during a capture like every other input —
            // a `--join --frames` screenshot must be
            // reproducible.
            net::lobby_input.run_if(not(capturing)),
            net::drive_lobby.after(session::drive_session),
            net::drive_lobby_text,
            // F25-A: the roster/host-pick drives remote
            // participant spawning; the newest drained snapshot
            // feeds their lerp. Both run after the drain sees
            // this frame's wire state. The apply runs after the
            // reconcile so its `NetPlayer` stamps and remote
            // spawns — deferred inserts — are visible the same
            // update they land: a snap held through the session
            // load then applies whole rather than skipping the
            // local seat's rows (its v14 terminal edge
            // included) on the frame they become receivable.
            netdrive::reconcile_remote_players.after(net::drive_lobby),
            netdrive::apply_snapshots
                .after(net::drive_lobby)
                .after(netdrive::reconcile_remote_players),
            netdrive::drive_remote_lerp,
            // F26-A: the host's knocked/broken/settled props
            // fold into the client's own stamped world.
            worldprops::apply_props.after(net::drive_lobby),
            // F26-A: the host's ambient cars — the copies a
            // `Remote` city Cruise session poses.
            worldtraffic::apply_traffic.after(net::drive_lobby),
            // F26-A: the host's world clock — the timed
            // scenery re-seeks to it on a Cruise client too.
            worldclock::apply_world_clock.after(net::drive_lobby),
            // F27-B.3: the host's Cops & Robbers match — the
            // replica the HUD and markers read (idle until a
            // match frame arrives).
            cnrnet::apply_cnr.after(net::drive_lobby),
            // F27-B.4c: the host's decided match ends this
            // client's `Playing` into the match-over screen.
            cnr::end_replicated_match.after(cnrnet::apply_cnr),
            // F25-B: `R` under a predicted session asks the
            // authority for the reset `reset_input` is gated
            // against — the granted answer arrives as the
            // own-seat epoch snap.
            netdrive::send_reset_request,
            // `--reset-at` is the scheduled `R`: the same ask.
            netdrive::send_dev_reset_request,
            // The wire sample reads the settled `VehicleInput`
            // — after the keyboard mapping and every scripted
            // owner that can overwrite it.
            netdrive::send_drive_input
                .after(input::vehicle_input)
                .after(input::parked_drive)
                .after(input::ram_drive)
                .after(scripted::scripted_drive)
                .after(sequence::sequence_drive),
        )
            .in_set(JoinedLobby)
            .run_if(resource_exists::<net::LobbyLink>),
    )
    // A lobby the menu opened goes with its link once it has ended; a
    // no-op without a menu. It removes the link and its companion
    // resources by deferred commands, and a sync point can apply those
    // mid-schedule, so it runs after every system that reads them.
    .add_systems(
        Update,
        net::close_menu_join
            .after(JoinedLobby)
            .run_if(resource_exists::<net::LobbyLink>),
    );
}

/// The hosted lobby's per-update systems. Registered for a `--host`
/// launch and for the menu (whose *Host lobby* row opens a link at run
/// time); every one only runs while a [`net::HostLink`] exists.
pub fn add_host_systems<M>(app: &mut App, capturing: impl SystemCondition<M> + Clone) {
    app.add_systems(
        Update,
        (
            net::host_input.run_if(not(capturing)),
            net::drive_host.after(session::drive_session),
            net::drive_host_text,
            // F25-A: the roster drives remote participant
            // spawning; their `VehicleInput` comes from the
            // wire mailbox and their settled poses go back out
            // as snapshots — all after the drain sees this
            // frame's lobby events. Resets bump the wire epoch
            // before the publish so a teleport and its epoch
            // leave on the same `Snap`; the tracker also runs
            // after `vehicle_reset`, which every Update-scheduled
            // `ResetVehicle` writer is ordered ahead of — so the
            // bump never trails the teleported pose.
            netdrive::reconcile_remote_players.after(net::drive_host),
            netdrive::apply_remote_inputs.after(net::drive_host),
            // F25-B: a wire seat whose input stream stalled is
            // retired — its `TimedOut` mint releases the
            // deferral and rides the next `Snap` like any
            // resolution.
            netdrive::retire_stalled_wire_seats
                .after(net::drive_host)
                .before(netdrive::publish_snapshots),
            // F25-B: driver `ResetRequest`s are `ResetVehicle`
            // writers — ahead of the apply like every other so
            // the granted reset's pose and epoch bump leave on
            // the same `Snap`.
            netdrive::apply_reset_requests
                .after(net::drive_host)
                .before(mm2_vehicle::systems::vehicle_reset),
            netdrive::track_reset_epochs
                .after(net::drive_host)
                .after(mm2_vehicle::systems::vehicle_reset)
                .before(netdrive::publish_snapshots),
            netdrive::publish_snapshots
                .after(net::drive_host)
                .after(mm2_vehicle::systems::vehicle_reset)
                // F25-B (v16): the seat's `SurfaceContact`
                // publish reads `surface_voices`' same-frame
                // resolution, not last frame's.
                .after(audio::surface_voices),
            // F26-A: the world's prop state rides its own frame
            // — after the lobby drain, like the snapshot.
            worldprops::publish_props.after(net::drive_host),
            // F26-A: the ambient population rides its own frame.
            worldtraffic::publish_traffic.after(net::drive_host),
            // F26-A: the world clock rides its own tiny frame.
            worldclock::publish_world_clock.after(net::drive_host),
            // F27-B.3: the Cops & Robbers match rides its own
            // frame (idle until a `CnrHost` exists).
            cnrnet::publish_cnr.after(net::drive_host),
        )
            .in_set(HostedLobby)
            .run_if(resource_exists::<net::HostLink>),
    )
    // A lobby the menu opened goes with its link once it has come down; a
    // no-op without a menu. It removes the link and its companion
    // resources by deferred commands, and a sync point can apply those
    // mid-schedule, so it runs after every system that reads them.
    .add_systems(
        Update,
        net::close_menu_host
            .after(HostedLobby)
            .run_if(resource_exists::<net::HostLink>),
    );
}
