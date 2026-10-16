//! Descriptive peer metadata. Identity and authorization never depend on the OS label.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperatingSystem {
    Macos,
    Windows,
    Linux,
    Other,
}

impl OperatingSystem {
    pub fn local() -> Self {
        match std::env::consts::OS {
            "macos" => Self::Macos,
            "windows" => Self::Windows,
            "linux" => Self::Linux,
            _ => Self::Other,
        }
    }

    pub(crate) fn encode(self) -> u8 {
        match self {
            Self::Macos => 0,
            Self::Windows => 1,
            Self::Linux => 2,
            Self::Other => 3,
        }
    }

    pub(crate) fn decode(value: u8) -> Result<Self, crate::Error> {
        match value {
            0 => Ok(Self::Macos),
            1 => Ok(Self::Windows),
            2 => Ok(Self::Linux),
            3 => Ok(Self::Other),
            _ => Err(crate::Error::Malformed),
        }
    }
}

/// Observed connection state, never inferred from cached addresses or relay registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DevicePresence {
    Connected,
    Connecting,
    Reconnecting,
    Disconnected,
}
