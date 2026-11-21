//! The measurement model: which rows are references, which are responses, where each
//! one is (its DOF) at every point of the roving plan, and which rows are driven. The
//! table's rules a setup is refused by live here, each by name.
//!
//! A row's part comes from its analysis role (`reference` or `response`); a row with any
//! other analysis role is recorded in the streams and takes no part in a measurement.
//! A roving channel takes the point's node (and its direction, when the point gives
//! one) at each point; every other row keeps the table's DOF throughout.

use crate::setup::{ModalSetup, Mode, Refusal};
use crate::structure::Dof;
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use serde_json::{json, Value};

/// A measured row.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The table index.
    pub index: u32,
    /// Its place in the station's input frame (enabled non-drive rows in table order).
    pub slot: usize,
    pub unit: String,
    pub range: f64,
    pub description: String,
    /// The table's DOF, when complete.
    pub dof: Option<Dof>,
    pub roves: bool,
}

/// A driven row.
#[derive(Debug, Clone, PartialEq)]
pub struct DriveRow {
    pub index: u32,
    /// Its place in the station's drive frame (enabled drive rows in table order).
    pub slot: usize,
    pub level_vrms: f64,
    pub range_v: f64,
}

/// One measurement: a point's DOFs for every reference and response.
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    /// The point, from 0; `None` with no roving plan.
    pub point: Option<usize>,
    pub refs: Vec<Dof>,
    pub resps: Vec<Dof>,
}

/// The plan of a test.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub inputs: usize,
    pub outputs: usize,
    pub refs: Vec<Row>,
    pub resps: Vec<Row>,
    pub drives: Vec<DriveRow>,
    pub measurements: Vec<Measurement>,
}

impl Plan {
    /// The plan, or every refusal the table and the setup give together.
    pub fn build(table: &ChannelTable, s: &ModalSetup) -> Result<Plan, Vec<Refusal>> {
        let mut r = s.refusals();
        let inputs: Vec<_> = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .collect();
        let drive_rows: Vec<_> = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Drive)
            .collect();
        let roving: Vec<u32> = s
            .roving
            .as_ref()
            .map(|v| v.channels.clone())
            .unwrap_or_default();
        let mut refs = Vec::new();
        let mut resps = Vec::new();
        for (slot, c) in inputs.iter().enumerate() {
            let part = c.analysis.role.as_str();
            if part != "reference" && part != "response" {
                continue;
            }
            let dir = c.analysis.direction.trim();
            let dof = c
                .analysis
                .node
                .filter(|_| !dir.is_empty())
                .map(|n| Dof::new(n, dir));
            let row = Row {
                index: c.index,
                slot,
                unit: c.units.name(),
                range: c.range_max,
                description: c.description.clone(),
                dof,
                roves: roving.contains(&c.index),
            };
            if part == "reference" {
                refs.push(row);
            } else {
                resps.push(row);
            }
        }
        if refs.is_empty() {
            r.push(Refusal::channel(
                "no_reference",
                None,
                "No enabled input row has the analysis role reference: give the hammer's or each \
                 shaker's force row that role.",
            ));
        }
        if resps.is_empty() {
            r.push(Refusal::channel(
                "no_response",
                None,
                "No enabled input row has the analysis role response.",
            ));
        }
        let point_dirs = s.roving.as_ref().is_some_and(|v| {
            !v.points.is_empty() && v.points.iter().all(|p| p.direction.is_some())
        });
        for row in refs.iter().chain(&resps) {
            let c = table.channels.iter().find(|c| c.index == row.index);
            let has_node = c.and_then(|c| c.analysis.node).is_some();
            let has_dir = c.is_some_and(|c| !c.analysis.direction.trim().is_empty());
            let ok = if row.roves {
                has_dir || point_dirs
            } else {
                has_node && has_dir
            };
            if !ok {
                r.push(Refusal::channel(
                    "dof_missing",
                    Some(row.index),
                    format!(
                        "Row {} has no {}: every reference and response has a node and a \
                         direction, or roves.",
                        row.index,
                        if row.roves || has_node {
                            "direction"
                        } else {
                            "node"
                        }
                    ),
                ));
            } else if has_dir {
                let d = c
                    .map(|c| c.analysis.direction.trim().to_owned())
                    .unwrap_or_default();
                if !crate::setup::direction_ok(&d) {
                    r.push(Refusal::channel(
                        "dof_missing",
                        Some(row.index),
                        format!(
                            "Row {}'s direction \"{d}\" is not X, Y or Z with a sign.",
                            row.index
                        ),
                    ));
                }
            }
        }
        for ch in &roving {
            if !refs.iter().chain(&resps).any(|x| x.index == *ch) {
                r.push(Refusal::field(
                    "roving_invalid",
                    "roving.channels",
                    format!("Row {ch} roves but is not an enabled reference or response row."),
                ));
            }
        }
        let mut drives = Vec::new();
        match s.mode {
            Mode::Impact => {
                if refs.len() > 1 {
                    r.push(Refusal::channel(
                        "reference_count",
                        None,
                        format!(
                            "Impact mode measures from one hammer: {} rows have the analysis role \
                             reference.",
                            refs.len()
                        ),
                    ));
                }
                if let (Some(t), Some(h)) = (&s.impact.target, refs.first()) {
                    if t.peak > h.range {
                        r.push(Refusal::field(
                            "out_of_range",
                            "impact.target.peak",
                            format!(
                                "The target peak force {} {} is over the hammer row's range of {} {}.",
                                t.peak, h.unit, h.range, h.unit
                            ),
                        ));
                    }
                }
            }
            Mode::Shaker => {
                for d in &s.shaker.drives {
                    match drive_rows.iter().position(|c| c.index == d.row) {
                        Some(slot) => {
                            let c = drive_rows[slot];
                            if s.shaker.ceiling_v > c.range_max {
                                r.push(Refusal::channel(
                                    "ceiling_over_range",
                                    Some(d.row),
                                    format!(
                                        "The drive ceiling of {} V is over drive row {}'s range of {} V.",
                                        s.shaker.ceiling_v, d.row, c.range_max
                                    ),
                                ));
                            }
                            drives.push(DriveRow {
                                index: d.row,
                                slot,
                                level_vrms: d.level_vrms,
                                range_v: c.range_max,
                            });
                        }
                        None => r.push(Refusal::channel(
                            "drive_row_invalid",
                            Some(d.row),
                            format!("Row {} is not an enabled drive row of the table.", d.row),
                        )),
                    }
                }
                if !s.shaker.drives.is_empty()
                    && !refs.is_empty()
                    && refs.len() != s.shaker.drives.len()
                {
                    r.push(Refusal::channel(
                        "reference_count",
                        None,
                        format!(
                            "Shaker mode takes one reference (force) row per driven shaker: {} \
                             references for {} drives.",
                            refs.len(),
                            s.shaker.drives.len()
                        ),
                    ));
                }
                if let Some(x) = refs.iter().find(|x| x.roves) {
                    r.push(Refusal::channel(
                        "roving_invalid",
                        Some(x.index),
                        "A shaker's force row does not rove: rove the responses.",
                    ));
                }
            }
        }
        if !r.is_empty() {
            return Err(r);
        }
        let take = |row: &Row, p: Option<&crate::setup::Point>| -> Dof {
            match (row.roves, p) {
                (true, Some(p)) => Dof {
                    node: p.node,
                    direction: p
                        .direction
                        .clone()
                        .or_else(|| row.dof.as_ref().map(|d| d.direction.clone()))
                        .unwrap_or_default(),
                },
                _ => row.dof.clone().unwrap_or_else(|| Dof::new(0, "")),
            }
        };
        let measurements = match &s.roving {
            Some(v) => v
                .points
                .iter()
                .enumerate()
                .map(|(k, p)| Measurement {
                    point: Some(k),
                    refs: refs.iter().map(|x| take(x, Some(p))).collect(),
                    resps: resps.iter().map(|x| take(x, Some(p))).collect(),
                })
                .collect(),
            None => vec![Measurement {
                point: None,
                refs: refs.iter().map(|x| take(x, None)).collect(),
                resps: resps.iter().map(|x| take(x, None)).collect(),
            }],
        };
        Ok(Plan {
            inputs: inputs.len(),
            outputs: drive_rows.len(),
            refs,
            resps,
            drives,
            measurements,
        })
    }

    /// A measurement's rows as the results name them.
    pub fn rows_json(&self, m: &Measurement) -> (Vec<Value>, Vec<Value>) {
        let side = |rows: &[Row], dofs: &[Dof]| -> Vec<Value> {
            rows.iter()
                .zip(dofs)
                .map(|(r, d)| json!({"index": r.index, "node": d.node, "direction": d.direction, "unit": r.unit}))
                .collect()
        };
        (side(&self.refs, &m.refs), side(&self.resps, &m.resps))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::setup::{DriveSet, Point, Roving, SourceKind};
    use hertzforge_channel_table::{Channel, Units};

    /// A table: row 0 a hammer at node 1 Z+, row 1 a response at node 4 Z+, row 2 a
    /// monitor, and with `shakers` two drive rows and a second force row.
    pub(crate) fn table(shakers: bool) -> ChannelTable {
        let mut t = ChannelTable::new();
        let mut c0 = Channel::new(0);
        c0.units = Units::ForceLbf;
        c0.range_max = 100.0;
        c0.analysis.node = Some(1);
        c0.analysis.direction = "Z+".into();
        c0.analysis.role = "reference".into();
        let mut c1 = Channel::new(1);
        c1.range_max = 50.0;
        c1.analysis.node = Some(4);
        c1.analysis.direction = "Z+".into();
        c1.analysis.role = "response".into();
        let mut c2 = Channel::new(2);
        c2.analysis.role = "monitor".into();
        t.channels = vec![c0, c1, c2];
        if shakers {
            let mut f2 = Channel::new(3);
            f2.units = Units::ForceLbf;
            f2.range_max = 100.0;
            f2.analysis.node = Some(8);
            f2.analysis.direction = "Z+".into();
            f2.analysis.role = "reference".into();
            t.channels.push(f2);
            for k in [4u32, 5] {
                let mut d = Channel::new(k);
                d.role = ChannelRole::Drive;
                d.units = Units::VoltageV;
                d.range_max = 10.0;
                t.channels.push(d);
            }
        }
        t
    }

    fn reasons(t: &ChannelTable, s: &ModalSetup) -> Vec<&'static str> {
        Plan::build(t, s)
            .err()
            .unwrap_or_default()
            .iter()
            .map(|r| r.reason)
            .collect()
    }

    #[test]
    fn modal_test_roving_hammer_plan_moves_the_reference() {
        let mut s = ModalSetup::new(Mode::Impact, 1024.0, 1024);
        s.roving = Some(Roving {
            channels: vec![0],
            points: (1..=3)
                .map(|n| Point {
                    node: n,
                    direction: None,
                })
                .collect(),
        });
        let p = Plan::build(&table(false), &s).unwrap();
        assert_eq!(p.inputs, 3);
        assert_eq!(p.measurements.len(), 3);
        assert_eq!(p.measurements[2].refs[0], Dof::new(3, "Z+"));
        assert_eq!(p.measurements[2].resps[0], Dof::new(4, "Z+"));
    }

    #[test]
    fn modal_test_table_refusals_are_named() {
        let s = ModalSetup::new(Mode::Impact, 1024.0, 1024);
        let mut t = table(false);
        t.channels[0].analysis.role = "monitor".into();
        assert_eq!(reasons(&t, &s), vec!["no_reference"]);
        let mut t = table(false);
        t.channels[1].analysis.role.clear();
        assert_eq!(reasons(&t, &s), vec!["no_response"]);
        let mut t = table(false);
        t.channels[1].analysis.node = None;
        assert_eq!(reasons(&t, &s), vec!["dof_missing"]);
        // Two references in impact mode.
        assert_eq!(reasons(&table(true), &s), vec!["reference_count"]);
    }

    #[test]
    fn modal_test_shaker_plan_takes_one_reference_per_drive() {
        let mut s = ModalSetup::new(Mode::Shaker, 1024.0, 1024);
        s.shaker.source = SourceKind::BurstRandom;
        s.shaker.drives = vec![
            DriveSet {
                row: 4,
                level_vrms: 0.05,
            },
            DriveSet {
                row: 5,
                level_vrms: 0.05,
            },
        ];
        let p = Plan::build(&table(true), &s).unwrap();
        assert_eq!((p.refs.len(), p.drives.len(), p.outputs), (2, 2, 2));
        s.shaker.drives.pop();
        assert_eq!(reasons(&table(true), &s), vec!["reference_count"]);
        s.shaker.drives = vec![DriveSet {
            row: 1,
            level_vrms: 0.05,
        }];
        assert!(reasons(&table(true), &s).contains(&"drive_row_invalid"));
    }

    #[test]
    fn modal_test_target_over_the_hammer_range_is_refused() {
        let mut s = ModalSetup::new(Mode::Impact, 1024.0, 1024);
        s.impact.target = Some(crate::setup::ForceTarget {
            peak: 150.0,
            tolerance_pct: 20.0,
        });
        assert_eq!(reasons(&table(false), &s), vec!["out_of_range"]);
    }

    #[test]
    fn modal_test_a_ceiling_over_a_drive_rows_range_is_refused_by_row() {
        let mut s = ModalSetup::new(Mode::Shaker, 1024.0, 1024);
        s.shaker.drives = vec![
            DriveSet {
                row: 4,
                level_vrms: 0.05,
            },
            DriveSet {
                row: 5,
                level_vrms: 0.05,
            },
        ];
        let mut t = table(true);
        assert!(Plan::build(&t, &s).is_ok());
        // drive row 5 reaches 2 V only: a 3 V ceiling is over it
        s.shaker.ceiling_v = 3.0;
        assert!(Plan::build(&t, &s).is_ok());
        t.channels
            .iter_mut()
            .find(|c| c.index == 5)
            .unwrap()
            .range_max = 2.0;
        let r = Plan::build(&t, &s).err().unwrap();
        assert_eq!(r.len(), 1, "{r:?}");
        assert_eq!(r[0].reason, "ceiling_over_range");
        assert_eq!(r[0].channel, Some(5));
        assert!(
            r[0].message.contains("3 V") && r[0].message.contains("2 V"),
            "{}",
            r[0].message
        );
    }
}
