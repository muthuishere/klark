//! `system.stats`: resource use of this server, the host, and each agent pane.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use crate::api::schema::{
    ResponseResult, SystemStatsAgent, SystemStatsHost, SystemStatsKlark, SystemStatsPanes,
};
use crate::app::App;
use crate::platform::sysstats;

use super::responses::encode_success;

/// CPU readings from the previous call, so each call reports the share since then.
struct PreviousSample {
    at: Instant,
    klark_cpu_ns: Option<u64>,
    pane_cpu_ns: HashMap<String, u64>,
}

static PREVIOUS: Mutex<Option<PreviousSample>> = Mutex::new(None);

impl App {
    pub(super) fn handle_system_stats(&mut self, id: String) -> String {
        let now = Instant::now();
        let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Agent panes: public id, detected name, state, shell pid.
        let mut pane_count = 0u32;
        let mut agents = Vec::new();
        for (ws_idx, ws) in self.state.workspaces.iter().enumerate() {
            for tab in &ws.tabs {
                for pane_id in tab.layout.pane_ids() {
                    pane_count += 1;
                    let Some(info) = self.agent_info(ws_idx, pane_id) else {
                        continue;
                    };
                    let shell_pid = self
                        .lookup_runtime_sender(ws_idx, pane_id)
                        .and_then(|runtime| runtime.child_pid());
                    let name = info
                        .agent
                        .clone()
                        .or_else(|| info.display_agent.clone())
                        .or_else(|| info.name.clone())
                        .unwrap_or_default();
                    agents.push((info.pane_id, name, info.agent_status, shell_pid));
                }
            }
        }

        // One process-table scan covers every pane.
        let roots: Vec<u32> = agents.iter().filter_map(|a| a.3).collect();
        let mut usages = sysstats::tree_usages(&roots).into_iter();
        let me = sysstats::self_usage();
        let host = sysstats::host_usage();

        let mut previous = PREVIOUS.lock().unwrap_or_else(|err| err.into_inner());
        let wall_ns = previous
            .as_ref()
            .map(|prev| now.duration_since(prev.at).as_nanos() as u64);
        let percent = |prev: Option<u64>, current: Option<u64>| match (wall_ns, prev, current) {
            (Some(wall), Some(prev), Some(current)) => sysstats::cpu_percent(prev, current, wall),
            _ => 0.0,
        };

        let klark_cpu = percent(
            previous.as_ref().and_then(|prev| prev.klark_cpu_ns),
            me.usage.cpu_ns,
        );
        let mut pane_cpu_ns = HashMap::new();
        let agents: Vec<SystemStatsAgent> = agents
            .into_iter()
            .map(|(pane_id, name, state, shell_pid)| {
                let usage = shell_pid.and_then(|_| usages.next()).unwrap_or_default();
                if let Some(cpu) = usage.cpu_ns {
                    pane_cpu_ns.insert(pane_id.clone(), cpu);
                }
                let prev = previous
                    .as_ref()
                    .and_then(|prev| prev.pane_cpu_ns.get(&pane_id).copied());
                SystemStatsAgent {
                    cpu_percent: percent(prev, usage.cpu_ns),
                    pane_id,
                    name,
                    state,
                    rss_bytes: usage.rss_bytes,
                }
            })
            .collect();
        *previous = Some(PreviousSample {
            at: now,
            klark_cpu_ns: me.usage.cpu_ns,
            pane_cpu_ns,
        });
        drop(previous);

        let agent_count = agents.len() as u32;
        encode_success(
            id,
            ResponseResult::SystemStats {
                ts_ms,
                klark: SystemStatsKlark {
                    pid: std::process::id(),
                    rss_bytes: me.usage.rss_bytes,
                    peak_rss_bytes: me.peak_rss_bytes,
                    cpu_percent: klark_cpu,
                    uptime_s: sysstats::uptime_s(),
                    version: crate::build_info::version(),
                },
                host: SystemStatsHost {
                    mem_total_bytes: host.mem_total_bytes,
                    mem_available_bytes: host.mem_available_bytes,
                    load1: host.load1,
                    cpus: host.cpus,
                },
                panes: SystemStatsPanes {
                    count: pane_count,
                    agents: agent_count,
                },
                agents,
            },
        )
    }
}
