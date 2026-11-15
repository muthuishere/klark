mod actions;
mod claude_settings;
mod command;
mod config_edit;
mod config_file;
mod env;
mod file_ops;
mod hook_run;
mod opencode_config;
mod registry;
mod targets;
mod types;
mod version;

pub(crate) use actions::{
    install_experimental_letta, install_target, uninstall_experimental_letta, uninstall_target,
};
#[cfg(test)]
pub(crate) use env::integration_env_lock;
pub(crate) use env::{
    apply_pane_base_env, KLARK_PANE_ID_ENV_VAR, KLARK_TAB_ID_ENV_VAR, KLARK_WORKSPACE_ID_ENV_VAR,
};
pub(crate) use hook_run::run_hook_command;
pub(crate) use registry::{
    experimental_letta_integration_status, installed_integration_statuses,
    integration_recommendations, integration_target_label, print_outdated_update_notice,
};
pub(crate) use types::{
    ExperimentalIntegrationStatus, IntegrationRecommendation, IntegrationStatus,
    IntegrationStatusKind,
};

/// CLI labels for experimental integrations that are intentionally not part of
/// the frozen client endpoint `IntegrationTarget` enum. Empty this list once the
/// agent registry provides first-class target registration.
pub(crate) const EXPERIMENTAL_INTEGRATION_TARGET_LABELS: &[&str] = &["letta"];

/// Windows hook file name: 8.3-clean, so only its folders need a short alias
/// in the hook command (see `command::hook_command`).
const WINDOWS_HOOK_INSTALL_NAME: &str = "klarkhk.cmd";
/// PowerShell hook files older Windows releases installed; install and
/// uninstall remove them and the config entries that ran them.
#[cfg(windows)]
const LEGACY_POWERSHELL_HOOK_NAMES: [&str; 2] =
    ["klark-agent-state.ps1", "klark-agent-session.ps1"];

/// A Windows hook: a `.cmd` that hands the agent's arguments and stdin to
/// `klark integration hook <agent>`, which does the JSON work in Rust. No
/// parenthesised blocks, so a `)` or `&` in `KLARK_BIN_PATH` cannot break it.
/// The `json` form is for agents that need exactly one `{}` on stdout: klark
/// prints it when it runs (and exits 0); otherwise the script does. The
/// exit code is compared as text: `if not errorlevel 1` would also pass a
/// negative code from a crash.
#[cfg(any(windows, test))]
macro_rules! windows_cmd_hook {
    (@head $id:literal, $version:literal, $agent:literal) => {
        concat!(
            "@echo off\r\n",
            "setlocal\r\n",
            "rem installed by klark\r\n",
            "rem managed by klark; reinstalling or updating the integration overwrites this file.\r\n",
            "rem KLARK_INTEGRATION_ID=", $id, "\r\n",
            "rem KLARK_INTEGRATION_VERSION=", $version, "\r\n",
            "if not \"%KLARK_ENV%\"==\"1\" goto done\r\n",
            "set \"KLARK_HOOK_EXE=%KLARK_BIN_PATH%\"\r\n",
            "if not defined KLARK_HOOK_EXE set \"KLARK_HOOK_EXE=klark\"\r\n",
            "\"%KLARK_HOOK_EXE%\" integration hook ", $agent, " %* 2>nul\r\n",
        )
    };
    (json $id:literal, $version:literal, $agent:literal) => {
        concat!(
            windows_cmd_hook!(@head $id, $version, $agent),
            "if \"%errorlevel%\"==\"0\" exit /b 0\r\n",
            ":done\r\n",
            "echo {}\r\n",
            "exit /b 0\r\n",
        )
    };
    ($id:literal, $version:literal, $agent:literal) => {
        concat!(
            windows_cmd_hook!(@head $id, $version, $agent),
            ":done\r\n",
            "exit /b 0\r\n",
        )
    };
}

const PI_EXTENSION_INSTALL_NAME: &str = "klark-agent-state.ts";
const PI_EXTENSION_ASSET: &str = include_str!("assets/pi/klark-agent-state.ts");
const PI_INTEGRATION_VERSION: u32 = 9;
const OMP_EXTENSION_INSTALL_NAME: &str = "klark-omp-agent-state.ts";
const OMP_EXTENSION_ASSET: &str = include_str!("assets/omp/klark-agent-state.ts");
const OMP_INTEGRATION_VERSION: u32 = 10;
const CLAUDE_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    WINDOWS_HOOK_INSTALL_NAME
} else {
    "klark-agent-state.sh"
};
#[cfg(any(windows, test))]
const WINDOWS_CLAUDE_HOOK_ASSET: &str = windows_cmd_hook!("claude", "12", "claude");
#[cfg(windows)]
const CLAUDE_HOOK_ASSET: &str = WINDOWS_CLAUDE_HOOK_ASSET;
#[cfg(not(windows))]
const CLAUDE_HOOK_ASSET: &str = include_str!("assets/claude/klark-agent-state.sh");
const CLAUDE_INTEGRATION_VERSION: u32 = 12;
const CODEX_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    WINDOWS_HOOK_INSTALL_NAME
} else {
    "klark-agent-state.sh"
};
#[cfg(any(windows, test))]
const WINDOWS_CODEX_HOOK_ASSET: &str = windows_cmd_hook!("codex", "11", "codex");
#[cfg(windows)]
const CODEX_HOOK_ASSET: &str = WINDOWS_CODEX_HOOK_ASSET;
#[cfg(not(windows))]
const CODEX_HOOK_ASSET: &str = include_str!("assets/codex/klark-agent-state.sh");
const CODEX_INTEGRATION_VERSION: u32 = 11;
const KIMI_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    WINDOWS_HOOK_INSTALL_NAME
} else {
    "klark-agent-state.sh"
};
#[cfg(any(windows, test))]
const WINDOWS_KIMI_HOOK_ASSET: &str = windows_cmd_hook!("kimi", "9", "kimi");
#[cfg(windows)]
const KIMI_HOOK_ASSET: &str = WINDOWS_KIMI_HOOK_ASSET;
#[cfg(not(windows))]
const KIMI_HOOK_ASSET: &str = include_str!("assets/kimi/klark-agent-state.sh");
const KIMI_INTEGRATION_VERSION: u32 = 9;
const KIMI_CONFIG_BLOCK_BEGIN: &str = "# >>> klark kimi integration";
const KIMI_CONFIG_BLOCK_END: &str = "# <<< klark kimi integration";
const KIMI_MIN_VERSION: &str = "0.14.0";
const KIMI_ASK_USER_QUESTION_MATCHER: &str = "^AskUserQuestion$";
const KIMI_OTHER_TOOL_MATCHER: &str = "^(?!AskUserQuestion$).*$";
const KIMI_HOOK_EVENTS: [(&str, Option<&str>, &str); 12] = [
    ("SessionStart", None, "session"),
    ("UserPromptSubmit", None, "working"),
    ("PreToolUse", Some(KIMI_OTHER_TOOL_MATCHER), "working"),
    (
        "PreToolUse",
        Some(KIMI_ASK_USER_QUESTION_MATCHER),
        "blocked",
    ),
    (
        "PostToolUse",
        Some(KIMI_ASK_USER_QUESTION_MATCHER),
        "working",
    ),
    (
        "PostToolUseFailure",
        Some(KIMI_ASK_USER_QUESTION_MATCHER),
        "working",
    ),
    ("SubagentStart", None, "working"),
    ("PreCompact", None, "working"),
    ("PermissionRequest", None, "blocked"),
    ("PermissionResult", None, "working"),
    ("Stop", None, "idle"),
    ("Interrupt", None, "idle"),
];
const COPILOT_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    WINDOWS_HOOK_INSTALL_NAME
} else {
    "klark-agent-state.sh"
};
#[cfg(any(windows, test))]
const WINDOWS_COPILOT_HOOK_ASSET: &str = windows_cmd_hook!("copilot", "5", "copilot");
#[cfg(windows)]
const COPILOT_HOOK_ASSET: &str = WINDOWS_COPILOT_HOOK_ASSET;
#[cfg(not(windows))]
const COPILOT_HOOK_ASSET: &str = include_str!("assets/copilot/klark-agent-state.sh");
const COPILOT_INTEGRATION_VERSION: u32 = 5;
const COPILOT_HOOK_EVENTS: [&str; 1] = ["SessionStart"];
const COPILOT_REMOVED_LIFECYCLE_HOOK_EVENTS: [&str; 9] = [
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "Stop",
    "agentStop",
    "SessionEnd",
    "notification",
    "sessionStart",
];
const DEVIN_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    WINDOWS_HOOK_INSTALL_NAME
} else {
    "klark-agent-state.sh"
};
#[cfg(any(windows, test))]
const WINDOWS_DEVIN_HOOK_ASSET: &str = windows_cmd_hook!("devin", "4", "devin");
#[cfg(windows)]
const DEVIN_HOOK_ASSET: &str = WINDOWS_DEVIN_HOOK_ASSET;
#[cfg(not(windows))]
const DEVIN_HOOK_ASSET: &str = include_str!("assets/devin/klark-agent-state.sh");
const DEVIN_INTEGRATION_VERSION: u32 = 4;
const DEVIN_HOOK_EVENTS: [(&str, &str); 6] = [
    ("SessionStart", "session"),
    ("UserPromptSubmit", "session"),
    ("PreToolUse", "session"),
    ("PostToolUse", "session"),
    ("PermissionRequest", "session"),
    ("Stop", "session"),
];
const DEVIN_REMOVED_LIFECYCLE_HOOK_EVENTS: [(&str, &str); 6] = [
    ("UserPromptSubmit", "working"),
    ("PreToolUse", "working"),
    ("PostToolUse", "working"),
    ("PermissionRequest", "blocked"),
    ("Stop", "idle"),
    ("SessionEnd", "release"),
];
const DROID_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    WINDOWS_HOOK_INSTALL_NAME
} else {
    "klark-agent-state.sh"
};
#[cfg(any(windows, test))]
const WINDOWS_DROID_HOOK_ASSET: &str = windows_cmd_hook!("droid", "5", "droid");
#[cfg(windows)]
const DROID_HOOK_ASSET: &str = WINDOWS_DROID_HOOK_ASSET;
#[cfg(not(windows))]
const DROID_HOOK_ASSET: &str = include_str!("assets/droid/klark-agent-state.sh");
const DROID_INTEGRATION_VERSION: u32 = 5;
const DROID_HOOK_EVENTS: [(&str, &str); 1] = [("SessionStart", "session")];
const DROID_REMOVED_LIFECYCLE_HOOK_EVENTS: [(&str, &str); 9] = [
    ("SessionStart", "idle"),
    ("UserPromptSubmit", "working"),
    ("PreToolUse", "working"),
    ("PostToolUse", "working"),
    ("Notification", "blocked"),
    ("Stop", "idle"),
    ("SubagentStop", "working"),
    ("PreCompact", "working"),
    ("SessionEnd", "release"),
];
const OPENCODE_PLUGIN_INSTALL_NAME: &str = "klark-agent-state.js";
const OPENCODE_PLUGIN_ASSET: &str = include_str!("assets/opencode/klark-agent-state.js");
const OPENCODE_TUI_PLUGIN_INSTALL_NAME: &str = "klark-tui-session.js";
const OPENCODE_TUI_PLUGIN_SPEC: &str = "./klark-tui-session.js";
const OPENCODE_TUI_PLUGIN_ASSET: &str = include_str!("assets/opencode/klark-tui-session.js");
const OPENCODE_V2_TUI_PLUGIN_DIR: &str = "klark-opencode";
const OPENCODE_V2_TUI_PLUGIN_SPEC: &str = "./klark-opencode";
const OPENCODE_V2_TUI_PLUGIN_ASSET: &str = include_str!("assets/opencode/tui.js");
const OPENCODE_INTEGRATION_VERSION: u32 = 13;
const KILO_PLUGIN_INSTALL_NAME: &str = "klark-agent-state.js";
const KILO_PLUGIN_ASSET: &str = include_str!("assets/kilo/klark-agent-state.js");
const KILO_INTEGRATION_VERSION: u32 = 4;
const HERMES_PLUGIN_INSTALL_NAME: &str = "klark-agent-state";
const HERMES_PLUGIN_MANIFEST_INSTALL_NAME: &str = "plugin.yaml";
const HERMES_PLUGIN_INIT_INSTALL_NAME: &str = "__init__.py";
const HERMES_PLUGIN_MANIFEST_ASSET: &str = include_str!("assets/hermes/plugin.yaml");
const HERMES_PLUGIN_INIT_ASSET: &str = include_str!("assets/hermes/__init__.py");
const HERMES_INTEGRATION_VERSION: u32 = 5;
const QODERCLI_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    WINDOWS_HOOK_INSTALL_NAME
} else {
    "klark-agent-state.sh"
};
#[cfg(any(windows, test))]
const WINDOWS_QODERCLI_HOOK_ASSET: &str = windows_cmd_hook!("qodercli", "5", "qodercli");
#[cfg(windows)]
const QODERCLI_HOOK_ASSET: &str = WINDOWS_QODERCLI_HOOK_ASSET;
#[cfg(not(windows))]
const QODERCLI_HOOK_ASSET: &str = include_str!("assets/qodercli/klark-agent-state.sh");
const QODERCLI_INTEGRATION_VERSION: u32 = 5;
const QODERCLI_HOOK_EVENTS: [(&str, &str); 1] = [("SessionStart", "session")];
const QWEN_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    WINDOWS_HOOK_INSTALL_NAME
} else {
    "klark-agent-session.sh"
};
#[cfg(any(windows, test))]
const WINDOWS_QWEN_HOOK_ASSET: &str = windows_cmd_hook!("qwen", "3", "qwen");
#[cfg(windows)]
const QWEN_HOOK_ASSET: &str = WINDOWS_QWEN_HOOK_ASSET;
#[cfg(not(windows))]
const QWEN_HOOK_ASSET: &str = include_str!("assets/qwen/klark-agent-session.sh");
const QWEN_INTEGRATION_VERSION: u32 = 3;
const QWEN_HOOK_EVENTS: [(&str, &str); 1] = [("SessionStart", "session")];
const LETTA_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    WINDOWS_HOOK_INSTALL_NAME
} else {
    "klark-agent-session.sh"
};
#[cfg(any(windows, test))]
const WINDOWS_LETTA_HOOK_ASSET: &str = windows_cmd_hook!("letta", "3", "letta");
#[cfg(windows)]
const LETTA_HOOK_ASSET: &str = WINDOWS_LETTA_HOOK_ASSET;
#[cfg(not(windows))]
const LETTA_HOOK_ASSET: &str = include_str!("assets/letta/klark-agent-session.sh");
const LETTA_INTEGRATION_VERSION: u32 = 3;
const LETTA_HOOK_TIMEOUT_MS: u64 = 10_000;
const QODERCLI_REMOVED_LIFECYCLE_HOOK_EVENTS: [(&str, &str); 12] = [
    ("SessionStart", "idle"),
    ("UserPromptSubmit", "working"),
    ("PreToolUse", "working"),
    ("PostToolUse", "working"),
    ("PostToolUseFailure", "working"),
    ("SubagentStart", "working"),
    ("SubagentStop", "working"),
    ("PreCompact", "working"),
    ("Notification", "blocked"),
    ("PermissionRequest", "blocked"),
    ("Stop", "idle"),
    ("SessionEnd", "release"),
];
const CURSOR_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    WINDOWS_HOOK_INSTALL_NAME
} else {
    "klark-agent-state.sh"
};
#[cfg(any(windows, test))]
const WINDOWS_CURSOR_HOOK_ASSET: &str = windows_cmd_hook!(json "cursor", "3", "cursor");
#[cfg(windows)]
const CURSOR_HOOK_ASSET: &str = WINDOWS_CURSOR_HOOK_ASSET;
#[cfg(not(windows))]
const CURSOR_HOOK_ASSET: &str = include_str!("assets/cursor/klark-agent-state.sh");
const CURSOR_INTEGRATION_VERSION: u32 = 3;
#[cfg(windows)]
const ANTIGRAVITY_CLI_HOOK_INSTALL_NAME: &str = WINDOWS_HOOK_INSTALL_NAME;
#[cfg(not(windows))]
const ANTIGRAVITY_CLI_HOOK_INSTALL_NAME: &str = "klark-agent-state.sh";
#[cfg(any(windows, test))]
const WINDOWS_ANTIGRAVITY_CLI_HOOK_ASSET: &str =
    windows_cmd_hook!(json "antigravity_cli", "5", "antigravity_cli");
#[cfg(windows)]
const ANTIGRAVITY_CLI_HOOK_ASSET: &str = WINDOWS_ANTIGRAVITY_CLI_HOOK_ASSET;
#[cfg(not(windows))]
const ANTIGRAVITY_CLI_HOOK_ASSET: &str =
    include_str!("assets/antigravity_cli/klark-agent-state.sh");
const ANTIGRAVITY_CLI_INTEGRATION_VERSION: u32 = 5;
/// Antigravity CLI keys `hooks.json` by hook name, so every Klark entry lives
/// under one Klark-owned block that install rewrites and uninstall removes.
const ANTIGRAVITY_CLI_HOOK_BLOCK_NAME: &str = "klark";
const ANTIGRAVITY_CLI_HOOK_TIMEOUT_SEC: u64 = 10;
/// `(event, reported action)`. Session-only: `PreInvocation` is the only event
/// we need because it carries `conversationId`. The others cannot express
/// lifecycle safely — Antigravity CLI has no blocked event, `PostInvocation` is
/// skipped on interruption, and `Stop` is end-of-turn rather than process exit.
/// Screen detection owns agent state instead.
///
/// `PreInvocation` takes a flat handler list; only the `PreToolUse`/`PostToolUse`
/// events accept a `matcher`/`hooks` wrapper, and sending one here would
/// invalidate the whole file.
const ANTIGRAVITY_CLI_HOOK_EVENTS: [(&str, &str); 1] = [("PreInvocation", "session")];
const INTEGRATION_VERSION_MARKER: &str = "KLARK_INTEGRATION_VERSION=";
const MASTRACODE_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    WINDOWS_HOOK_INSTALL_NAME
} else {
    "klark-agent-state.sh"
};
#[cfg(any(windows, test))]
const WINDOWS_MASTRACODE_HOOK_ASSET: &str = windows_cmd_hook!("mastracode", "4", "mastracode");
#[cfg(windows)]
const MASTRACODE_HOOK_ASSET: &str = WINDOWS_MASTRACODE_HOOK_ASSET;
#[cfg(not(windows))]
const MASTRACODE_HOOK_ASSET: &str = include_str!("assets/mastracode/klark-agent-state.sh");
const MASTRACODE_INTEGRATION_VERSION: u32 = 4;
const MASTRACODE_HOOK_TIMEOUT_MS: u64 = 10_000;
const MASTRACODE_REMOVED_HOOK_EVENTS: [(&str, &str); 2] =
    [("SessionStart", "idle"), ("SessionEnd", "release")];
const MASTRACODE_HOOK_EVENTS: [(&str, &str); 11] = [
    ("SessionStart", "session"),
    ("UserPromptSubmit", "working"),
    ("AgentStart", "working"),
    ("PreToolUse", "working"),
    ("PermissionRequest", "blocked"),
    ("PermissionResult", "working"),
    ("SubagentStart", "working"),
    ("SubagentEnd", "working"),
    ("Interrupt", "idle"),
    ("AgentEnd", "idle"),
    ("Stop", "idle"),
];
const GROK_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    WINDOWS_HOOK_INSTALL_NAME
} else {
    "klark-agent-state.sh"
};
const GROK_HOOK_CONFIG_INSTALL_NAME: &str = "klark.json";
#[cfg(any(windows, test))]
const WINDOWS_GROK_HOOK_ASSET: &str = windows_cmd_hook!("grok", "4", "grok");
#[cfg(windows)]
const GROK_HOOK_ASSET: &str = WINDOWS_GROK_HOOK_ASSET;
#[cfg(not(windows))]
const GROK_HOOK_ASSET: &str = include_str!("assets/grok/klark-agent-state.sh");
const GROK_INTEGRATION_VERSION: u32 = 4;

pub(crate) const INSTALL_WARNING_PREFIX: &str = "warning:";

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
