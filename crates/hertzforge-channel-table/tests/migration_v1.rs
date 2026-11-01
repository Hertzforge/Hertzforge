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
//! Both flat and structured doors are covered, because they date a table
//! differently: JSON carries `schema_version` (absent = v1), the flat forms
//! carry nothing at all and date themselves by column width.

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
    assert!(ch2.stream);
    assert_eq!(ch2.abort.velocity_ips, Some(70.0));
    assert_eq!(ch2.abort.displacement_mil, Some(2250.0));

    // The reference spec (JSON-only) survived too.
    let spec = t.reference.as_ref().expect("the v1 reference spec");
    assert_eq!(spec.n_reference, 0);
    assert!(spec.notes.contains("before the v2 schema bump"));

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

/// The frozen v1 CSV (16 columns, no version marker anywhere) dates itself by
/// width and lands in the same place.
#[test]
fn the_v1_csv_fixture_loads_and_migrates() {
    let t = ChannelTable::from_csv(&fixture("v1_table.csv")).expect("a v1 CSV must still load");
    assert_eq!(t.schema_version, SCHEMA_VERSION);
    assert_eq!(
        t.channels.len(),
        2,
        "the v1 header row must not become a channel"
    );
    assert_eq!(t.channels[0].description, "Airbags");
    assert_eq!(t.channels[0].interface, Interface::Iepe);
    assert_eq!(t.channels[0].role, ChannelRole::Control);
    assert_eq!(t.channels[1].units, Units::Custom("psi".into()));
    assert_eq!(t.channels[1].abort.displacement_mil, Some(2250.0));
    for ch in &t.channels {
        assert_eq!(ch.physical, None);
        assert_eq!(ch.analysis_tier, AnalysisTier::Full);
    }
    t.validate().unwrap();
}

/// The two doors agree: the same table authored as v1 JSON and as v1 CSV
/// migrates to the same channels. (CSV drops the reference spec by design - it
/// is a channel-rows-only format - so only `channels` is compared.)
#[test]
fn both_v1_doors_migrate_to_the_same_channels() {
    let from_json = ChannelTable::from_json(&fixture("v1_table.json")).unwrap();
    let from_csv = ChannelTable::from_csv(&fixture("v1_table.csv")).unwrap();
    assert_eq!(from_json.channels, from_csv.channels);
}

/// A migrated v1 table RE-EXPORTS at v2: the round-trip out of the migration is
/// the new schema, and re-importing it is a no-op. Without this the migration
/// could be a read-time cosmetic that never reaches disk.
#[test]
fn a_migrated_v1_table_re_exports_at_v2_and_re_imports_unchanged() {
    let t = ChannelTable::from_json(&fixture("v1_table.json")).unwrap();

    let csv = t.to_csv();
    assert!(
        csv.lines().next().unwrap().ends_with("desc,physical,tier"),
        "the export must carry the v2 columns: {}",
        csv.lines().next().unwrap()
    );
    let back = ChannelTable::from_csv(&csv).unwrap();
    assert_eq!(back.schema_version, SCHEMA_VERSION);
    assert_eq!(back.channels, t.channels);

    let json = t.to_json().unwrap();
    assert!(json.contains("\"schema_version\": 2"));
    assert_eq!(ChannelTable::from_json(&json).unwrap(), t);
}

/// Migration is not a one-way door out of usefulness: a v1 table that has been
/// migrated can then be BOUND to physical channels and round-trips with them.
/// This is the whole point of the bump.
#[test]
fn a_migrated_v1_table_can_then_be_bound_and_round_trips() {
    let mut t = ChannelTable::from_csv(&fixture("v1_table.csv")).unwrap();
    t.channels[0].physical = Some(PhysicalChannelRef::new("cDAQ9178Sim1Mod1", "ai0"));
    t.channels[1].physical = Some(PhysicalChannelRef::new("cDAQ9178Sim1Mod1", "ai1"));
    t.channels[1].analysis_tier = AnalysisTier::Decimated(8);

    for reloaded in [
        ChannelTable::from_csv(&t.to_csv()).unwrap(),
        ChannelTable::from_tsv(&t.to_tsv()).unwrap(),
        ChannelTable::from_json(&t.to_json().unwrap()).unwrap(),
    ] {
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
}
