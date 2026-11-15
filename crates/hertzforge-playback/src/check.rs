//! The file's checks before arming (D3), as one streaming pass over `drive.bin`.
//!
//! One pass reads the samples in pieces, so a file of any length is checked in
//! memory that does not grow with it: the SHA-256 of `drive.bin` against its sidecar
//! and against the setup's pinned value, every value finite, and per channel its
//! peak, its first and last samples, and its velocity and displacement taken as the
//! drive were an acceleration, integrated with the PURE trapezoidal integrator and no
//! high-pass, the same integration as the shock compensation gate
//! (`hertzforge_dsp::shock::pulse_kinematic_series`, held to it by a test). Each
//! channel keeps a few hundred points of envelope of the drive, the velocity and the
//! displacement for a preview.
//!
//! The ends pass when, per channel:
//! - the first and last samples are within [`ENDS_FRACTION`] of the channel's peak;
//! - the velocity's end value is within [`VELOCITY_FRACTION`] of its peak magnitude;
//! - the displacement's end value is within [`DISPLACEMENT_FRACTION`] of its peak
//!   magnitude.
//!
//! An all-zero channel passes, with a note. The tolerances are fixed.
//!
//! The check knows only the drive signal. It cannot know the table's motion without a
//! model of the shaker and its amplifier, so stroke is not checked.

use crate::file::{hex, DriveFile, FileError};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Read;

/// The first and last samples, as a fraction of the channel's peak.
pub const ENDS_FRACTION: f64 = 0.005;
/// The velocity's end value, as a fraction of its peak magnitude.
pub const VELOCITY_FRACTION: f64 = 0.01;
/// The displacement's end value, as a fraction of its peak magnitude.
pub const DISPLACEMENT_FRACTION: f64 = 0.01;
/// Buckets in each envelope: each keeps its minimum and maximum, so an envelope is
/// twice this many points.
pub const ENVELOPE_BUCKETS: usize = 200;
/// Frames read per piece.
const PIECE_FRAMES: usize = 16_384;

/// The pure trapezoidal double integrator, one sample at a time: velocity and
/// displacement from zero, no high-pass. `O(1)` memory.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PureIntegrator {
    dt: f64,
    started: bool,
    prev_a: f64,
    pub velocity: f64,
    pub displacement: f64,
}

impl PureIntegrator {
    pub fn new(fs_hz: f64) -> PureIntegrator {
        PureIntegrator {
            dt: 1.0 / fs_hz,
            ..PureIntegrator::default()
        }
    }

    /// Take the next sample; the velocity and displacement are then at it.
    pub fn push(&mut self, a: f64) {
        if self.started {
            let v = self.velocity + 0.5 * (self.prev_a + a) * self.dt;
            self.displacement += 0.5 * (self.velocity + v) * self.dt;
            self.velocity = v;
        }
        self.started = true;
        self.prev_a = a;
    }
}

/// A decimated envelope: each bucket's minimum and maximum.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    /// Samples per bucket.
    pub bucket: u64,
    pub min: Vec<f64>,
    pub max: Vec<f64>,
    filled: u64,
}

impl Envelope {
    fn new(samples: u64) -> Envelope {
        let bucket = samples.div_ceil(ENVELOPE_BUCKETS as u64).max(1);
        Envelope {
            bucket,
            min: Vec::with_capacity(ENVELOPE_BUCKETS),
            max: Vec::with_capacity(ENVELOPE_BUCKETS),
            filled: 0,
        }
    }

    fn push(&mut self, x: f64) {
        if self.filled == 0 {
            self.min.push(x);
            self.max.push(x);
        } else if let (Some(lo), Some(hi)) = (self.min.last_mut(), self.max.last_mut()) {
            *lo = lo.min(x);
            *hi = hi.max(x);
        }
        self.filled += 1;
        if self.filled == self.bucket {
            self.filled = 0;
        }
    }
}

/// Why a channel's ends fail, each with its numbers.
#[derive(Debug, Clone, PartialEq)]
pub enum EndFault {
    Start { value: f64, allowed: f64 },
    End { value: f64, allowed: f64 },
    Velocity { value: f64, allowed: f64 },
    Displacement { value: f64, allowed: f64 },
}

impl EndFault {
    pub fn reason(&self) -> &'static str {
        match self {
            EndFault::Start { .. } => "start_not_zero",
            EndFault::End { .. } => "end_not_zero",
            EndFault::Velocity { .. } => "velocity_not_zero",
            EndFault::Displacement { .. } => "displacement_not_zero",
        }
    }

    fn words(&self) -> String {
        match self {
            EndFault::Start { value, allowed } => {
                format!("starts at {value:.6} V, more than {allowed:.6} V from zero")
            }
            EndFault::End { value, allowed } => {
                format!("ends at {value:.6} V, more than {allowed:.6} V from zero")
            }
            EndFault::Velocity { value, allowed } => format!(
                "ends with a velocity of {value:.6e} V s, more than {allowed:.6e} V s from zero"
            ),
            EndFault::Displacement { value, allowed } => format!(
                "ends with a displacement of {value:.6e} V s², more than {allowed:.6e} V s² \
                 from zero"
            ),
        }
    }

    fn to_json(&self) -> Value {
        let (value, allowed) = match self {
            EndFault::Start { value, allowed }
            | EndFault::End { value, allowed }
            | EndFault::Velocity { value, allowed }
            | EndFault::Displacement { value, allowed } => (*value, *allowed),
        };
        json!({"reason": self.reason(), "value": value, "allowed": allowed,
               "text": self.words()})
    }
}

/// One channel's numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelCheck {
    pub channel: usize,
    pub name: String,
    pub peak_v: f64,
    pub start_v: f64,
    pub end_v: f64,
    pub velocity_end: f64,
    pub velocity_peak: f64,
    pub displacement_end: f64,
    pub displacement_peak: f64,
    pub faults: Vec<EndFault>,
    /// A note for an all-zero channel.
    pub note: Option<String>,
    pub drive: Envelope,
    pub velocity: Envelope,
    pub displacement: Envelope,
}

impl ChannelCheck {
    pub fn pass(&self) -> bool {
        self.faults.is_empty()
    }

    /// The channel's words for a refusal: its name and every fault with its numbers.
    pub fn words(&self) -> String {
        let faults: Vec<String> = self.faults.iter().map(EndFault::words).collect();
        format!(
            "drive file channel {} ({}) {}",
            self.channel,
            self.name,
            faults.join("; ")
        )
    }

    /// The channel's numbers, without the envelopes.
    pub fn to_json(&self) -> Value {
        json!({
            "channel": self.channel,
            "name": self.name,
            "peak_v": self.peak_v,
            "start_v": self.start_v,
            "end_v": self.end_v,
            "velocity_v_s": {"end": self.velocity_end, "peak": self.velocity_peak},
            "displacement_v_s2": {"end": self.displacement_end, "peak": self.displacement_peak},
            "pass": self.pass(),
            "faults": self.faults.iter().map(EndFault::to_json).collect::<Vec<_>>(),
            "note": self.note,
        })
    }

    /// The envelopes for a preview: each as its bucket's start time and its minimum and
    /// maximum.
    pub fn envelopes_json(&self, rate_hz: f64) -> Value {
        let env = |e: &Envelope| {
            let t: Vec<f64> = (0..e.min.len())
                .map(|k| (k as u64 * e.bucket) as f64 / rate_hz)
                .collect();
            json!({"t_s": t, "min": e.min, "max": e.max})
        };
        json!({"channel": self.channel, "drive_v": env(&self.drive),
               "velocity_v_s": env(&self.velocity),
               "displacement_v_s2": env(&self.displacement)})
    }
}

/// The file's check: the hash it was taken on and each channel's numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct FileCheck {
    pub sha256: String,
    pub rate_hz: f64,
    pub samples: u64,
    pub channels: Vec<ChannelCheck>,
}

impl FileCheck {
    /// Whether every channel's ends pass.
    pub fn ends_pass(&self) -> bool {
        self.channels.iter().all(ChannelCheck::pass)
    }

    pub fn tolerances_json() -> Value {
        json!({"ends_fraction_of_peak": ENDS_FRACTION,
               "velocity_fraction_of_peak": VELOCITY_FRACTION,
               "displacement_fraction_of_peak": DISPLACEMENT_FRACTION,
               "integrator": "pure trapezoidal, no high-pass, the drive taken as an \
                              acceleration"})
    }
}

struct Acc {
    peak: f64,
    first: f64,
    last: f64,
    integ: PureIntegrator,
    v_peak: f64,
    d_peak: f64,
    env: [Envelope; 3],
}

/// Check `file` in one streaming pass. `pinned` is the SHA-256 the setup names, when
/// it names one. Refused when the hash differs from the sidecar's or the pin
/// (`drive_hash_mismatch`, `drive_pin_mismatch`), a value is not finite
/// (`drive_not_finite`), or the file cannot be read; otherwise the numbers, whether or
/// not the ends pass.
pub fn check_file(file: &DriveFile, pinned: Option<&str>) -> Result<FileCheck, FileError> {
    if let Some(p) = pinned {
        if !p.eq_ignore_ascii_case(&file.sha256) {
            return Err(FileError::new(
                "drive_pin_mismatch",
                format!(
                    "The drive file's sidecar states SHA-256 {}, not the {p} the setup names.",
                    file.sha256
                ),
                json!({"sidecar": file.sha256, "pinned": p}),
            ));
        }
    }
    let width = file.channels.len();
    let path = file.bin_path();
    let mut reader = file.samples_reader()?;
    let mut hasher = Sha256::new();
    hasher.update(crate::file::MAGIC);
    let mut acc: Vec<Acc> = (0..width)
        .map(|_| Acc {
            peak: 0.0,
            first: 0.0,
            last: 0.0,
            integ: PureIntegrator::new(file.rate_hz),
            v_peak: 0.0,
            d_peak: 0.0,
            env: [
                Envelope::new(file.samples),
                Envelope::new(file.samples),
                Envelope::new(file.samples),
            ],
        })
        .collect();
    let mut buf = vec![0u8; PIECE_FRAMES * width * 4];
    let mut frame: u64 = 0;
    while frame < file.samples {
        let frames = ((file.samples - frame) as usize).min(PIECE_FRAMES);
        let bytes = &mut buf[..frames * width * 4];
        reader.read_exact(bytes).map_err(|e| {
            FileError::new(
                "drive_unreadable",
                format!(
                    "drive.bin could not be read at sample {frame}: {} ({e}).",
                    path.display()
                ),
                json!({"path": path.display().to_string(), "sample": frame,
                       "reason": e.to_string()}),
            )
        })?;
        hasher.update(&*bytes);
        for (k, s) in bytes.chunks_exact(4).enumerate() {
            let x = f64::from(f32::from_le_bytes([s[0], s[1], s[2], s[3]]));
            let ch = k % width;
            let at = frame + (k / width) as u64;
            if !x.is_finite() {
                return Err(FileError::new(
                    "drive_not_finite",
                    format!(
                        "Drive file channel {ch} ({}) holds a value that is not a number at \
                         sample {at}.",
                        file.channels[ch].name
                    ),
                    json!({"channel": ch, "sample": at}),
                ));
            }
            let a = &mut acc[ch];
            if at == 0 {
                a.first = x;
            }
            a.last = x;
            a.peak = a.peak.max(x.abs());
            a.integ.push(x);
            a.v_peak = a.v_peak.max(a.integ.velocity.abs());
            a.d_peak = a.d_peak.max(a.integ.displacement.abs());
            a.env[0].push(x);
            a.env[1].push(a.integ.velocity);
            a.env[2].push(a.integ.displacement);
        }
        frame += frames as u64;
    }
    let sha = hex(&hasher.finalize());
    // Bytes past the samples were refused by the layout check; a file that grew since
    // is caught here.
    let mut tail = [0u8; 1];
    if reader.read(&mut tail).unwrap_or(0) != 0 {
        return Err(FileError::new(
            "drive_size",
            "drive.bin grew while it was being checked.",
            json!({"expected": file.expected_bytes()}),
        ));
    }
    if sha != file.sha256 {
        return Err(FileError::new(
            "drive_hash_mismatch",
            format!(
                "drive.bin's SHA-256 is {sha}; its sidecar states {}. The samples are not the \
                 ones the sidecar describes.",
                file.sha256
            ),
            json!({"computed": sha, "sidecar": file.sha256}),
        ));
    }
    let channels = acc
        .into_iter()
        .enumerate()
        .map(|(ch, a)| {
            let [drive, velocity, displacement] = a.env;
            let mut faults = Vec::new();
            let mut note = None;
            if a.peak == 0.0 {
                note = Some("the channel is zero throughout and drives nothing".to_owned());
            } else {
                let allowed = ENDS_FRACTION * a.peak;
                if a.first.abs() > allowed {
                    faults.push(EndFault::Start {
                        value: a.first,
                        allowed,
                    });
                }
                if a.last.abs() > allowed {
                    faults.push(EndFault::End {
                        value: a.last,
                        allowed,
                    });
                }
                let allowed = VELOCITY_FRACTION * a.v_peak;
                if a.integ.velocity.abs() > allowed {
                    faults.push(EndFault::Velocity {
                        value: a.integ.velocity,
                        allowed,
                    });
                }
                let allowed = DISPLACEMENT_FRACTION * a.d_peak;
                if a.integ.displacement.abs() > allowed {
                    faults.push(EndFault::Displacement {
                        value: a.integ.displacement,
                        allowed,
                    });
                }
            }
            ChannelCheck {
                channel: ch,
                name: file.channels[ch].name.clone(),
                peak_v: a.peak,
                start_v: a.first,
                end_v: a.last,
                velocity_end: a.integ.velocity,
                velocity_peak: a.v_peak,
                displacement_end: a.integ.displacement,
                displacement_peak: a.d_peak,
                faults,
                note,
                drive,
                velocity,
                displacement,
            }
        })
        .collect();
    Ok(FileCheck {
        sha256: sha,
        rate_hz: file.rate_hz,
        samples: file.samples,
        channels,
    })
}
