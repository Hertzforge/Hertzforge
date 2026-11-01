//! Error type for the NI backend and its mapping onto [`eigen_hal::HalError`].

use eigen_hal::HalError;
use std::fmt;

/// Everything that can go wrong inside `eigen-hal-ni`.
///
/// Deliberately NOT a wrapper around a stringly-typed catch-all: the four cases
/// have different operator meanings and different UI treatments.
///
/// - [`NiError::DriverNotAvailable`] - there is no NI-DAQmx runtime on this host.
///   This is a normal, expected state on a developer Mac and MUST surface as
///   "no driver" rather than silently degrading to simulated data.
/// - [`NiError::MissingSymbol`] - the runtime loaded but does not export a
///   function this backend needs (an old or partial DAQmx install). Named per
///   symbol so the diagnosis is actionable; one missing symbol never bricks the
///   whole loader.
/// - [`NiError::Daqmx`] - the driver itself refused, carrying the driver's own
///   code and its extended error text verbatim.
/// - [`NiError::Config`] - Eigen's own configuration is inconsistent; the driver
///   was never asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NiError {
    /// No NI-DAQmx runtime could be loaded on this host.
    DriverNotAvailable {
        /// Library names that were tried (or a note that none were, on hosts
        /// where DAQmx does not exist at all).
        tried: String,
        /// Loader detail (dlopen/LoadLibrary message), when there was one.
        detail: String,
    },
    /// The runtime loaded but does not export `symbol`.
    MissingSymbol {
        symbol: &'static str,
        detail: String,
    },
    /// The driver returned a negative status. `text` is
    /// `DAQmxGetExtendedErrorInfo`'s output when it could be read.
    Daqmx {
        code: i32,
        text: String,
        /// What Eigen was doing (the call site), so the text has a subject.
        context: String,
    },
    /// Eigen's own configuration is inconsistent; the driver was never called.
    Config(String),
}

impl NiError {
    /// A configuration error from anything displayable.
    pub fn config(msg: impl Into<String>) -> Self {
        NiError::Config(msg.into())
    }

    /// The driver's numeric status code, when this error came from the driver.
    pub fn daqmx_code(&self) -> Option<i32> {
        match self {
            NiError::Daqmx { code, .. } => Some(*code),
            _ => None,
        }
    }

    /// True when this error means the device link is GONE (unplugged, powered
    /// off, removed from the chassis) as opposed to merely refused.
    ///
    /// The distinction is load-bearing upstream: `HalError::Disconnected` is its
    /// own stop class (a run may still be live on the hardware with no host link
    /// to command it), while a plain config refusal is not.
    pub fn is_device_gone(&self) -> bool {
        self.daqmx_code().is_some_and(crate::ffi::is_device_gone)
    }
}

impl fmt::Display for NiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NiError::DriverNotAvailable { tried, detail } => {
                write!(f, "NI-DAQmx driver not available (tried: {tried})")?;
                if !detail.is_empty() {
                    write!(f, ": {detail}")?;
                }
                Ok(())
            }
            NiError::MissingSymbol { symbol, detail } => {
                write!(f, "NI-DAQmx runtime is missing symbol `{symbol}`")?;
                if !detail.is_empty() {
                    write!(f, ": {detail}")?;
                }
                Ok(())
            }
            NiError::Daqmx {
                code,
                text,
                context,
            } => {
                write!(f, "NI-DAQmx error {code} during {context}")?;
                if !text.is_empty() {
                    write!(f, ": {text}")?;
                }
                Ok(())
            }
            NiError::Config(s) => write!(f, "NI backend configuration error: {s}"),
        }
    }
}

impl std::error::Error for NiError {}

impl From<NiError> for HalError {
    fn from(e: NiError) -> HalError {
        match &e {
            // No NI-DAQmx runtime on this host, and the HAL now has a variant
            // that SAYS SO. The first version mapped this to `Unsupported`, whose
            // `&'static str` payload threw away the loader diagnosis (which
            // library names were tried and why each failed) - so the one error
            // an operator most needs to act on arrived with its actionable half
            // deleted, and widening the HAL error type was recorded as
            // owed work. `DriverNotAvailable(String)` is that widening: the
            // detail survives the hop, and `Unsupported` goes back to meaning
            // "this backend does not offer that operation".
            NiError::DriverNotAvailable { .. } => HalError::DriverNotAvailable(e.to_string()),
            // A runtime that loaded but is missing an entry point is an
            // environment fault, and the symbol NAME is the whole diagnosis -
            // so this one keeps its string.
            NiError::MissingSymbol { .. } => HalError::Io(e.to_string()),
            // Device-gone codes are their own stop class upstream.
            NiError::Daqmx { .. } if e.is_device_gone() => HalError::Disconnected(e.to_string()),
            NiError::Daqmx { .. } => HalError::Io(e.to_string()),
            NiError::Config(_) => HalError::Config(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RED-MUTATION: map `DriverNotAvailable` back to
    /// `HalError::Unsupported("NI driver not available on this host")` (the original
    /// stopgap) and the two `contains` assertions below red - that mapping
    /// cannot carry the loader detail, which is the whole reason the HAL error
    /// type was widened.
    #[test]
    fn driver_absent_maps_to_driver_not_available_and_keeps_the_loader_detail() {
        let e = NiError::DriverNotAvailable {
            tried: "libnidaqmx.so.1".into(),
            detail: "no such file".into(),
        };
        // The NiError itself keeps the full loader diagnosis...
        let full = e.to_string();
        assert!(full.contains("libnidaqmx.so.1"), "{full}");
        assert!(full.contains("no such file"), "{full}");
        // ...and so does the HAL-level shape now: `DriverNotAvailable` owns a
        // String, so nothing is lost crossing the layer boundary.
        match HalError::from(e) {
            HalError::DriverNotAvailable(s) => {
                assert!(s.contains("libnidaqmx.so.1"), "{s}");
                assert!(s.contains("no such file"), "{s}");
            }
            other => panic!("expected DriverNotAvailable, got {other:?}"),
        }
    }

    #[test]
    fn missing_symbol_maps_to_io_and_names_the_symbol() {
        let h: HalError = NiError::MissingSymbol {
            symbol: "DAQmxGetWriteSpaceAvail",
            detail: "undefined symbol".into(),
        }
        .into();
        match h {
            HalError::Io(s) => assert!(s.contains("DAQmxGetWriteSpaceAvail"), "{s}"),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn config_maps_to_config() {
        let h: HalError = NiError::config("two rows claim the same physical channel").into();
        assert!(matches!(h, HalError::Config(_)), "{h:?}");
    }

    #[test]
    fn ordinary_driver_refusal_maps_to_io_not_disconnected() {
        let e = NiError::Daqmx {
            code: -200077,
            text: "Requested value is not a supported value".into(),
            context: "CfgSampClkTiming".into(),
        };
        assert!(!e.is_device_gone());
        let h: HalError = e.into();
        match h {
            HalError::Io(s) => assert!(s.contains("-200077"), "{s}"),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn device_gone_code_maps_to_disconnected() {
        // -88705 = "Specified device is not present or is not active in the
        // system" - the hot-unplug shape.
        let e = NiError::Daqmx {
            code: -88705,
            text: "Device not present".into(),
            context: "ReadAnalogF64".into(),
        };
        assert!(e.is_device_gone(), "-88705 must be in the device-gone set");
        let h: HalError = e.into();
        match h {
            HalError::Disconnected(s) => assert!(s.contains("-88705"), "{s}"),
            other => panic!("expected Disconnected, got {other:?}"),
        }
    }

    #[test]
    fn daqmx_code_accessor_only_answers_for_driver_errors() {
        assert_eq!(NiError::config("x").daqmx_code(), None);
        assert_eq!(
            NiError::Daqmx {
                code: -1,
                text: String::new(),
                context: String::new()
            }
            .daqmx_code(),
            Some(-1)
        );
    }
}
