//! Stream SELECTORS - what a client asks to be sent, and how much of it (v3).
//!
//! # Why selectors exist, and why they had to precede the daemon
//!
//! v1/v2's `Subscribe { streams: Vec<StreamKind> }` was all-or-nothing per stream:
//! subscribing to `Waveform` meant every enabled channel, at whatever cadence the
//! producer chose. In one process that costs a memcpy. Across a network it costs the
//! link, and the 400-channel posture the design targets makes the difference
//! structural rather than a matter of taste: at 400 channels a full waveform stream is
//! not a display feed, it is a firehose the operator did not ask for and cannot use -
//! a laptop screen has perhaps a dozen useful traces on it.
//!
//! So a subscription now states three things beyond the stream kind:
//!
//! * **which channels** ([`ChannelSel`]) - all, an explicit id list, or one control
//!   group;
//! * **how often** (`max_rate_hz`) - a producer-side cadence ceiling;
//! * **how finely** (`bin_decimation`) - keep every Nth spectrum bin.
//!
//! All three are HINTS THE PRODUCER HONORS, never guarantees the consumer enforces:
//! the point is that the bytes are not produced or sent, not that they are filtered
//! after arrival. A selector honored only at the receiver saves nothing.
//!
//! Under the exact-match version rule (see [`crate::envelope`]) there is no graceful
//! degradation, so this change had to land as a version bump BEFORE the deployment
//! that needs it - which is why the selectors are wire bump #2 and ship in the
//! same increment as the daemon that uses them, not after it.
//!
//! # What a selector is NOT
//!
//! It is not a control-path filter. Nothing here can narrow what the CONTROL loop
//! reads: the loop is co-resident with the hardware and never crosses this seam at
//! all (crate docs). Decimating a display stream to 4 channels at 10 Hz has no effect
//! on the 51.2 kHz acquisition feeding the servo, and must never be presented in a UI
//! as though it did.

use serde::{Deserialize, Serialize};

use crate::frames::StreamKind;

/// Which channels a subscription wants.
///
/// Closed enum, three cases, each one an answer an operator actually gives: "show me
/// everything" (a small rig), "show me these" (the traces on screen), "show me this
/// control group" (a MIMO drive group, without the UI having to know its membership -
/// the producer resolves it from the channel table, which is the authority).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelSel {
    /// Every channel the stream would otherwise carry.
    All,
    /// Exactly these channel ids (the channel table's 1-based `index`). Ids the
    /// producer does not have are silently absent from the stream rather than an
    /// error: a UI that keeps a subscription across a table edit should degrade to
    /// fewer traces, not to a refused subscription mid-run.
    Ids(Vec<u32>),
    /// Every channel whose `control_group` is this. Resolved BY THE PRODUCER against
    /// the live channel table, so group membership has exactly one authority.
    Group(u32),
}

impl ChannelSel {
    /// Does this selector accept `channel_id`, given the group the producer knows
    /// that channel belongs to?
    ///
    /// `group` is `None` when the channel has no control group; a [`ChannelSel::Group`]
    /// selector then rejects it (an ungrouped channel is not in group 0 - it is in no
    /// group, and treating "no group" as "group 0" would silently widen a MIMO
    /// subscription).
    pub fn accepts(&self, channel_id: u32, group: Option<u32>) -> bool {
        match self {
            ChannelSel::All => true,
            ChannelSel::Ids(ids) => ids.contains(&channel_id),
            ChannelSel::Group(g) => group == Some(*g),
        }
    }

    /// True for [`ChannelSel::All`] - the case a producer can shortcut.
    pub fn is_all(&self) -> bool {
        matches!(self, ChannelSel::All)
    }
}

impl Default for ChannelSel {
    /// `All`, which is what v2's `Subscribe` meant.
    fn default() -> Self {
        ChannelSel::All
    }
}

/// One stream subscription: a kind plus its selectors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamSub {
    /// Which pinned layout this subscription is for.
    pub kind: StreamKind,
    /// Which channels (see [`ChannelSel`]).
    #[serde(default)]
    pub channels: ChannelSel,
    /// Producer-side cadence ceiling, frames per second. `None` = the producer's own
    /// natural rate. A value of `0.0` or a negative value is treated as `None`
    /// ([`StreamSub::min_period_s`] answers `None`) rather than as "never send",
    /// because a UI arithmetic bug that produced a 0 must not silently make a
    /// display go dark and look like a dead link.
    pub max_rate_hz: Option<f64>,
    /// Keep every Nth spectrum bin. `None` or `0` or `1` all mean "every bin"
    /// ([`StreamSub::decimation`] normalizes). Meaningless for
    /// [`StreamKind::Scope`] / [`StreamKind::Waveform`], where the producer ignores
    /// it - stated here rather than modeled per-kind, because a per-kind selector
    /// enum would have to be extended in lockstep with every future stream.
    pub bin_decimation: Option<u32>,
}

impl StreamSub {
    /// Every channel, every frame, every bin - v2's `Subscribe` semantics exactly.
    ///
    /// The migration shape: `Subscribe { streams: vec![k] }` becomes
    /// `Subscribe { subs: vec![StreamSub::all(k)] }` with identical behavior.
    pub fn all(kind: StreamKind) -> Self {
        StreamSub {
            kind,
            channels: ChannelSel::All,
            max_rate_hz: None,
            bin_decimation: None,
        }
    }

    /// The minimum seconds between frames this subscription asks for, or `None` for
    /// "as fast as the producer runs".
    ///
    /// Non-finite and non-positive rates answer `None` (see `max_rate_hz`).
    pub fn min_period_s(&self) -> Option<f64> {
        match self.max_rate_hz {
            Some(hz) if hz.is_finite() && hz > 0.0 => Some(1.0 / hz),
            _ => None,
        }
    }

    /// The bin stride, normalized: always at least 1.
    pub fn decimation(&self) -> u32 {
        self.bin_decimation.unwrap_or(1).max(1)
    }

    /// Is bin index `i` kept under this subscription's decimation?
    ///
    /// Index 0 is always kept, so the DC/first bin never disappears at a stride the
    /// operator did not think about.
    pub fn keeps_bin(&self, i: usize) -> bool {
        i.is_multiple_of(self.decimation() as usize)
    }

    /// How many bins survive decimating `n_bins`.
    pub fn decimated_bins(&self, n_bins: usize) -> usize {
        let d = self.decimation() as usize;
        n_bins.div_ceil(d)
    }

    /// How many channels of `available` survive the channel selector.
    ///
    /// `available` is `(channel_id, control_group)` as the producer knows them.
    pub fn selected_channels(&self, available: &[(u32, Option<u32>)]) -> usize {
        available
            .iter()
            .filter(|(id, g)| self.channels.accepts(*id, *g))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_is_the_v2_shape_and_the_default() {
        let s = StreamSub::all(StreamKind::Waveform);
        assert!(s.channels.is_all());
        assert_eq!(s.min_period_s(), None);
        assert_eq!(s.decimation(), 1);
        assert_eq!(s.decimated_bins(1024), 1024);
        assert_eq!(ChannelSel::default(), ChannelSel::All);
    }

    #[test]
    fn id_selection_is_exact_and_group_selection_never_widens() {
        let ids = ChannelSel::Ids(vec![1, 4, 9]);
        assert!(ids.accepts(4, None));
        assert!(!ids.accepts(5, None));
        // A group selector matches ONLY the stated group.
        let g = ChannelSel::Group(2);
        assert!(g.accepts(7, Some(2)));
        assert!(!g.accepts(7, Some(3)));
        // ... and an UNGROUPED channel is in no group, not in group 0.
        assert!(!ChannelSel::Group(0).accepts(7, None));
    }

    /// A rate of zero or a NaN is "no ceiling", never "never send". A display that
    /// goes dark because of an arithmetic bug upstream is indistinguishable from a
    /// dead link, which is the worst failure a remote UI can present.
    #[test]
    fn a_zero_or_nonfinite_rate_is_no_ceiling_not_silence() {
        for hz in [Some(0.0), Some(-5.0), Some(f64::NAN), None] {
            let s = StreamSub {
                max_rate_hz: hz,
                ..StreamSub::all(StreamKind::Scope)
            };
            assert_eq!(s.min_period_s(), None, "{hz:?}");
        }
        let s = StreamSub {
            max_rate_hz: Some(10.0),
            ..StreamSub::all(StreamKind::Scope)
        };
        assert_eq!(s.min_period_s(), Some(0.1));
    }

    /// Decimation keeps bin 0 and rounds UP, so the last partial stride still
    /// contributes a bin rather than being dropped.
    #[test]
    fn decimation_keeps_the_first_bin_and_rounds_up() {
        let s = StreamSub {
            bin_decimation: Some(4),
            ..StreamSub::all(StreamKind::Spectrum)
        };
        assert!(s.keeps_bin(0));
        assert!(!s.keeps_bin(1));
        assert!(s.keeps_bin(4));
        assert_eq!(s.decimated_bins(1024), 256);
        assert_eq!(s.decimated_bins(1025), 257);

        // 0 and 1 both mean "every bin" - a UI that computes a stride of 0 must not
        // divide by zero on the producer side.
        for d in [Some(0), Some(1), None] {
            let s = StreamSub {
                bin_decimation: d,
                ..StreamSub::all(StreamKind::Spectrum)
            };
            assert_eq!(s.decimation(), 1, "{d:?}");
            assert_eq!(s.decimated_bins(1024), 1024);
        }
    }

    #[test]
    fn channel_counting_is_the_selector_applied() {
        let available: Vec<(u32, Option<u32>)> = (1..=400)
            .map(|i| (i, if i <= 8 { Some(1) } else { None }))
            .collect();
        assert_eq!(
            StreamSub::all(StreamKind::Waveform).selected_channels(&available),
            400
        );
        let four = StreamSub {
            channels: ChannelSel::Ids(vec![1, 2, 3, 4]),
            ..StreamSub::all(StreamKind::Waveform)
        };
        assert_eq!(four.selected_channels(&available), 4);
        let grp = StreamSub {
            channels: ChannelSel::Group(1),
            ..StreamSub::all(StreamKind::Waveform)
        };
        assert_eq!(grp.selected_channels(&available), 8);
    }
}
