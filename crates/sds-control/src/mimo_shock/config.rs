//! A MIMO Shock configuration (schema/PROTOCOL.md, "MIMO Shock Configuration"):
//! parsing (shape and types, refused `bad_params`), the content checks (items,
//! refused `config_invalid`), the derived values, and the engine's configuration
//! and target spec built from it.

use crate::protocol::{num, MethodError};
use crate::shock::config::{self as sc, Obj, WirePulse};
use crate::srs::config::{self as srsc, WireBands, WireSynthesis};
use crate::table::{At, Item, Report};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_mimo_shock::{
    ClassicalControl, DriveChannel, GuardPlan, HoldTiming, IdentificationPlan, LadderPlan,
    LimitChannel, MimoShockConfig, SrsSpec, Target, TargetKind, TargetSpec, Timing, VerdictMode,
    WaveformTarget, MAX_CONTROLS, MAX_DRIVES, MAX_RECORD_LEN, MIN_RECORD_LEN,
};
use hertzforge_shock_common::law_guard::LawGuards;
use hertzforge_shock_common::ShockRunMode;
use hertzforge_shock_host::tolerance::ToleranceStandard;
use hertzforge_shock_host::Polarity;
use serde_json::{json, Map, Value};

/// The rate asked when none is given, Hz.
pub const DEFAULT_FS_HZ: f64 = 51_200.0;
/// The default level ladder, dB: a half step before full level.
pub const DEFAULT_LEVELS_DB: [f64; 6] = [-12.0, -9.0, -6.0, -3.0, -1.5, 0.0];
/// Where every target starts in its record, s.
pub const DEFAULT_LEAD_S: f64 = 0.05;
/// The shortest record the default length picks.
pub const DEFAULT_MIN_RECORD: usize = 1024;
/// The simulator's standard shock plants.
pub const PLANTS: [&str; 4] = [
    "shock-linear-4x4",
    "shock-jointed-4x4-moderate",
    "shock-jointed-4x4-severe",
    "shock-jointed-6x3",
];

/// One drive as sent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WireDrive {
    pub channel: u32,
    pub ceiling_vpk: f64,
    pub abort_vpk: f64,
}

/// One SRS control as sent.
#[derive(Debug, Clone, PartialEq)]
pub struct WireSrsControl {
    pub synthesis: WireSynthesis,
    pub inverted: bool,
}

/// The target as sent.
#[derive(Debug, Clone, PartialEq)]
pub enum WireTargetSet {
    Waveform {
        standard: ToleranceStandard,
        pulses: Vec<WirePulse>,
    },
    Srs {
        bands: WireBands,
        tolerance_db: (f64, f64),
        controls: Vec<WireSrsControl>,
    },
}

impl WireTargetSet {
    pub fn kind(&self) -> TargetKind {
        match self {
            WireTargetSet::Waveform { .. } => TargetKind::Waveform,
            WireTargetSet::Srs { .. } => TargetKind::Srs,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            WireTargetSet::Waveform { pulses, .. } => pulses.len(),
            WireTargetSet::Srs { controls, .. } => controls.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A parsed configuration, every default filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct WireMs {
    pub raw: Value,
    pub title: String,
    pub controls: Vec<u32>,
    pub drives: Vec<WireDrive>,
    pub limits: Vec<(u32, f64)>,
    pub target: WireTargetSet,
    pub timing: Timing,
    pub verdict: VerdictMode,
    pub fs_hz: f64,
    pub record_len: Option<usize>,
    pub band_hz: Option<(f64, f64)>,
    pub lead_s: f64,
    pub mode: ShockRunMode,
    pub levels_db: Vec<f64>,
    pub max_pulses_per_level: u64,
    pub full_level_pulses: u64,
    pub cooling_s: f64,
    pub settling_s: f64,
    pub compute_s: f64,
    pub drive_vrms: f64,
    pub frames: Option<u64>,
    pub coherence_floor: f64,
    pub review: bool,
    pub law_id: String,
    pub law_params: Map<String, Value>,
    pub law_budget: f64,
    pub law_misses: u64,
    pub max_pulses: u64,
    pub max_growth_db: f64,
    pub max_band_growth_db: f64,
    pub divergence_pulses: u64,
    pub divergence_drive_ratio: f64,
    pub first_drive_ratio: f64,
    pub abort_db: Option<f64>,
    pub scaled_aborts: bool,
    pub max_velocity_ips: Option<f64>,
    pub max_displacement_in: Option<f64>,
    pub plant: Option<String>,
    pub capture_auto_start: bool,
    pub run_name: String,
}

fn bad(field: &str, message: impl Into<String>) -> MethodError {
    MethodError::bad_params(field, message.into())
}

fn channel_of(v: &Value, path: &str) -> Result<u32, MethodError> {
    let o = Obj::of(v, path)?;
    let c = o
        .opt_count("channel")?
        .ok_or_else(|| bad(&o.at("channel"), "a channel is named by its table index"))?;
    u32::try_from(c).map_err(|_| bad(&o.at("channel"), "a table index"))
}

fn list<'a>(o: &Obj<'a>, k: &str) -> Result<&'a Vec<Value>, MethodError> {
    o.array(k)?
        .ok_or_else(|| bad(&o.at(k), format!("\"{}\" is a list", o.at(k))))
}

/// Parse `params.config`: its shape and types. Content is checked by [`WireMs::check`].
pub fn parse(config: &Value) -> Result<WireMs, MethodError> {
    let c = Obj::of(config, "config")?;
    let title = c.opt_str("title")?.unwrap_or("").to_owned();

    let ch = c.obj("channels")?;
    let mut controls = Vec::new();
    for (i, v) in list(&ch, "controls")?.iter().enumerate() {
        controls.push(channel_of(v, &format!("{}[{i}]", ch.at("controls")))?);
    }
    let mut drives = Vec::new();
    for (i, v) in list(&ch, "drives")?.iter().enumerate() {
        let at = format!("{}[{i}]", ch.at("drives"));
        let o = Obj::of(v, &at)?;
        drives.push(WireDrive {
            channel: channel_of(v, &at)?,
            ceiling_vpk: o.f64("ceiling_vpk")?,
            abort_vpk: o.f64("abort_vpk")?,
        });
    }
    let mut limits = Vec::new();
    if let Some(a) = ch.array("limits")? {
        for (i, v) in a.iter().enumerate() {
            let at = format!("{}[{i}]", ch.at("limits"));
            let o = Obj::of(v, &at)?;
            limits.push((channel_of(v, &at)?, o.f64("abort_peak")?));
        }
    }

    let mut fs_hz = DEFAULT_FS_HZ;
    let mut record_len = None;
    let mut band_hz = None;
    let mut lead_s = DEFAULT_LEAD_S;
    if let Some(a) = c.opt_obj("acquisition")? {
        fs_hz = a.f64_or("fs_hz", fs_hz)?;
        record_len = a.opt_count("record_len")?.map(|n| n as usize);
        band_hz = a.band("band_hz")?;
        lead_s = a.f64_or("lead_s", lead_s)?;
    }

    let t = c.obj("target")?;
    let target = match t.opt_str("kind")? {
        Some("waveform") => {
            let standard = t.choice(
                "standard",
                &[
                    ("mil_std_810_516", ToleranceStandard::Mil810_516),
                    ("iec_60068_2_27", ToleranceStandard::Iec60068_2_27),
                ],
                Some(ToleranceStandard::Mil810_516),
            )?;
            let mut pulses = Vec::new();
            for (i, v) in list(&t, "pulses")?.iter().enumerate() {
                pulses.push(sc::parse_pulse(v, &format!("{}[{i}]", t.at("pulses")))?);
            }
            WireTargetSet::Waveform { standard, pulses }
        }
        Some("srs") => {
            let bands = srsc::parse_bands(t.require("bands")?, &t.at("bands"))?;
            let tolerance_db = t.band("tolerance_db")?.unwrap_or((-3.0, 3.0));
            let mut out = Vec::new();
            for (i, v) in list(&t, "controls")?.iter().enumerate() {
                let at = format!("{}[{i}]", t.at("controls"));
                let o = Obj::of(v, &at)?;
                let target = srsc::parse_target(o.require("target")?, &o.at("target"))?;
                let synthesis = srsc::parse_synthesis(
                    target,
                    bands,
                    o.m.get("synthesis"),
                    &o.at("synthesis"),
                    fs_hz,
                )?;
                let polarity = o.choice(
                    "polarity",
                    &[
                        ("positive", Polarity::Positive),
                        ("negative", Polarity::Negative),
                    ],
                    Some(Polarity::Positive),
                )?;
                out.push(WireSrsControl {
                    synthesis,
                    inverted: polarity == Polarity::Negative,
                });
            }
            WireTargetSet::Srs {
                bands,
                tolerance_db,
                controls: out,
            }
        }
        _ => {
            return Err(bad(
                &t.at("kind"),
                format!("\"{}\" is \"waveform\" or \"srs\"", t.at("kind")),
            ))
        }
    };

    let mut timing = Timing::Free { window_s: 0.002 };
    if let Some(o) = c.opt_obj("timing")? {
        let window_s = o.f64_or("window_s", 0.002)?;
        timing = match o.opt_str("mode")?.unwrap_or("free") {
            "free" => Timing::Free { window_s },
            "hold" => {
                let a = o.array("delays_s")?.ok_or_else(|| {
                    bad(
                        &o.at("delays_s"),
                        "a held timing gives one delay per control",
                    )
                })?;
                let mut delays_s = Vec::new();
                for (i, v) in a.iter().enumerate() {
                    delays_s.push(v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| {
                        bad(&format!("{}[{i}]", o.at("delays_s")), "a delay in seconds")
                    })?);
                }
                Timing::Hold(HoldTiming { delays_s, window_s })
            }
            other => {
                return Err(bad(
                    &o.at("mode"),
                    format!(
                        "\"{}\" is \"free\" or \"hold\", not \"{other}\"",
                        o.at("mode")
                    ),
                ))
            }
        };
    }
    let verdict = c.choice(
        "verdict",
        &[("all", VerdictMode::All), ("global", VerdictMode::Global)],
        Some(VerdictMode::All),
    )?;

    let mut mode = ShockRunMode::Auto;
    let mut levels_db = DEFAULT_LEVELS_DB.to_vec();
    let mut max_pulses_per_level = 3;
    let mut full_level_pulses = 3;
    if let Some(s) = c.opt_obj("schedule")? {
        mode = s.choice(
            "mode",
            &[
                ("auto", ShockRunMode::Auto),
                ("manual", ShockRunMode::Manual),
            ],
            Some(ShockRunMode::Auto),
        )?;
        if let Some(a) = s.array("levels_db")? {
            levels_db = Vec::with_capacity(a.len());
            for (i, v) in a.iter().enumerate() {
                levels_db.push(
                    v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| {
                        bad(&format!("{}[{i}]", s.at("levels_db")), "a level in dB")
                    })?,
                );
            }
        }
        max_pulses_per_level = s
            .opt_count("max_pulses_per_level")?
            .unwrap_or(max_pulses_per_level);
        full_level_pulses = s
            .opt_count("full_level_pulses")?
            .unwrap_or(full_level_pulses);
    }
    let (mut cooling_s, mut settling_s, mut compute_s) = (1.0, 0.5, 0.25);
    if let Some(s) = c.opt_obj("spacing")? {
        cooling_s = s.f64_or("cooling_s", cooling_s)?;
        settling_s = s.f64_or("settling_s", settling_s)?;
        compute_s = s.f64_or("compute_s", compute_s)?;
    }
    let (mut drive_vrms, mut frames, mut coherence_floor, mut review) = (0.05, None, 0.5, true);
    if let Some(s) = c.opt_obj("identification")? {
        drive_vrms = s.f64_or("drive_vrms", drive_vrms)?;
        frames = s.opt_count("frames")?;
        coherence_floor = s.f64_or("coherence_floor", coherence_floor)?;
        review = s.opt_bool("review")?.unwrap_or(review);
    }
    let d = LawGuards::default();
    let default_law = match target.kind() {
        TargetKind::Waveform => "inverse",
        TargetKind::Srs => "shape",
    };
    let mut w = WireMs {
        raw: config.clone(),
        title,
        controls,
        drives,
        limits,
        target,
        timing,
        verdict,
        fs_hz,
        record_len,
        band_hz,
        lead_s,
        mode,
        levels_db,
        max_pulses_per_level,
        full_level_pulses,
        cooling_s,
        settling_s,
        compute_s,
        drive_vrms,
        frames,
        coherence_floor,
        review,
        law_id: default_law.into(),
        law_params: Map::new(),
        law_budget: d.budget_fraction,
        law_misses: u64::from(d.max_misses),
        max_pulses: d.max_pulses,
        max_growth_db: 6.0,
        max_band_growth_db: 10.0,
        divergence_pulses: 3,
        divergence_drive_ratio: 1.3,
        first_drive_ratio: 2.0,
        abort_db: None,
        scaled_aborts: true,
        max_velocity_ips: None,
        max_displacement_in: None,
        plant: None,
        capture_auto_start: false,
        run_name: "mimo_shock".into(),
    };
    if let Some(s) = c.opt_obj("control")? {
        if let Some(l) = s.opt_obj("law")? {
            if let Some(id) = l.opt_str("id")? {
                w.law_id = id.to_owned();
            }
            match l.m.get("params") {
                None | Some(Value::Null) => {}
                Some(Value::Object(m)) => w.law_params = m.clone(),
                Some(_) => {
                    return Err(bad(
                        &l.at("params"),
                        "\"params\" is an object of parameter values by key",
                    ))
                }
            }
        }
        w.law_budget = s.f64_or("law_budget", w.law_budget)?;
        w.law_misses = s.opt_count("law_misses")?.unwrap_or(w.law_misses);
        w.max_pulses = s.opt_count("max_pulses")?.unwrap_or(w.max_pulses);
        w.max_growth_db = s.f64_or("max_growth_db", w.max_growth_db)?;
        w.max_band_growth_db = s.f64_or("max_band_growth_db", w.max_band_growth_db)?;
        w.divergence_pulses = s
            .opt_count("divergence_pulses")?
            .unwrap_or(w.divergence_pulses);
        w.divergence_drive_ratio = s.f64_or("divergence_drive_ratio", w.divergence_drive_ratio)?;
        w.first_drive_ratio = s.f64_or("first_drive_ratio", w.first_drive_ratio)?;
        w.abort_db = s.opt_f64("abort_db")?;
        w.scaled_aborts = s.opt_bool("scaled_aborts")?.unwrap_or(true);
        w.max_velocity_ips = s.opt_f64("max_velocity_ips")?;
        w.max_displacement_in = s.opt_f64("max_displacement_in")?;
    }
    if let Some(s) = c.opt_obj("simulator")? {
        w.plant = s.opt_str("plant")?.map(str::to_owned);
    }
    if let Some(s) = c.opt_obj("capture")? {
        w.capture_auto_start = s.opt_bool("auto_start")?.unwrap_or(false);
        if let Some(n) = s.opt_str("run_name")? {
            w.run_name = n.to_owned();
        }
    }
    if let Some(m) = c.array("metadata")? {
        for (i, v) in m.iter().enumerate() {
            Obj::of(v, &format!("config.metadata[{i}]"))?;
        }
    }
    Ok(w)
}

/// A law as a configuration's checks see it.
#[derive(Debug, Clone, PartialEq)]
pub struct LawFacts {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Empty for a built-in law.
    pub sha256: String,
    pub builtin: bool,
    /// `Err` with the test type it is for when it is not a MIMO Shock law.
    pub world: Result<(), &'static str>,
    pub targets: Vec<TargetKind>,
    pub params: Vec<hertzforge_law_host::ParamSpec>,
}

/// Everything a configuration's checks are made against.
pub struct CheckContext<'a> {
    pub table: &'a ChannelTable,
    pub law: &'a dyn Fn(&str) -> Option<LawFacts>,
    /// The controller timeout, s: one record must be shorter.
    pub timeout_s: f64,
}

/// The derived values `configure` returns.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub fs_hz: f64,
    pub record_len: usize,
    /// Each control: its channel and its full-level target's peak (EU).
    pub controls: Vec<(u32, f64)>,
    pub spacing_s: f64,
    pub call_budget_s: f64,
    /// The targets at the asked rate, for the configuration's views.
    pub target: Target,
}

impl Derived {
    pub fn to_json(&self) -> Value {
        json!({
            "grid": {
                "fs_hz": num(self.fs_hz),
                "record_len": self.record_len,
                "lines": self.record_len / 2 + 1,
                "actual": false,
            },
            "controls": self.controls.iter().map(|(c, pk)| json!({
                "channel": c, "peak": num(crate::shock::view::sig(*pk, 4)),
            })).collect::<Vec<_>>(),
            "spacing_s": num(sc::round(self.spacing_s, 4)),
            "call_budget_s": num(sc::round(self.call_budget_s, 4)),
        })
    }
}

fn err(r: &mut Report, reason: &'static str, field: &str, message: impl Into<String>) {
    r.errors.push(Item::field(reason, field, message));
}

fn range(r: &mut Report, field: &str, x: f64, ok: bool, words: &str) {
    if !ok {
        err(
            r,
            "out_of_range",
            field,
            format!("{} is out of range: {words}.", num(x)),
        );
    }
}

impl WireMs {
    /// The time between pulse starts, s.
    pub fn spacing_s(&self) -> f64 {
        self.cooling_s.max(self.settling_s).max(self.compute_s)
    }

    /// The correction band, Hz.
    pub fn band(&self, fs_hz: f64) -> (f64, f64) {
        self.band_hz.unwrap_or((5.0, 5000f64.min(0.4 * fs_hz)))
    }

    /// The identification frames.
    pub fn frames(&self) -> u64 {
        self.frames
            .unwrap_or_else(|| 64u64.max(10 * self.drives.len() as u64))
    }

    /// The target spec per control.
    pub fn target_spec(&self) -> TargetSpec {
        match &self.target {
            WireTargetSet::Waveform { standard, pulses } => TargetSpec::Classical {
                controls: pulses
                    .iter()
                    .map(|p| ClassicalControl {
                        pulse: p.spec(),
                        inverted: p.polarity == Polarity::Negative,
                    })
                    .collect(),
                standard: *standard,
                lead_s: self.lead_s,
            },
            WireTargetSet::Srs {
                bands,
                tolerance_db,
                controls,
            } => {
                let natural_hz = bands.centres();
                let first = controls.first().map(|c| &c.synthesis);
                TargetSpec::Srs {
                    spec: SrsSpec {
                        natural_hz,
                        q: bands.q,
                        points: controls
                            .iter()
                            .map(|c| crate::srs::preview::target_on_grid(&c.synthesis))
                            .collect(),
                        tol_lo_db: tolerance_db.0,
                        tol_hi_db: tolerance_db.1,
                        tables: controls
                            .iter()
                            .map(|c| {
                                crate::srs::preview::seed_table(&c.synthesis).unwrap_or_else(|| {
                                    hertzforge_srs::SynthesisEngine::Wavsyn(
                                        hertzforge_srs::table::WaveletTable::new(),
                                    )
                                })
                            })
                            .collect(),
                        synthesis: first.map(|s| s.opts()).unwrap_or_default(),
                        inverted: controls.iter().map(|c| c.inverted).collect(),
                    },
                    lead_s: self.lead_s,
                }
            }
        }
    }

    /// The record length: as given, or the shortest power of two (at least
    /// [`DEFAULT_MIN_RECORD`]) whose first three quarters hold every target.
    pub fn record_len_at(&self, spec: &TargetSpec, fs_hz: f64) -> Option<usize> {
        if let Some(n) = self.record_len {
            return Some(n);
        }
        let mut n = DEFAULT_MIN_RECORD;
        while n <= MAX_RECORD_LEN {
            if spec.build(fs_hz, n).is_ok() {
                return Some(n);
            }
            n *= 2;
        }
        None
    }

    /// The engine's configuration (its target a placeholder the spec replaces).
    pub fn engine_config(&self, record_len: usize) -> MimoShockConfig {
        let (lo, hi) = self.band(self.fs_hz);
        let nc = self.controls.len();
        MimoShockConfig {
            fs_hz: self.fs_hz,
            record_len,
            band_lo_hz: lo,
            band_hi_hz: hi,
            controls: self.controls.clone(),
            drives: self
                .drives
                .iter()
                .map(|d| DriveChannel {
                    channel: d.channel,
                    ceiling_v: d.ceiling_vpk,
                    abort_v: d.abort_vpk,
                })
                .collect(),
            limits: self
                .limits
                .iter()
                .map(|(c, a)| LimitChannel {
                    channel: *c,
                    abort_peak: *a,
                })
                .collect(),
            target: Target::Waveform(WaveformTarget {
                records: vec![0.0; nc * record_len],
            }),
            timing: self.timing.clone(),
            ladder: LadderPlan {
                levels_db: self.levels_db.clone(),
                max_pulses_per_level: self.max_pulses_per_level as u32,
                full_level_pulses: self.full_level_pulses as u32,
            },
            identification: IdentificationPlan {
                drive_vrms: self.drive_vrms,
                frames: self.frames() as u32,
                coherence_floor: self.coherence_floor,
                review: self.review,
            },
            guards: GuardPlan {
                max_growth_db: self.max_growth_db,
                max_band_growth_db: self.max_band_growth_db,
                divergence_pulses: self.divergence_pulses as u32,
                law: LawGuards {
                    budget_fraction: self.law_budget,
                    max_misses: self.law_misses as u32,
                    max_pulses: self.max_pulses,
                },
                divergence_drive_ratio: self.divergence_drive_ratio,
                control_abort_db: self.abort_db,
                first_drive_ratio: self.first_drive_ratio,
                max_velocity_ips: self.max_velocity_ips,
                max_displacement_in: self.max_displacement_in,
                verdict: self.verdict,
                scaled_aborts: self.scaled_aborts,
            },
            min_spacing_s: self.spacing_s(),
        }
    }

    /// The simulator's plant for this run: as named, or by size.
    pub fn sim_plant(&self) -> Option<&str> {
        match &self.plant {
            Some(p) => PLANTS.iter().copied().find(|x| x == p),
            None => match (self.controls.len(), self.drives.len()) {
                (4, 4) => Some("shock-linear-4x4"),
                (6, 3) => Some("shock-jointed-6x3"),
                _ => None,
            },
        }
    }

    /// Check the content against `ctx`: the items, and the derived values when
    /// there is no error.
    pub fn check(&self, ctx: &CheckContext<'_>) -> (Report, Option<Derived>) {
        let mut r = Report::default();
        let (nc, nd) = (self.controls.len(), self.drives.len());
        if nc == 0 || nc > MAX_CONTROLS {
            err(
                &mut r,
                "controls_out_of_range",
                "channels.controls",
                format!("{nc} controls; a run controls 1 to {MAX_CONTROLS}."),
            );
        }
        if nd == 0 || nd > MAX_DRIVES {
            err(
                &mut r,
                "drives_out_of_range",
                "channels.drives",
                format!("{nd} drives; a run commands 1 to {MAX_DRIVES}."),
            );
        }
        let mut seen: Vec<u32> = self.controls.clone();
        seen.extend(self.drives.iter().map(|d| d.channel));
        seen.extend(self.limits.iter().map(|l| l.0));
        let mut sorted = seen.clone();
        sorted.sort_unstable();
        for w in sorted.windows(2).filter(|w| w[0] == w[1]) {
            r.errors.push(Item::channel(
                "duplicate_channel",
                Some(w[0]),
                format!(
                    "Channel {} is named twice among the controls, drives and limits.",
                    w[0]
                ),
            ));
        }
        let row = |c: u32| {
            ctx.table
                .channels
                .iter()
                .find(|x| x.index == c && x.enabled)
        };
        for &c in &self.controls {
            if row(c).map(|x| x.role) != Some(ChannelRole::Control) {
                r.errors.push(Item::channel(
                    "control_unknown_channel",
                    Some(c),
                    format!("Control {c} is not an enabled control row of the table."),
                ));
            }
        }
        for d in &self.drives {
            if row(d.channel).map(|x| x.role) != Some(ChannelRole::Drive) {
                r.errors.push(Item::channel(
                    "drive_unknown_channel",
                    Some(d.channel),
                    format!(
                        "Drive {} is not an enabled drive row of the table.",
                        d.channel
                    ),
                ));
            }
        }
        let table_drives: Vec<u32> = ctx
            .table
            .channels
            .iter()
            .filter(|x| x.enabled && x.role == ChannelRole::Drive)
            .map(|x| x.index)
            .collect();
        let ours: Vec<u32> = self.drives.iter().map(|d| d.channel).collect();
        if table_drives != ours {
            for t in table_drives.iter().filter(|t| !ours.contains(t)) {
                r.errors.push(Item::channel(
                    "drive_unknown_channel",
                    Some(*t),
                    format!(
                        "The table's drive row {t} is not one of the run's drives; every \
                         enabled drive row is an output, so name it or disable it."
                    ),
                ));
            }
            if table_drives.len() == ours.len() && table_drives.iter().all(|t| ours.contains(t)) {
                r.errors.push(Item::field(
                    "drive_unknown_channel",
                    "channels.drives",
                    "The drives are not in the order of the table's drive rows.",
                ));
            }
        }
        for (c, a) in &self.limits {
            let ok = row(*c).is_some_and(|x| {
                matches!(x.role, ChannelRole::Monitor | ChannelRole::Control)
                    && !self.controls.contains(c)
            });
            if !ok {
                r.errors.push(Item::channel(
                    "limit_unknown_channel",
                    Some(*c),
                    format!("Limit {c} is not an enabled input row the run does not control."),
                ));
            }
            if !(a.is_finite() && *a > 0.0) {
                range(
                    &mut r,
                    "channels.limits.abort_peak",
                    *a,
                    false,
                    "it is above 0",
                );
            }
        }
        for (i, d) in self.drives.iter().enumerate() {
            let at = format!("channels.drives[{i}]");
            range(
                &mut r,
                &format!("{at}.ceiling_vpk"),
                d.ceiling_vpk,
                d.ceiling_vpk > 0.0 && d.ceiling_vpk <= sc::MAX_DRIVE_V,
                "a ceiling is above 0 and at most 10 V",
            );
            range(
                &mut r,
                &format!("{at}.abort_vpk"),
                d.abort_vpk,
                d.abort_vpk > d.ceiling_vpk && d.abort_vpk <= sc::MAX_DRIVE_V,
                "an abort is above the ceiling and at most 10 V",
            );
        }
        let (field, n) = match &self.target {
            WireTargetSet::Waveform { .. } => ("target.pulses", self.target.len()),
            WireTargetSet::Srs { .. } => ("target.controls", self.target.len()),
        };
        if n != nc {
            err(
                &mut r,
                "targets_length_mismatch",
                field,
                format!("{n} targets for {nc} controls; give one target per control."),
            );
        }
        if let WireTargetSet::Srs {
            bands,
            tolerance_db,
            controls,
        } = &self.target
        {
            if bands.centres().is_empty() {
                err(
                    &mut r,
                    "bands_empty",
                    "target.bands",
                    "The band grid holds no natural frequency.",
                );
            }
            if !(tolerance_db.0 < 0.0 && tolerance_db.1 > 0.0) {
                range(
                    &mut r,
                    "target.tolerance_db",
                    tolerance_db.0,
                    false,
                    "the tolerance is below 0 dB under the target and above 0 dB over it",
                );
            }
            for (i, c) in controls.iter().enumerate() {
                srsc::check_synthesis(&mut r, &c.synthesis, &format!("target.controls[{i}]."));
            }
        }
        if self.verdict == VerdictMode::Global && self.target.kind() == TargetKind::Waveform {
            err(
                &mut r,
                "verdict_needs_srs",
                "verdict",
                "The global verdict judges an SRS target; this target is a waveform.",
            );
        }
        if let Timing::Hold(h) = &self.timing {
            if h.delays_s.len() != nc {
                err(
                    &mut r,
                    "delays_length_mismatch",
                    "timing.delays_s",
                    format!(
                        "{} delays for {nc} controls; a held timing gives one per control.",
                        h.delays_s.len()
                    ),
                );
            }
            if h.delays_s.iter().any(|d| !(0.0..=0.1).contains(d)) {
                range(
                    &mut r,
                    "timing.delays_s",
                    h.delays_s.iter().copied().fold(0.0, f64::max),
                    false,
                    "a delay is 0 to 0.1 s",
                );
            }
        }
        let window = match &self.timing {
            Timing::Free { window_s } => *window_s,
            Timing::Hold(h) => h.window_s,
        };
        range(
            &mut r,
            "timing.window_s",
            window,
            window > 0.0 && window <= 0.1,
            "a window is above 0 and at most 0.1 s",
        );
        let fs = self.fs_hz;
        range(
            &mut r,
            "acquisition.fs_hz",
            fs,
            (sc::FS_RANGE.0..=sc::FS_RANGE.1).contains(&fs),
            "the station runs from 1000 to 204800 Hz",
        );
        if let Some(n) = self.record_len {
            if !(n.is_power_of_two() && (MIN_RECORD_LEN..=MAX_RECORD_LEN).contains(&n)) {
                range(
                    &mut r,
                    "acquisition.record_len",
                    n as f64,
                    false,
                    "a record is a power of two from 64 to 16384",
                );
            }
        }
        let (lo, hi) = self.band(fs);
        if !(lo >= 0.0 && lo < hi) {
            range(
                &mut r,
                "acquisition.band_hz",
                lo,
                false,
                "the band is [low, high] with low below high",
            );
        } else if hi >= fs / 2.0 {
            err(
                &mut r,
                "band_above_grid",
                "acquisition.band_hz",
                format!(
                    "The band reaches {} Hz; it stays below half the rate ({} Hz).",
                    num(hi),
                    num(fs / 2.0)
                ),
            );
        }
        range(
            &mut r,
            "acquisition.lead_s",
            self.lead_s,
            (0.0..=1.0).contains(&self.lead_s),
            "a lead is 0 to 1 s",
        );
        let l = &self.levels_db;
        if l.is_empty() {
            err(
                &mut r,
                "levels_empty",
                "schedule.levels_db",
                "The ladder has no level.",
            );
        } else if l.windows(2).any(|w| w[1] <= w[0]) || l.last() != Some(&0.0) {
            err(
                &mut r,
                "levels_not_ascending",
                "schedule.levels_db",
                "The levels ascend and the last is 0 dB (full level).",
            );
        } else if l[0] < sc::LEVEL_DB_RANGE.0 {
            range(
                &mut r,
                "schedule.levels_db",
                l[0],
                false,
                "a level is at least -60 dB",
            );
        }
        for (f, v) in [
            ("schedule.max_pulses_per_level", self.max_pulses_per_level),
            ("schedule.full_level_pulses", self.full_level_pulses),
            ("control.law_misses", self.law_misses),
            ("control.divergence_pulses", self.divergence_pulses),
        ] {
            range(&mut r, f, v as f64, (1..=100).contains(&v), "1 to 100");
        }
        range(
            &mut r,
            "control.max_pulses",
            self.max_pulses as f64,
            (1..=100_000).contains(&self.max_pulses),
            "1 to 100000",
        );
        for (f, v) in [
            ("spacing.cooling_s", self.cooling_s),
            ("spacing.settling_s", self.settling_s),
            ("spacing.compute_s", self.compute_s),
        ] {
            range(
                &mut r,
                f,
                v,
                (0.0..=sc::MAX_SPACING_S).contains(&v),
                "0 to 3600 s",
            );
        }
        if self.spacing_s() <= 0.0 {
            range(
                &mut r,
                "spacing",
                0.0,
                false,
                "the time between pulses is above 0",
            );
        }
        let min_ceiling = self
            .drives
            .iter()
            .map(|d| d.ceiling_vpk)
            .fold(f64::INFINITY, f64::min);
        range(
            &mut r,
            "identification.drive_vrms",
            self.drive_vrms,
            self.drive_vrms > 0.0 && self.drive_vrms < min_ceiling,
            "above 0 and under every drive's ceiling",
        );
        let f = self.frames();
        range(
            &mut r,
            "identification.frames",
            f as f64,
            f >= 10 * nd as u64 && f <= 4096,
            "10 times the drives to 4096",
        );
        range(
            &mut r,
            "identification.coherence_floor",
            self.coherence_floor,
            (0.0..=1.0).contains(&self.coherence_floor),
            "0 to 1",
        );
        range(
            &mut r,
            "control.law_budget",
            self.law_budget,
            self.law_budget > 0.0 && self.law_budget <= 1.0,
            "above 0 and at most 1",
        );
        for (f, v) in [
            ("control.max_growth_db", self.max_growth_db),
            ("control.max_band_growth_db", self.max_band_growth_db),
        ] {
            range(
                &mut r,
                f,
                v,
                v > 0.0 && v <= 40.0,
                "above 0 and at most 40 dB",
            );
        }
        range(
            &mut r,
            "control.divergence_drive_ratio",
            self.divergence_drive_ratio,
            self.divergence_drive_ratio > 1.0 && self.divergence_drive_ratio <= 10.0,
            "above 1 and at most 10",
        );
        range(
            &mut r,
            "control.first_drive_ratio",
            self.first_drive_ratio,
            (1.0..=10.0).contains(&self.first_drive_ratio),
            "1 to 10",
        );
        if let Some(a) = self.abort_db {
            range(
                &mut r,
                "control.abort_db",
                a,
                a > 0.0 && a <= 40.0,
                "above 0 and at most 40 dB",
            );
        }
        for (f, v) in [
            ("control.max_velocity_ips", self.max_velocity_ips),
            ("control.max_displacement_in", self.max_displacement_in),
        ] {
            if let Some(v) = v {
                range(&mut r, f, v, v > 0.0, "above 0");
            }
        }
        if let Some(p) = &self.plant {
            if !PLANTS.contains(&p.as_str()) {
                err(
                    &mut r,
                    "plant_unknown",
                    "simulator.plant",
                    format!(
                        "\"{p}\" is not a simulator plant; the shock plants are {}.",
                        PLANTS.join(", ")
                    ),
                );
            }
        }
        // The law.
        match (ctx.law)(&self.law_id) {
            None => err(
                &mut r,
                "law_unknown",
                "control.law.id",
                format!("No control law \"{}\" is in the catalog.", self.law_id),
            ),
            Some(f) => match f.world {
                Err(for_) => err(
                    &mut r,
                    "law_wrong_world",
                    "control.law.id",
                    format!(
                        "The law \"{}\" is a {for_} law, not a MIMO Shock law.",
                        f.name
                    ),
                ),
                Ok(()) => {
                    let kind = self.target.kind();
                    if !f.targets.contains(&kind) {
                        err(
                            &mut r,
                            "law_wrong_target",
                            "control.law.id",
                            format!(
                                "The law \"{}\" does not run {} targets.",
                                f.name,
                                kind.as_str()
                            ),
                        );
                    }
                    for (k, v) in &self.law_params {
                        match f.params.iter().find(|p| &p.key == k) {
                            None => err(
                                &mut r,
                                "law_param_unknown",
                                &format!("control.law.params.{k}"),
                                format!("The law \"{}\" has no parameter \"{k}\".", f.name),
                            ),
                            Some(spec) => {
                                if let Some(why) = sc::param_problem(spec, v) {
                                    err(
                                        &mut r,
                                        "law_param_invalid",
                                        &format!("control.law.params.{k}"),
                                        format!("{} {why}", spec.label),
                                    );
                                }
                            }
                        }
                    }
                }
            },
        }
        if !r.errors.is_empty() {
            return (r, None);
        }
        // The targets at the asked rate.
        let spec = self.target_spec();
        let Some(n) = self.record_len_at(&spec, fs) else {
            err(
                &mut r,
                "target_invalid",
                "target",
                "No record of up to 16384 samples holds the targets at this rate.",
            );
            return (r, None);
        };
        let built = match spec.build(fs, n) {
            Ok(b) => b,
            Err(e) => {
                err(&mut r, "target_invalid", "target", format!("{e}."));
                return (r, None);
            }
        };
        let record_s = n as f64 / fs;
        if record_s >= ctx.timeout_s {
            err(
                &mut r,
                "record_too_long",
                "acquisition.record_len",
                format!(
                    "One record lasts {} s, as long as the controller timeout ({} s).",
                    num(sc::round(record_s, 3)),
                    num(ctx.timeout_s)
                ),
            );
            return (r, None);
        }
        let w = built.target.waveforms();
        let controls = self
            .controls
            .iter()
            .enumerate()
            .map(|(c, ch)| {
                (
                    *ch,
                    w[c * n..(c + 1) * n]
                        .iter()
                        .fold(0.0f64, |a, v| a.max(v.abs())),
                )
            })
            .collect();
        let spacing_s = self.spacing_s();
        let derived = Derived {
            fs_hz: fs,
            record_len: n,
            controls,
            spacing_s,
            call_budget_s: self.law_budget * spacing_s,
            target: built.target,
        };
        (r, Some(derived))
    }
}

/// The `config_invalid` refusal of `report`.
pub fn config_invalid(report: &Report) -> MethodError {
    sc::config_invalid(report)
}

/// Whether an item points at a field (for the dry run's grouping).
pub fn is_field(i: &Item) -> bool {
    matches!(i.at, At::Field(_))
}
