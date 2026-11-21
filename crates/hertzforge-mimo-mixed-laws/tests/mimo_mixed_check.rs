//! The MIMO Mixed check: the built-in pair passes each of the nine required
//! cases, each on the simulator's existing structure its own facts name, and
//! the whole check's record clears the built-in in each role; a tone law
//! that misses its level and a floor law that overdrives fail.

use hertzforge_hal_sim::plants::SineStructure;
use hertzforge_hal_sim::SimConfig;
use hertzforge_mimo_mixed::service::MimoMixedLaws;
use hertzforge_mimo_mixed_laws::{
    mimo_mixed_check_config, mimo_mixed_check_record, mimo_mixed_check_station, role_record_clears,
    run_mimo_mixed_check_case, LawRole, MimoMixedCheckCase,
};
use hertzforge_mimo_sine::law::{
    LawDescriptor, LawManifest, LawSetup, LawValues, MimoSineLaw, MimoSineLawFactory,
};
use hertzforge_mimo_sine::tick::{LawFault, TickInputs, TickOutputs};
use hertzforge_random_laws::CheckStatus;
use std::sync::Arc;

fn station(case: MimoMixedCheckCase) -> Box<dyn hertzforge_hal::Backend> {
    let st = mimo_mixed_check_station(case);
    let s = SineStructure::from_name(st.structure).expect("an existing structure");
    let (n_out, n_d) = s.dims();
    assert_eq!(
        (n_out, s.n_controls(), n_d),
        (st.n_outputs, st.n_controls, st.n_drives),
        "{}",
        st.structure
    );
    Box::new(
        SimConfig::new()
            .sample_rate_hz(st.fs_hz)
            .plant(s.build().model)
            .noise_level(st.noise_rms_g)
            .transport_delay_samples(st.transport_delay_samples)
            .seed(st.seed)
            .shared_clock(true)
            .build(),
    )
}

fn builtin(case: MimoMixedCheckCase) -> MimoMixedLaws {
    MimoMixedLaws::builtin(&mimo_mixed_check_config(case)).expect("built-in laws")
}

/// Run `case` with the built-in pair and print its verdict.
fn built_in_case(case: MimoMixedCheckCase) -> bool {
    let t = std::time::Instant::now();
    let r = run_mimo_mixed_check_case(case, &builtin(case), station(case)).expect("run");
    eprintln!(
        "{} {} ({:.0} s): {}",
        if r.passed { "PASS" } else { "FAIL" },
        r.name,
        t.elapsed().as_secs_f64(),
        r.detail
    );
    r.passed
}

/// Requirement, for each case: the built-in floor and tone laws pass the
/// case's stated requirement (`MimoMixedCheckCase::requirement`).
macro_rules! built_in_passes {
    ($($(#[$m:meta])* $test:ident => $case:ident;)*) => {$(
        $(#[$m])*
        #[test]
        fn $test() {
            assert!(built_in_case(MimoMixedCheckCase::$case));
        }
    )*};
}

built_in_passes! {
    mimo_mixed_check_floor_only => FloorOnly;
    mimo_mixed_check_fixed_tones_on_floor => FixedTonesOnFloor;
    mimo_mixed_check_narrowband_fixed => NarrowbandFixed;
    mimo_mixed_check_narrowband_swept => NarrowbandSwept;
    mimo_mixed_check_level_ladder => LevelLadder;
    mimo_mixed_check_shared_ceiling => SharedCeiling;
    mimo_mixed_check_more_drives_than_controls => MoreDrivesThanControls;
    mimo_mixed_check_transport_delay => TransportDelay;
    mimo_mixed_check_limit_channel => LimitChannel;
}

/// Every required case has a test above; the informational one runs in the
/// whole check below.
#[test]
fn mimo_mixed_check_every_case_is_tested() {
    assert_eq!(MimoMixedCheckCase::ALL.len(), 10);
    assert_eq!(MimoMixedCheckCase::REQUIRED.len(), 9);
    assert_eq!(
        MimoMixedCheckCase::INFORMATIONAL,
        [MimoMixedCheckCase::SweptToneThroughMode]
    );
}

/// Requirement: the whole check, every case in its order, with the built-in
/// floor and tone laws, gives a record that passes and clears the law in
/// either role; the informational swept-tone case is run and its result is
/// in the record, outside the verdict.
#[test]
fn mimo_mixed_the_built_in_laws_pass_the_whole_required_check_in_each_role() {
    const SHA: &str = "cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34";
    let cases: Vec<_> = MimoMixedCheckCase::ALL
        .into_iter()
        .map(|case| {
            let r = run_mimo_mixed_check_case(case, &builtin(case), station(case)).expect("run");
            eprintln!(
                "{} {}: {}",
                if r.passed { "PASS" } else { "FAIL" },
                r.name,
                r.detail
            );
            r
        })
        .collect();
    let record = mimo_mixed_check_record(SHA, "2026-11-21T10:00:00Z", cases);
    let failed: Vec<_> = record
        .cases
        .iter()
        .filter(|c| !c.passed)
        .map(|c| &c.name)
        .collect();
    assert!(record.passed, "required cases failed: {failed:?}");
    assert_eq!(record.cases.len(), MimoMixedCheckCase::REQUIRED.len());
    assert_eq!(record.informational.len(), 1);
    assert_eq!(record.informational[0].name, "swept_tone_through_mode");
    eprintln!(
        "informational swept_tone_through_mode passed {}: {}",
        record.informational[0].passed, record.informational[0].detail
    );
    // The check runs a law in one role with the built-in in the other, so
    // the built-in pair's run is each role's: the record, written under
    // either role's name, clears it.
    for role in [LawRole::Floor, LawRole::Tones] {
        assert_eq!(
            role_record_clears(&CheckStatus::Checked(record.clone()), SHA),
            Ok(()),
            "{}",
            hertzforge_mimo_mixed_laws::mimo_mixed_check_name(role)
        );
    }
}

/// A tone law that answers as the built-in, then sends half its drive (6 dB
/// under what the built-in asks).
struct HalfDrive(Arc<dyn MimoSineLawFactory>);

struct HalfDriveLaw(Box<dyn MimoSineLaw + Send>);

impl MimoSineLaw for HalfDriveLaw {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn engage(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        v: &mut LawValues,
    ) -> Result<(), LawFault> {
        self.0.engage(x, o, v)?;
        o.drive.iter_mut().for_each(|d| *d *= 0.5);
        Ok(())
    }
    fn tick(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        v: &mut LawValues,
    ) -> Result<(), LawFault> {
        self.0.tick(x, o, v)?;
        o.drive.iter_mut().for_each(|d| *d *= 0.5);
        Ok(())
    }
}

impl MimoSineLawFactory for HalfDrive {
    fn build(&self, setup: &LawSetup) -> Result<Box<dyn MimoSineLaw + Send>, LawFault> {
        Ok(Box::new(HalfDriveLaw(self.0.build(setup)?)))
    }
    fn describe(&self) -> LawDescriptor {
        self.0.describe()
    }
    fn manifest(&self) -> LawManifest {
        self.0.manifest()
    }
}

/// Requirement: a tone law that sends half the drive it should fails the
/// fixed-tones case (its tones sit under their alarm band, or abort).
#[test]
fn mimo_mixed_a_tone_law_that_misses_its_level_fails_the_check() {
    let case = MimoMixedCheckCase::FixedTonesOnFloor;
    let mut laws = builtin(case);
    laws.tones = Arc::new(HalfDrive(laws.tones.clone()));
    let r = run_mimo_mixed_check_case(case, &laws, station(case)).expect("run");
    eprintln!("half-drive law: {}", r.detail);
    assert!(!r.passed);
}

/// A floor law that seeds as the built-in, then stops adapting: every call
/// after its seed answers with its seed at twice the drive power (3 dB over
/// it). A law that only scaled each answer of the built-in would be
/// corrected by the loop itself, its next answer asking less.
struct DoubleFloor(Arc<dyn hertzforge_random_laws::MimoLawFactory>);

struct DoubleFloorLaw(
    Box<dyn hertzforge_sdm::law::MimoControlLaw + Send>,
    Option<hertzforge_sdm::law::MimoLoopOutputs>,
);

fn doubled(
    r: Result<hertzforge_sdm::law::MimoLoopOutputs, hertzforge_sdm::law::LawError>,
) -> Result<hertzforge_sdm::law::MimoLoopOutputs, hertzforge_sdm::law::LawError> {
    r.map(|mut o| {
        let twice = vec![2.0; o.drive.lines()];
        o.drive.scale_lines(&twice).expect("one factor a line");
        o
    })
}

impl hertzforge_sdm::law::MimoControlLaw for DoubleFloorLaw {
    fn needs(&self) -> &[hertzforge_sdm::law::MimoInput] {
        self.0.needs()
    }
    fn shapes(&self) -> &[hertzforge_sdm::law::MimoShape] {
        self.0.shapes()
    }
    fn seed(
        &mut self,
        x: &hertzforge_sdm::law::MimoLoopInputs<'_>,
    ) -> Result<hertzforge_sdm::law::MimoLoopOutputs, hertzforge_sdm::law::LawError> {
        let o = self.0.seed(x)?;
        self.1 = Some(o.clone());
        Ok(o)
    }
    fn iterate(
        &mut self,
        x: &hertzforge_sdm::law::MimoLoopInputs<'_>,
    ) -> Result<hertzforge_sdm::law::MimoLoopOutputs, hertzforge_sdm::law::LawError> {
        self.0.iterate(x)?;
        doubled(
            self.1
                .clone()
                .ok_or_else(|| hertzforge_sdm::law::LawError("iterated before its seed".into())),
        )
    }
}

impl hertzforge_random_laws::MimoLawFactory for DoubleFloor {
    fn identity(&self) -> hertzforge_law_host::LawIdentity {
        self.0.identity()
    }
    fn params(&self) -> &[hertzforge_law_host::Param] {
        self.0.params()
    }
    fn build(
        &self,
        setup: &hertzforge_sdm::law::MimoSetup,
        budget: std::time::Duration,
    ) -> Result<
        Box<dyn hertzforge_sdm::law::MimoControlLaw + Send>,
        hertzforge_random_laws::ChoiceError,
    > {
        Ok(Box::new(DoubleFloorLaw(self.0.build(setup, budget)?, None)))
    }
}

/// Requirement: a floor law that stops adapting at twice the drive power it
/// should ask fails the floor-only case (its floor sits over its 1 dB band,
/// or aborts).
#[test]
fn mimo_mixed_a_floor_law_that_overdrives_fails_the_check() {
    let case = MimoMixedCheckCase::FloorOnly;
    let mut laws = builtin(case);
    laws.floor = Arc::new(DoubleFloor(laws.floor.clone()));
    let r = run_mimo_mixed_check_case(case, &laws, station(case)).expect("run");
    eprintln!("double-drive floor law: {}", r.detail);
    assert!(!r.passed);
}
