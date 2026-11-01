// Discriminator, pure software: is the boundary silence IN the
// synthesized drive itself? The AO buffer is FULL at every boundary
// (space_after = 0), so the quiet the DMM measured on the wire cannot be a
// buffering gap - it has to be in the samples that were queued.
#[test]
fn drive_is_continuous_across_segment_seams() {
    use eigen_dsp::cola::ColaSynth;
    let nfft = 2048usize;
    let overlap = 0.5;
    let fs = 8533.33;
    let hop = ((nfft as f64) * (1.0 - overlap)).round() as usize;
    let blocks_per_window = 6usize;
    let want = blocks_per_window * nfft;
    let window_len = want.div_ceil(hop) * hop;

    // Flat PSD, constant across segments: any amplitude structure in the
    // output is the synthesizer's, not the profile's.
    let psd = vec![1e-4f64; nfft / 2 + 1];
    let mut cola = ColaSynth::with_seed(nfft, overlap, fs, 12345);

    // Mimic DriveSynth::segment across several consecutive segments.
    let mut all: Vec<f64> = Vec::new();
    let mut seams: Vec<usize> = Vec::new();
    for _ in 0..5 {
        let mut out: Vec<f64> = Vec::with_capacity(window_len);
        while out.len() < window_len {
            out.extend_from_slice(&cola.next_frame(&psd));
        }
        out.truncate(window_len);
        seams.push(all.len());
        all.extend_from_slice(&out);
    }

    // Block RMS in 10 ms blocks, exactly as the DMM analysis did.
    let blk = (fs * 0.010) as usize;
    let n = all.len() / blk;
    let rms: Vec<f64> = (0..n)
        .map(|i| {
            let s: f64 = all[i * blk..(i + 1) * blk].iter().map(|v| v * v).sum();
            (s / blk as f64).sqrt()
        })
        .collect();
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = sorted[n / 2];
    // The first ~2 blocks are the COLA overlap-add ramp-up of a FRESH
    // synthesizer - expected, and one-time per run (DriveSynth is built once at
    // arm, never per segment). Everything after that must be stationary.
    const RAMP_BLOCKS: usize = 3;
    let dips: Vec<usize> = (RAMP_BLOCKS..n).filter(|&i| rms[i] < 0.2 * med).collect();
    println!(
        "window_len={window_len} hop={hop} blocks={n} median={med:.6} dips={:?}",
        dips
    );
    for (k, s) in seams.iter().enumerate() {
        let b = s / blk;
        let around: Vec<String> = (b.saturating_sub(1)..(b + 3).min(n))
            .map(|i| format!("{:.4}", rms[i]))
            .collect();
        println!("seam {k} at sample {s} (block {b}): rms around = {around:?}");
    }
    assert!(
        dips.is_empty(),
        "synthesized drive has {} quiet block(s) past the startup ramp - the \
         boundary silence measured on the wire would be IN THE SYNTHESIS",
        dips.len()
    );
    // And specifically at each seam: the overlap-add must carry across the
    // `segment()` call boundary with no amplitude notch.
    for (k, s) in seams.iter().enumerate().skip(1) {
        let b = s / blk;
        assert!(
            rms[b] > 0.5 * med,
            "seam {k} (block {b}) dipped to {:.4} against median {med:.4}",
            rms[b]
        );
    }
}
