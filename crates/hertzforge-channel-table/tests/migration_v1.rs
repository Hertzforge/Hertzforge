//! The schema v1 -> v2 migration, proven against FILES rather than against
//! strings written next to the assertions.
//!
//! The distinction matters. A migration test whose input is a literal in the
//! same file as the parser is really a test that the parser agrees with itself:
//! the moment someone widens the schema again, the literal gets updated in the
//! same edit and the test keeps passing while nothing is proven about the tables
//! already on operators' disks. The fixtures under `tests/fixtures/` are frozen
//! artifacts of the PRE-BUMP schema. Nothing may edit them - a change to a
//! fixture is a change to what a v1 file was, which is not a thing this project
//! is allowed to do.
//!
//! JSON is the one door: it carries `schema_version`, and its absence is v1.
//! (The flat forms this crate once had are retired; the table's files are the
//! analysis side's.)

use hertzforge_channel_table::{
    AnalysisTier, ChannelRole, ChannelTable, Interface, PhysicalChannelRef, Polarity, Units,
    SCHEMA_VERSION,
};

fn fixture(name: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("reading {}: {e}", p.display()))
}

/// The frozen v1 JSON loads at the CURRENT schema, every v1 field intact and
/// every v2 field at its honest default.
#[test]
fn the_v1_json_fixture_loads_and_migrates() {
    let t = ChannelTable::from_json(&fixture("v1_table.json")).expect("a v1 file must still load");

    assert_eq!(
        t.schema_version, SCHEMA_VERSION,
        "the loaded table must be at the current schema, not the one the file was written at"
    );
    assert!(!t.needs_migration());
    assert_eq!(t.channels.len(), 2);

    // Every v1 field survived, including the ones with non-default values that a
    // sloppy migration would flatten.
    let ch1 = &t.channels[0];
    assert_eq!(ch1.index, 1);
    assert_eq!(ch1.interface, Interface::Iepe);
    assert_eq!(ch1.iepe_bias_v, Some(12.5));
    assert_eq!(ch1.units, Units::AccelG);
    assert_eq!(ch1.sensitivity_mv_per_eu, 1000.0);
    assert_eq!(ch1.range_max, 5.0);
    assert_eq!(ch1.role, ChannelRole::Control);
    assert_eq!(ch1.control_group, Some(0));
    assert_eq!(ch1.abort.accel_g, Some(9.0));
    assert_eq!(ch1.description, "Airbags");

    let ch2 = &t.channels[1];
    assert_eq!(ch2.units, Units::Custom("psi".into()));
    assert_eq!(ch2.polarity, Polarity::Negative);
    assert_eq!(ch2.abort.velocity_ips, Some(70.0));
    assert_eq!(ch2.abort.displacement_mil, Some(2250.0));

    // The v2 fields default honestly: UNBOUND, not bound to something invented.
    for ch in &t.channels {
        assert_eq!(
            ch.physical, None,
            "a v1 row names no physical channel, so the migrated row must not either"
        );
        assert_eq!(ch.analysis_tier, AnalysisTier::Full);
    }

    t.validate()
        .expect("a valid v1 table stays valid after migration");
}

/// A migrated v1 table RE-EXPORTS at v2: the round-trip out of the migration is
/// the new schema, and re-importing it is a no-op. Without this the migration
/// could be a read-time cosmetic that never reaches disk.
#[test]
fn a_migrated_v1_table_re_exports_at_v2_and_re_imports_unchanged() {
    let t = ChannelTable::from_json(&fixture("v1_table.json")).unwrap();

    let json = t.to_json().unwrap();
    assert!(json.contains("\"schema_version\": 2"));
    assert_eq!(ChannelTable::from_json(&json).unwrap(), t);
}

/// Migration is not a one-way door out of usefulness: a v1 table that has been
/// migrated can then be BOUND to physical channels and round-trips with them.
/// This is the whole point of the bump.
#[test]
fn a_migrated_v1_table_can_then_be_bound_and_round_trips() {
    let mut t = ChannelTable::from_json(&fixture("v1_table.json")).unwrap();
    t.channels[0].physical = Some(PhysicalChannelRef::new("cDAQ9178Sim1Mod1", "ai0"));
    t.channels[1].physical = Some(PhysicalChannelRef::new("cDAQ9178Sim1Mod1", "ai1"));
    t.channels[1].analysis_tier = AnalysisTier::Decimated(8);

    let reloaded = ChannelTable::from_json(&t.to_json().unwrap()).unwrap();
    assert_eq!(reloaded.channels, t.channels);
    assert_eq!(
        reloaded.channels[0].physical.as_ref().unwrap().device_id,
        "cDAQ9178Sim1Mod1"
    );
    assert_eq!(
        reloaded.channels[1].analysis_tier,
        AnalysisTier::Decimated(8)
    );
}
