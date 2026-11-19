//! The daemon's configuration file, and the refusals that keep a misconfigured
//! daemon from starting.
//!
//! # Everything load-bearing is EXPLICIT and has no permissive default
//!
//! Two fields have no default at all - `bind` and `token` - and one has a default
//! that refuses rather than admits. That is the whole design of this module:
//!
//! * **`bind` must name an interface.** There is deliberately NO `0.0.0.0` default.
//!   A daemon that binds every interface by default puts a shaker on whatever network
//!   the box happens to be plugged into, including a guest VLAN or a hotel Wi-Fi - and
//!   it does so on the day someone runs it without reading the config. Naming the
//!   interface is a two-second decision that has to be made once; the failure mode of
//!   not making it is not recoverable by noticing later.
//! * **`token` must be non-empty.** Same reasoning one layer up
//!   (`Hello::check_token`): a blank token admits nobody rather than everybody, and
//!   this refusal means the operator finds out at start rather than at connect.
//! * **`controller_timeout_s` defaults to 30 s** (the design's number), and it is a
//!   ramp-down deadline, not an abort deadline.
//!
//! # What is NOT configurable, and why
//!
//! There is no `allow_any_token`, no `bind_all`, and no `fallback_station`. Each would
//! be a switch whose only purpose is to defeat a refusal that exists for a reason; a
//! config option is documentation that someone will eventually follow.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The parsed daemon configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonConfig {
    /// The interface AND port to serve on, e.g. `"192.168.4.20:8765"` or
    /// `"127.0.0.1:8765"`. REQUIRED - see the module docs on why there is no
    /// wildcard default.
    pub bind: String,

    /// The shared admission token every peer must present in its `Hello`. REQUIRED
    /// and must be non-empty.
    pub token: String,

    /// Where captures, recordings and the lockfile live. Defaults to
    /// `hertzforge_capture::default_data_root()` (`$HERTZFORGE_DATA_ROOT`, else `~/Hertzforge_Data`).
    #[serde(default)]
    pub data_root: Option<PathBuf>,

    /// Seconds between the server's WebSocket pings. The client answers with a pong;
    /// a Controller whose pongs stop starts the [`DaemonConfig::controller_timeout_s`]
    /// clock.
    #[serde(default = "default_heartbeat_s")]
    pub heartbeat_s: f64,

    /// Seconds a run continues after the CONTROLLER stops answering, before the
    /// daemon brings it down.
    ///
    /// **The run CONTINUES during this window**: a network
    /// hiccup on the operator's laptop must not abort a 40-minute test the hardware
    /// host is running perfectly well. When the window closes with no reconnect, the
    /// stop is the CONTROLLED ramp-down path - "nobody is watching" is a reason to
    /// stop safely, never a reason to slam the drive to zero.
    #[serde(default = "default_controller_timeout_s")]
    pub controller_timeout_s: f64,

    /// The station the daemon builds for its modules: `"sim"` or `"ni"`.
    ///
    /// Named rather than discovered. The Tauri app reads the operator's live
    /// selection from a running UI; a headless daemon has no operator to ask, and
    /// "whatever is attached" is exactly the substitution the station factory exists
    /// to refuse.
    #[serde(default = "default_station")]
    pub station: String,

    /// The NI device ids the station binds, when `station = "ni"`. EMPTY means "every
    /// device the scan finds", which is the pre-selection case and is a development
    /// convenience, not a claim that the first device is the right device.
    #[serde(default)]
    pub ni_devices: Vec<String>,

    /// Refuse to start when the NI preflight cannot reserve the configured devices.
    ///
    /// `true` by default: a daemon that starts anyway will fail at the operator's
    /// first `configure`, three screens away from the cause. Set `false` only to run
    /// the daemon's non-hardware surface (channel table, capture list) on a box where
    /// the devices are legitimately busy.
    #[serde(default = "default_true")]
    pub preflight_required: bool,
}

fn default_heartbeat_s() -> f64 {
    5.0
}
fn default_controller_timeout_s() -> f64 {
    30.0
}
fn default_station() -> String {
    "sim".to_string()
}
fn default_true() -> bool {
    true
}

/// Why a configuration was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The file could not be read.
    Read {
        /// The path that could not be read.
        path: String,
        /// The OS error.
        detail: String,
    },
    /// The file is not valid TOML, or carries a field this build does not know.
    Parse {
        /// The path that could not be parsed.
        path: String,
        /// The parser's own message, which names the offending key.
        detail: String,
    },
    /// A required field is missing, empty, or nonsensical.
    Invalid(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Read { path, detail } => {
                write!(f, "cannot read the daemon config at {path}: {detail}")
            }
            ConfigError::Parse { path, detail } => {
                write!(f, "the daemon config at {path} is not valid: {detail}")
            }
            ConfigError::Invalid(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl DaemonConfig {
    /// Read and validate a config file.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Read {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;
        Self::parse(&text, path)
    }

    /// Parse and validate config TEXT. Split from [`DaemonConfig::load`] so every
    /// refusal is testable without a filesystem.
    pub fn parse(text: &str, path: &Path) -> Result<Self, ConfigError> {
        let cfg: DaemonConfig = toml::from_str(text).map_err(|e| ConfigError::Parse {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// THE REFUSALS. Every one of them is a thing that must not be defaulted.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let addr: SocketAddr = self.bind.parse().map_err(|_| {
            ConfigError::Invalid(format!(
                "`bind` must be an explicit address:port (e.g. \"192.168.4.20:8765\" \
                 or \"127.0.0.1:8765\"); got {:?}. There is no default: a daemon that \
                 binds every interface puts a shaker on whatever network the box is \
                 plugged into.",
                self.bind
            ))
        })?;
        if addr.ip().is_unspecified() {
            return Err(ConfigError::Invalid(format!(
                "`bind` is the WILDCARD address {addr}, which serves every interface \
                 on this host. Name the interface you mean - the lab NIC, or \
                 127.0.0.1 for a same-box front end. This is refused rather than \
                 warned about because the failure mode is a shaker reachable from a \
                 network nobody intended."
            )));
        }
        if self.token.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "`token` is required and must be non-empty: it is the shared secret \
                 every front end presents at the handshake. A blank token admits \
                 nobody (see Hello::check_token), so a daemon started with one could \
                 never serve anyone - refusing here says so at start instead of at \
                 the operator's first connect."
                    .into(),
            ));
        }
        if !(self.heartbeat_s.is_finite() && self.heartbeat_s > 0.0) {
            return Err(ConfigError::Invalid(format!(
                "`heartbeat_s` must be a positive number of seconds; got {}",
                self.heartbeat_s
            )));
        }
        if !(self.controller_timeout_s.is_finite() && self.controller_timeout_s > 0.0) {
            return Err(ConfigError::Invalid(format!(
                "`controller_timeout_s` must be a positive number of seconds; got {}",
                self.controller_timeout_s
            )));
        }
        if self.controller_timeout_s < self.heartbeat_s {
            return Err(ConfigError::Invalid(format!(
                "`controller_timeout_s` ({}) is shorter than `heartbeat_s` ({}), so a \
                 healthy client would be declared gone before it was ever asked to \
                 answer. Make the timeout a multiple of the heartbeat.",
                self.controller_timeout_s, self.heartbeat_s
            )));
        }
        match self.station.as_str() {
            "sim" | "ni" => {}
            other => {
                return Err(ConfigError::Invalid(format!(
                    "`station` must be \"sim\" (the HERTZFORGE-SIM physics plant) or \"ni\" \
                     (the NI-DAQmx station); got {other:?}."
                )))
            }
        }
        Ok(())
    }

    /// The validated bind address.
    pub fn socket_addr(&self) -> SocketAddr {
        self.bind.parse().expect("validated at load")
    }

    /// The data root, resolved.
    pub fn resolved_data_root(&self) -> PathBuf {
        self.data_root
            .clone()
            .unwrap_or_else(hertzforge_capture::default_data_root)
    }

    /// The station kind this config names.
    pub fn backend_kind(&self) -> hertzforge_station::BackendKind {
        match self.station.as_str() {
            "ni" => hertzforge_station::BackendKind::Ni,
            _ => hertzforge_station::BackendKind::Sim,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> PathBuf {
        PathBuf::from("/tmp/hertzforge-backendd.toml")
    }

    fn minimal() -> String {
        r#"
bind = "127.0.0.1:8765"
token = "s3cret"
"#
        .to_string()
    }

    #[test]
    fn a_minimal_config_parses_with_the_documented_defaults() {
        let c = DaemonConfig::parse(&minimal(), &p()).expect("valid");
        assert_eq!(c.heartbeat_s, 5.0);
        assert_eq!(c.controller_timeout_s, 30.0, "the plan's default");
        assert_eq!(c.station, "sim");
        assert!(c.preflight_required);
        assert_eq!(c.backend_kind(), hertzforge_station::BackendKind::Sim);
    }

    /// THE WILDCARD REFUSAL (bind an EXPLICIT interface,
    /// never 0.0.0.0 by default).
    ///
    /// RED-MUTATION: delete the `is_unspecified()` arm and this reds - which is the
    /// state where a daemon started from a copied config serves a shaker to every
    /// network the box is attached to.
    #[test]
    fn the_wildcard_bind_is_refused_and_the_refusal_says_why() {
        for wildcard in ["0.0.0.0:8765", "[::]:8765"] {
            let text = format!("bind = \"{wildcard}\"\ntoken = \"x\"\n");
            let err = DaemonConfig::parse(&text, &p()).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("WILDCARD"), "{msg}");
            assert!(msg.contains("Name the interface"), "{msg}");
        }
        // ANTI-VACUITY: an explicit loopback bind is accepted, so the refusal above
        // is about the wildcard and not about every address.
        assert!(DaemonConfig::parse(&minimal(), &p()).is_ok());
    }

    /// A missing `bind` is a PARSE failure (the field is not optional), and a
    /// malformed one is an explicit refusal naming what a good value looks like.
    #[test]
    fn bind_is_required_and_must_be_an_address() {
        let err = DaemonConfig::parse("token = \"x\"\n", &p()).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }), "{err}");

        let err = DaemonConfig::parse("bind = \"eth0\"\ntoken = \"x\"\n", &p()).unwrap_err();
        assert!(err.to_string().contains("address:port"), "{err}");
    }

    /// A blank token is refused AT START, with the reason - not discovered when the
    /// first operator cannot connect.
    #[test]
    fn a_blank_token_is_refused_at_start() {
        for blank in ["", "   "] {
            let text = format!("bind = \"127.0.0.1:8765\"\ntoken = \"{blank}\"\n");
            let err = DaemonConfig::parse(&text, &p()).unwrap_err();
            assert!(
                err.to_string().contains("admits \nnobody") || err.to_string().contains("admits"),
                "{err}"
            );
        }
    }

    /// A timeout shorter than the heartbeat would declare a healthy client gone.
    #[test]
    fn a_timeout_shorter_than_the_heartbeat_is_refused() {
        let text = "bind = \"127.0.0.1:8765\"\ntoken = \"x\"\nheartbeat_s = 10.0\n\
                    controller_timeout_s = 5.0\n";
        let err = DaemonConfig::parse(text, &p()).unwrap_err();
        assert!(err.to_string().contains("shorter than"), "{err}");
    }

    /// An unknown key is a REFUSAL, not a silent ignore. A misspelled `token` that
    /// parsed and then defaulted would be the same class of defect as a silently
    /// dropped profile field.
    #[test]
    fn an_unknown_config_key_is_refused() {
        let text = "bind = \"127.0.0.1:8765\"\ntoken = \"x\"\nbind_all = true\n";
        let err = DaemonConfig::parse(text, &p()).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert!(err.to_string().contains("bind_all"), "{err}");
    }

    /// A station the daemon cannot serve is refused by name, which beats a confusing failure
    /// three layers down.
    #[test]
    fn only_sim_and_ni_are_servable_stations() {
        let text = "bind = \"127.0.0.1:8765\"\ntoken = \"x\"\nstation = \"unknown\"\n";
        let err = DaemonConfig::parse(text, &p()).unwrap_err();
        assert!(err.to_string().contains("\"sim\""), "{err}");

        let text = "bind = \"127.0.0.1:8765\"\ntoken = \"x\"\nstation = \"ni\"\n";
        let c = DaemonConfig::parse(text, &p()).expect("ni is servable");
        assert_eq!(c.backend_kind(), hertzforge_station::BackendKind::Ni);
    }
}
