//! ANSI S1.11 base-ten fractional-octave bands and each narrowband line's bin
//! overlap with them.

/// Lower and upper edges of the 1/N-octave bands overlapping `f_min..f_max`
/// (base-10 octave ratio 10^0.3, 1 kHz reference).
pub fn nth_octave_edges(f_min: f64, f_max: f64, oct_order: usize) -> (Vec<f64>, Vec<f64>) {
    let ratio = 10f64.powf(0.3);
    let n = oct_order as f64;
    let round12 = |x: f64| (x * 1e12).round_ties_even() / 1e12;
    let (mut lb, mut ub) = (Vec::new(), Vec::new());
    for b in -200..=200 {
        let b = b as f64;
        let power = if oct_order.is_multiple_of(2) {
            (2.0 * b + 1.0) / (2.0 * n)
        } else {
            b / n
        };
        let center = 1000.0 * ratio.powf(power);
        let lower = round12(center * ratio.powf(-1.0 / (2.0 * n)));
        let upper = round12(center * ratio.powf(1.0 / (2.0 * n)));
        if f_min.max(10.0) < upper && f_max > lower {
            lb.push(lower);
            ub.push(upper);
        }
    }
    (lb, ub)
}

/// Each band's lines on a grid of `n_lines` lines `df` apart, as half-open ranges in
/// line order: a line belongs to the band whose edges hold its bin centre, the rule
/// the law places a line by. Bands that hold no line are left out.
pub fn band_line_ranges(df: f64, oct_order: usize, n_lines: usize) -> Vec<(usize, usize)> {
    let band_of = Bands::new(df, oct_order, n_lines).band_of;
    let mut ranges = Vec::new();
    let mut k = 0;
    while k < n_lines {
        let Some(b) = band_of[k] else {
            k += 1;
            continue;
        };
        let first = k;
        while k < n_lines && band_of[k] == Some(b) {
            k += 1;
        }
        ranges.push((first, k));
    }
    ranges
}

/// The analysis grid against the bands.
pub(crate) struct Bands {
    pub lb: Vec<f64>,
    pub ub: Vec<f64>,
    pub widths: Vec<f64>,
    /// For each line, the band whose edges hold its bin centre.
    pub band_of: Vec<Option<usize>>,
    /// For each band, the lines whose df-wide bin overlaps it and the overlap in Hz.
    pub overlap: Vec<Vec<(usize, f64)>>,
    /// Each line's bin edges, for the overlap of a chosen line.
    pub df: f64,
}

impl Bands {
    pub fn new(df: f64, oct_order: usize, n_lines: usize) -> Self {
        let f = |k: usize| df * k as f64;
        let (lb, ub) = nth_octave_edges(f(1), f(n_lines - 1), oct_order);
        let widths: Vec<f64> = lb.iter().zip(&ub).map(|(l, u)| u - l).collect();
        let band_of = (0..n_lines)
            .map(|k| {
                let above = lb.partition_point(|&l| l <= f(k));
                (above > 0 && f(k) < ub[above - 1]).then(|| above - 1)
            })
            .collect();
        let overlap = lb
            .iter()
            .zip(&ub)
            .map(|(&l, &u)| {
                (0..n_lines)
                    .filter_map(|k| {
                        let hz = bin_overlap(f(k), df, l, u);
                        (hz > 0.0).then_some((k, hz))
                    })
                    .collect()
            })
            .collect();
        Bands {
            lb,
            ub,
            widths,
            band_of,
            overlap,
            df,
        }
    }

    pub fn n_bands(&self) -> usize {
        self.lb.len()
    }

    /// Hz of a line's bin inside band `b`.
    pub fn line_overlap(&self, b: usize, line: usize) -> f64 {
        bin_overlap(self.df * line as f64, self.df, self.lb[b], self.ub[b])
    }
}

fn bin_overlap(f: f64, df: f64, lb: f64, ub: f64) -> f64 {
    ((f + 0.5 * df).min(ub) - (f - 0.5 * df).max(lb)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_orders_put_a_band_edge_on_1khz_and_tile_the_axis() {
        let (lb, ub) = nth_octave_edges(1.0, 8192.0, 6);
        assert!(lb.iter().any(|&l| (l - 1000.0).abs() < 1e-9));
        for w in 0..lb.len() - 1 {
            assert!((ub[w] - lb[w + 1]).abs() < 1e-9);
            assert!((ub[w] / lb[w] - 10f64.powf(0.05)).abs() < 1e-9);
        }
        assert!(ub[0] > 10.0 && lb[0] <= 10.0);
        assert!(*ub.last().unwrap() > 8192.0);
    }

    #[test]
    fn band_line_ranges_hold_each_line_of_a_band_once_and_in_order() {
        let (df, lines) = (2.0, 2049);
        let ranges = band_line_ranges(df, 6, lines);
        let bands = Bands::new(df, 6, lines);
        let mut placed = 0;
        for (w, &(first, end)) in ranges.iter().enumerate() {
            assert!(first < end && end <= lines);
            if w > 0 {
                assert!(ranges[w - 1].1 <= first);
            }
            let b = bands.band_of[first].expect("a line of a band");
            assert!(
                bands.band_of[first..end].iter().all(|&of| of == Some(b)),
                "band {b}"
            );
            assert!((first as f64) * df >= bands.lb[b] && ((end - 1) as f64) * df < bands.ub[b]);
            placed += end - first;
        }
        assert_eq!(placed, bands.band_of.iter().flatten().count());
        // 1 kHz is a band edge at an even order, so line 500 starts a band.
        assert!(ranges.iter().any(|&(first, _)| first == 500));
    }

    #[test]
    fn odd_orders_centre_a_band_on_1khz() {
        let (lb, ub) = nth_octave_edges(1.0, 8192.0, 3);
        let i = lb.iter().position(|&l| l > 1000.0).unwrap() - 1;
        assert!(((lb[i] * ub[i]).sqrt() - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn the_overlaps_of_a_fully_covered_band_add_to_its_width() {
        let bands = Bands::new(1.0, 6, 8193);
        for b in 0..bands.n_bands() {
            if bands.ub[b] < 8000.0 {
                let total: f64 = bands.overlap[b].iter().map(|x| x.1).sum();
                assert!((total - bands.widths[b]).abs() < 1e-9, "band {b}");
            }
        }
    }
}
