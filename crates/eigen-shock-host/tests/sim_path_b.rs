//! Path-B (HostDac/Sim) closed-loop demonstration - the OTHER half of the
//! compensation honesty split. Moved here from the Path-A crate when the
//! host-side module was split out: everything it drives is this crate's.
//!
//! - Path A (delegated): the UNCOMPENSATED main pulse is downloaded and the
//!   device compensates on-device (covered by the `delegated` module tests).
//! - Path B (host): Eigen synthesizes the HOST-COMPENSATED composite, drives it
//!   through the DAC (SimBackend here), and the tolerance checks run on the
//!   measured control channel. This test closes that loop on Sim: the composite
//!   is legal as a `ReferenceForPath::host` reference (the delegated seam
//!   rejects it), the sim "measures" it, and all three tolerance checks pass.
//!
//! The compensation geometry is chosen deliberately: rectangular shoulders at
//! 4x the main duration solve to ~-8% of the peak, which fits INSIDE the
//! MIL-STD-810 15%-of-peak surround band and costs ~6% of the windowed dV -
//! inside the 10% dV band. The DEFAULT 1.5x half-sine shoulders (~-33%) are
//! legal to drive but would trip the surround band: that is the real
//! amplitude-vs-duration compensation trade, surfaced by the checker rather
//! than hidden.

use eigen_channel_table::ChannelTable;
use eigen_dsp::shock::{CompensationConfig, CompensationShape};
use eigen_hal::Backend;
use eigen_hal_sim::SimConfig;
use eigen_shock_common::{DrivePath, ReferenceForPath};
use eigen_shock_host::{check_tolerance, ClassicalPulseSpec, ClassicalShape, ToleranceStandard};

const FS: f64 = 5120.0;

#[test]
fn sim_host_path_drives_compensated_composite_and_passes_tolerance() {
    // 20 g / 11 ms half-sine with long/low rectangular shoulders (see module
    // docs for why this geometry).
    let mut spec = ClassicalPulseSpec::new(ClassicalShape::HalfSine, 20.0, 11.0);
    spec.compensation = CompensationConfig {
        shape: CompensationShape::Rectangular,
        pre_frac: 4.0,
        post_frac: 4.0,
        symmetric: true,
    };
    let comp = spec.synthesize(FS);
    assert!(comp.residual_velocity_ips.abs() < 1e-6);
    assert!(comp.residual_displacement_in.abs() < 1e-8);

    // The composite is a legal HOST (Path B) reference; the same waveform is
    // REJECTED by the delegated constructor (covered in delegated tests).
    let host_ref = ReferenceForPath::host(&comp.waveform_g).unwrap();
    assert_eq!(host_ref.path(), DrivePath::HostDac);

    // A clean unity plant: pure feedthrough, no resonant modes, no noise, no
    // transport delay - the sim "shaker" reproduces the drive on its inputs.
    let mut backend = SimConfig::new()
        .sample_rate_hz(FS)
        .resonances(Vec::new())
        .noise_level(0.0)
        .feedthrough(1.0)
        .shared_clock(true)
        .build();
    // Fixture table: channel 0 is its Control row (deprecated outside tests).
    #[allow(deprecated)]
    let table = ChannelTable::nonuniform_default_8ch();
    backend.open().unwrap();
    let acq = eigen_hal::AcquisitionConfig::new(backend.caps().sample_rate_hz, 1);
    backend.configure(&table, &acq).unwrap();
    backend.arm().unwrap();
    backend.start().unwrap();

    // Drive the composite sample by sample with quiet lead-in/lead-out, reading
    // the (8-wide) input frame each step and keeping channel 0 (Control).
    let n_ch = 8usize;
    let quiet = vec![0.0f64; 200];
    let mut measured = Vec::new();
    for &d in quiet
        .iter()
        .chain(host_ref.waveform_g().iter())
        .chain(quiet.iter())
    {
        backend.write_drive(&[d as f32]).unwrap();
        let mut buf = vec![0.0f32; n_ch];
        let frames = backend.read_input(&mut buf).unwrap();
        assert_eq!(frames, eigen_hal::InputRead::Frames(1));
        measured.push(buf[0] as f64);
    }
    backend.stop().unwrap();

    // The sim-measured pulse passes all three tolerance checks under MIL-STD-810.
    let report = check_tolerance(&measured, &spec, FS, ToleranceStandard::Mil810_516);
    assert!(report.overall_pass, "{report:#?}");
    // The nominal main pulse sits after the quiet lead-in + the pre shoulder.
    assert_eq!(
        report.aligned_pulse_start_index,
        (quiet.len() + comp.pre_len) as i64
    );
    // The compensation shoulders are VISIBLE to the checker: they occupy the
    // surround windows (within band) and bite ~6% out of the windowed dV
    // (within the 10% band) - the honest Path-B accounting.
    assert!(report.amplitude_envelope.surround_pass);
    assert!(report.velocity_change.error_frac < -0.02);
    assert!(report.velocity_change.error_frac > -0.10);

    // IEC's wider bands pass as well.
    let iec = check_tolerance(&measured, &spec, FS, ToleranceStandard::Iec60068_2_27);
    assert!(iec.overall_pass, "{iec:#?}");
}

#[test]
fn sim_measured_composite_is_rejected_by_the_delegated_seam() {
    // Belt-and-braces on the compensation split from the Path-B side: what Path B drives
    // must never be downloadable on Path A.
    let spec = ClassicalPulseSpec::new(ClassicalShape::HalfSine, 20.0, 11.0);
    let comp = spec.synthesize(FS);
    assert!(ReferenceForPath::delegated(&comp.waveform_g).is_err());
    assert!(ReferenceForPath::host(&comp.waveform_g).is_ok());
}
