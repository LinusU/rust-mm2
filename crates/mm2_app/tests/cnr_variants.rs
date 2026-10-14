//! F27-AC06: each stock Cops & Robbers variant is started on its own,
//! in each city, from the retail install — a single chase sandbox
//! cannot stand in for the other variants. `MM2_RETAIL`-gated; the
//! per-variant evidence levels are recorded in `docs/cnr-variants.md`.

use bevy::prelude::*;
use mm2_app::cnr::CnrHost;
use mm2_content::cnr::{CnrContent, CnrSettings, GoldMass};
use mm2_game::gold::{CnrVariant, Contact, DeliveryVerdict, PickupVerdict, Side};
use mm2_game::{PlayerId, Session, SessionConfig};

const TICK_HZ: u32 = 60;

/// Four participants dealt over the variant's sides, in order.
fn roster(variant: CnrVariant) -> Vec<(PlayerId, Side)> {
    let sides = variant.sides();
    (0..4u16)
        .map(|i| (PlayerId(i), sides[i as usize % sides.len()]))
        .collect()
}

#[test]
fn retail_every_variant_starts_and_plays_a_round_in_each_city() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut vfs = mm2_assets::Vfs::new();
    mm2_assets::mount_install(&mut vfs, &retail, &mm2_assets::InstallMount::default()).unwrap();

    let mut started = 0;
    for city in ["sf", "london"] {
        let content = CnrContent::load(&vfs, city);
        assert!(
            content.sites.len() >= 3,
            "{city}: the authored site pool seeds a round ({} sites)",
            content.sites.len()
        );
        for variant in CnrVariant::ALL {
            let what = format!("{city} {variant:?}");
            let settings = CnrSettings {
                variant,
                gold_mass: GoldMass::QuarterTon,
                ..CnrSettings::default()
            };
            let mut session = Session::new();
            session.begin(SessionConfig::default()).unwrap();
            let gold = session.mint_object_id();
            let participants = roster(variant);
            let mut host = CnrHost::from_content(
                &content,
                &settings,
                TICK_HZ,
                session.generation(),
                gold,
                7,
                &participants,
            )
            .unwrap_or_else(|e| panic!("{what}: {e:?}"));
            let game = &mut host.game;
            assert_eq!(game.rules().variant, variant, "{what}");
            for (id, side) in &participants {
                assert_eq!(game.side_of(*id), Some(*side), "{what}");
            }

            // A pickup by the first participant: they carry, and only
            // they bear the load.
            let (who, side) = participants[0];
            let at = game.gold_position().unwrap();
            let round = game.round();
            let verdicts = game.resolve_pickups(&[Contact {
                player: who,
                round,
                position: at,
            }]);
            assert!(
                matches!(verdicts[0], PickupVerdict::Granted { .. }),
                "{what}: {verdicts:?}"
            );
            assert_eq!(game.carrier(), Some(who), "{what}");
            assert!(
                game.load_for(who).is_some(),
                "{what}: the carrier is loaded"
            );
            assert!(game.load_for(participants[1].0).is_none(), "{what}");

            // The delivery lands at the side's own marker, and scores
            // for the side the variant counts.
            let target = game.sites().target(side.delivery_target());
            match game.deliver(who, round, target) {
                DeliveryVerdict::Delivered { points, .. } => {
                    assert!(points > 0, "{what}");
                    assert_eq!(game.score(who), Some(points + 25), "{what}");
                    if variant.team_scored() {
                        assert_eq!(game.side_total(side), points + 25, "{what}");
                    }
                }
                other => panic!("{what}: delivery refused: {other:?}"),
            }
            assert_eq!(
                game.carrier(),
                None,
                "{what}: the load ends with the delivery"
            );
            started += 1;
        }
    }
    assert_eq!(started, 6, "three variants in two cities");
}
