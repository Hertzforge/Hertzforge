//! DOF addressing (the sdynpy-inspired node + signed-direction scheme,
//! research report 01 - design idea only, clean-room).
//!
//! v1 carries translational X/Y/Z plus scalar; rotational DOF arrive with the
//! modal package (Module B). String forms: "101X+", "12Z-", bare "7" for a
//! scalar DOF at node 7, "X+" for a direction with no node.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Signed direction code: 0 = scalar/none, ±1 = ±X, ±2 = ±Y, ±3 = ±Z.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Direction(i8);

impl Direction {
    pub const SCALAR: Direction = Direction(0);

    pub fn new(code: i8) -> Result<Direction, String> {
        if (-3..=3).contains(&code) {
            Ok(Direction(code))
        } else {
            Err(format!(
                "direction code {code} out of range (v1 supports 0, ±1..±3)"
            ))
        }
    }

    pub fn code(self) -> i8 {
        self.0
    }

    pub fn is_scalar(self) -> bool {
        self.0 == 0
    }

    /// Flip polarity (X+ <-> X-). Scalar is its own negation.
    pub fn flipped(self) -> Direction {
        Direction(-self.0)
    }

    /// Same axis regardless of polarity.
    pub fn same_axis(self, other: Direction) -> bool {
        self.0.abs() == other.0.abs()
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self.0 {
            0 => "",
            1 => "X+",
            -1 => "X-",
            2 => "Y+",
            -2 => "Y-",
            3 => "Z+",
            -3 => "Z-",
            _ => unreachable!("Direction invariant"),
        };
        f.write_str(s)
    }
}

impl FromStr for Direction {
    type Err = String;

    fn from_str(s: &str) -> Result<Direction, String> {
        let code = match s {
            "" => 0,
            "X+" | "X" | "x+" | "x" => 1,
            "X-" | "x-" => -1,
            "Y+" | "Y" | "y+" | "y" => 2,
            "Y-" | "y-" => -2,
            "Z+" | "Z" | "z+" | "z" => 3,
            "Z-" | "z-" => -3,
            other => return Err(format!("unknown direction '{other}'")),
        };
        Ok(Direction(code))
    }
}

/// A degree-of-freedom reference: optional node id + direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DofRef {
    pub node: Option<u32>,
    pub dir: Direction,
}

impl DofRef {
    pub fn scalar(node: u32) -> DofRef {
        DofRef {
            node: Some(node),
            dir: Direction::SCALAR,
        }
    }

    pub fn new(node: u32, dir: Direction) -> DofRef {
        DofRef {
            node: Some(node),
            dir,
        }
    }
}

impl fmt::Display for DofRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.node {
            Some(n) => write!(f, "{n}{}", self.dir),
            None => write!(f, "{}", self.dir),
        }
    }
}

impl FromStr for DofRef {
    type Err = String;

    fn from_str(s: &str) -> Result<DofRef, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty DOF string".into());
        }
        let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
        let rest = &s[digits.len()..];
        let node = if digits.is_empty() {
            None
        } else {
            Some(
                digits
                    .parse::<u32>()
                    .map_err(|e| format!("node id '{digits}': {e}"))?,
            )
        };
        let dir: Direction = rest.parse()?;
        if node.is_none() && dir.is_scalar() {
            return Err(format!("'{s}' has neither node nor direction"));
        }
        Ok(DofRef { node, dir })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_parse_round_trip() {
        for s in ["101X+", "12Z-", "7", "X+", "3Y-"] {
            let d: DofRef = s.parse().unwrap();
            let out = d.to_string();
            let back: DofRef = out.parse().unwrap();
            assert_eq!(d, back, "{s} -> {out}");
        }
    }

    #[test]
    fn canonicalizes_bare_axis_to_positive() {
        let d: DofRef = "5x".parse().unwrap();
        assert_eq!(d.to_string(), "5X+");
    }

    #[test]
    fn rejects_garbage() {
        for s in ["", "X++", "12Q+", "-3X+"] {
            assert!(s.parse::<DofRef>().is_err(), "{s} should fail");
        }
    }

    #[test]
    fn flip_and_axis() {
        let d: Direction = "Z+".parse().unwrap();
        assert_eq!(d.flipped().to_string(), "Z-");
        assert!(d.same_axis(d.flipped()));
        assert!(!d.same_axis("X+".parse().unwrap()));
    }
}
