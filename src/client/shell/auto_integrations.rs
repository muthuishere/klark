//! When klark opens: add klark's hook to every agent CLI on this machine.
//!
//! An agent tells klark which conversation it is in through klark's hook
//! (`klark integration install <agent>`). Without it klark cannot resume the
//! agent after a restart. So once per klark start, on the local machine, the
//! client lists the integrations and installs every one whose agent is
//! installed but has no hook yet, and every outdated one. Agents installed
//! later are picked up at the next start.
//!
//! A hook klark added and the user then removed is not added again: the
//! targets klark installed are remembered in `<state>/integrations-auto.json`.
//! `[advanced] auto_install_integrations = false` or
//! `KLARK_NO_AUTO_INTEGRATIONS=1` turns the check off.

use super::*;

#[derive(Debug, Default)]
pub(super) struct AutoIntegrations {
    /// The check ran (or was skipped) for this klark start.
    started: bool,
    pending: usize,
    installed: Vec<String>,
    failed: Vec<String>,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Remembered {
    #[serde(default)]
    installed: Vec<String>,
}

fn remembered_path() -> std::path::PathBuf {
    crate::config::state_dir().join("integrations-auto.json")
}

fn remembered() -> Remembered {
    std::fs::read(remembered_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Record targets that have klark's hook, so a later removal is respected.
fn remember(names: &[String]) {
    let mut r = remembered();
    let before = r.installed.len();
    for name in names {
        if !r.installed.iter().any(|n| n == name) {
            r.installed.push(name.clone());
        }
    }
    if r.installed.len() == before {
        return;
    }
    let path = remembered_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&r) {
        let _ = std::fs::write(path, bytes);
    }
}

/// The integration's stable name (`claude`, `antigravity-cli`, ...).
fn target_name(target: crate::api::schema::IntegrationTarget) -> String {
    serde_json::to_value(target)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{target:?}").to_lowercase())
}

/// Which integrations to install: outdated ones, and those whose agent is
/// installed, with no hook, that klark did not add before (the user removed it).
pub(super) fn targets_to_install(
    list: &[crate::api::schema::IntegrationInfo],
    remembered: &[String],
) -> Vec<(crate::api::schema::IntegrationTarget, String, String)> {
    use crate::api::schema::IntegrationState;
    list.iter()
        .filter_map(|info| {
            let name = target_name(info.target);
            let install = match info.state {
                IntegrationState::Outdated => true,
                IntegrationState::NotInstalled => info.available && !remembered.contains(&name),
                IntegrationState::Current => false,
            };
            install.then(|| (info.target, info.label.clone(), name))
        })
        .collect()
}

/// The install error every target returns when the agent's config folder is
/// missing: "<agent> config directory not found at <path>. install <agent> first"
/// (src/integration/targets.rs).
fn agent_not_set_up(message: &str) -> bool {
    message.contains(" not found at ") && message.trim_end().ends_with(" first")
}

impl ClientShellState {
    /// Called on the client's tick. Starts the check once, as soon as the
    /// local machine's session is shown.
    pub(crate) fn tick_auto_integrations(&mut self) -> Vec<ClientShellAction> {
        if self.auto_integrations.started || self.snapshot.is_none() {
            return Vec::new();
        }
        self.auto_integrations.started = true;
        // Windows hooks are still PowerShell scripts; they are not installed
        // unasked until they become .cmd hooks that call klark.exe.
        if cfg!(windows)
            || !self.config.auto_install_integrations
            || std::env::var_os("KLARK_NO_AUTO_INTEGRATIONS").is_some()
            || !self.active_endpoint_id.is_local()
        {
            return Vec::new();
        }
        let mut outcome = ClientShellInput::default();
        self.push_endpoint_method_with_kind(
            crate::api::schema::Method::IntegrationList(crate::api::schema::EmptyParams::default()),
            PendingEndpointKind::AutoIntegrationList,
            &mut outcome,
        );
        outcome.actions
    }

    pub(super) fn handle_auto_integration_result(
        &mut self,
        kind: PendingEndpointKind,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        use crate::api::schema::ResponseResult;
        let mut outcome = ClientShellInput::default();
        match kind {
            PendingEndpointKind::AutoIntegrationList => {
                let Ok(ResponseResult::IntegrationList { integrations }) = result else {
                    return (false, Vec::new());
                };
                // Hooks already in place (installed by hand, or earlier) count as
                // added: if the user removes one later, it stays removed.
                let current: Vec<String> = integrations
                    .iter()
                    .filter(|i| i.state == crate::api::schema::IntegrationState::Current)
                    .map(|i| target_name(i.target))
                    .collect();
                remember(&current);
                let remembered = remembered().installed;
                for (target, label, name) in targets_to_install(&integrations, &remembered) {
                    if self.push_endpoint_method_with_kind(
                        crate::api::schema::Method::IntegrationInstall(
                            crate::api::schema::IntegrationInstallParams { target },
                        ),
                        PendingEndpointKind::AutoIntegrationInstall { label, name },
                        &mut outcome,
                    ) {
                        self.auto_integrations.pending += 1;
                    }
                }
                (false, outcome.actions)
            }
            PendingEndpointKind::AutoIntegrationInstall { label, name } => {
                self.auto_integrations.pending = self.auto_integrations.pending.saturating_sub(1);
                match result {
                    Ok(ResponseResult::IntegrationInstall { .. }) => {
                        remember(std::slice::from_ref(&name));
                        self.auto_integrations.installed.push(label);
                    }
                    Ok(_) => self.auto_integrations.failed.push(label),
                    // The agent is on PATH but has never run, so its config folder
                    // does not exist yet. Not a failure: try again at the next start.
                    Err(error) if agent_not_set_up(&error.message) => {}
                    Err(error) => self
                        .auto_integrations
                        .failed
                        .push(format!("{label} ({})", error.message)),
                }
                if self.auto_integrations.pending > 0 {
                    return (false, Vec::new());
                }
                self.finish_auto_integrations(&mut outcome);
                (true, outcome.actions)
            }
            _ => (false, Vec::new()),
        }
    }

    /// One notice for the whole check; the integrations panel when something failed.
    fn finish_auto_integrations(&mut self, outcome: &mut ClientShellInput) {
        let installed = std::mem::take(&mut self.auto_integrations.installed);
        let failed = std::mem::take(&mut self.auto_integrations.failed);
        if !failed.is_empty() {
            if self.overlay.is_none() {
                self.open_settings_overlay();
                self.select_settings_section(ClientSettingsSection::Integrations, outcome);
            }
            self.set_endpoint_error(format!(
                "klark could not add its hook to {}; see settings → integrations",
                failed.join(", ")
            ));
        } else if !installed.is_empty() {
            self.set_endpoint_error(format!(
                "klark added its hook to {}: these agents now resume after a restart",
                installed.join(", ")
            ));
        }
        outcome.repaint = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::{IntegrationInfo, IntegrationState, IntegrationTarget};

    fn info(
        target: IntegrationTarget,
        available: bool,
        state: IntegrationState,
    ) -> IntegrationInfo {
        IntegrationInfo {
            target,
            label: target_name(target),
            command: target_name(target),
            available,
            state,
        }
    }

    #[test]
    fn missing_config_folder_is_not_a_failure() {
        assert!(agent_not_set_up(
            "claude directory not found at /home/u/.claude. install claude code first"
        ));
        assert!(agent_not_set_up(
            "codex config directory not found at C:\\Users\\u\\.codex. install codex first"
        ));
        assert!(!agent_not_set_up("permission denied (os error 13)"));
    }

    #[test]
    fn installs_missing_available_and_outdated_but_not_removed_ones() {
        let list = vec![
            info(
                IntegrationTarget::Claude,
                true,
                IntegrationState::NotInstalled,
            ),
            info(IntegrationTarget::Codex, true, IntegrationState::Outdated),
            info(
                IntegrationTarget::Devin,
                false,
                IntegrationState::NotInstalled,
            ),
            info(IntegrationTarget::Kimi, true, IntegrationState::Current),
            info(
                IntegrationTarget::Opencode,
                true,
                IntegrationState::NotInstalled,
            ),
        ];
        // opencode's hook was added before and then removed by the user.
        let names: Vec<String> = targets_to_install(&list, &["opencode".to_string()])
            .into_iter()
            .map(|(_, _, name)| name)
            .collect();
        assert_eq!(names, vec!["claude", "codex"]);
    }
}
