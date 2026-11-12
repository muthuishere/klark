use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct PingParams {}

crate::cli::projection::cli_params! {
    #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
    pub struct ServerLiveHandoffParams => ServerLiveHandoffProjection {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub import_exe: Option<String> => crate::cli::projection::Field::new(None).input(
            crate::cli::projection::Input::value("import-exe", "path", |slot, value| {
                *slot = Some(value.to_owned());
                Ok(())
            }).path()
        ),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub expected_protocol: Option<u32> => crate::cli::projection::Field::new(None).input(
            crate::cli::projection::Input::value("expected-protocol", "n", |slot, value| {
                *slot = Some(value.parse::<u32>().map_err(|_| {
                    crate::cli::projection::ParseError::Usage(format!("invalid expected protocol: {value}"))
                })?);
                Ok(())
            })
        ),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub expected_version: Option<String> => crate::cli::projection::Field::new(None).input(
            crate::cli::projection::Input::value("expected-version", "version", |slot, value| {
                *slot = Some(value.to_owned());
                Ok(())
            })
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ServerSshAgentRegisterParams {
    /// Absolute remote-host agent socket. Registration lasts until this API connection closes.
    pub socket_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ServerCapabilities {
    pub live_handoff: bool,
    #[serde(default)]
    pub detached_server_daemon: bool,
    /// Stable client-owned endpoint generation supported by this server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_protocol_generation: Option<u32>,
    /// Whether this server supports explicit client-shell surface interest.
    #[serde(default)]
    pub surface_interest: bool,
    /// Whether this server supports endpoint health probes.
    #[serde(default)]
    pub health_check: bool,
    /// Supports connection-scoped `server.ssh_agent.register` on the local JSON API.
    #[serde(default)]
    pub ssh_agent_registration: bool,
}

/// This klark server process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SystemStatsKlark {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rss_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_rss_bytes: Option<u64>,
    /// CPU use since the previous `system.stats` call, in percent of one core; 0 on the first call.
    pub cpu_percent: f64,
    pub uptime_s: u64,
    pub version: String,
}

/// The machine klark runs on. A field is left out when the OS cannot tell.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SystemStatsHost {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_total_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_available_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load1: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SystemStatsPanes {
    pub count: u32,
    pub agents: u32,
}

/// One agent pane's resource use: the process tree under its shell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SystemStatsAgent {
    pub pane_id: String,
    /// Detected agent name (for example `claude`).
    pub name: String,
    pub state: super::AgentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rss_bytes: Option<u64>,
    /// CPU use since the previous call, in percent of one core; 0 on the first call.
    pub cpu_percent: f64,
}
