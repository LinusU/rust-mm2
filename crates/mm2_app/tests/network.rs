//! Network integration tests, including real host/join process tests.
//! Add new test modules here; automatic test-target discovery is disabled.

mod support;

mod net_app;
mod net_baseline;
mod net_check;
mod net_cnr_authority;
mod net_cnr_bot_replication;
mod net_cnr_bots_mp;
mod net_cnr_lobby_bots;
mod net_commands;
mod net_contact;
mod net_drive;
mod net_edge;
mod net_focus_handoff;
mod net_host;
mod net_island_retail;
mod net_join;
mod net_lane_loss;
mod net_lane_pace;
mod net_lifecycle;
mod net_matrix;
mod net_partial_snaps;
mod net_peer_adapt;
mod net_qualify;
mod net_remote_commands;
mod net_remote_predict;
