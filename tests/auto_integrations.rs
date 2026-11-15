//! klark adds its hook to agent CLIs on start, and leaves a removed hook removed.
//!
//! Everything runs under a scratch HOME with every agent config-dir variable
//! removed, so nothing here can reach the real agent configs.

#![cfg(unix)]

pub mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use support::{
    cleanup_test_base, register_runtime_dir, register_spawned_klark_pid,
    unregister_spawned_klark_pid, wait_for_socket, wait_until,
};

/// Every variable klark reads to find an agent's config folder.
const AGENT_DIR_VARS: &[&str] = &[
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "KIMI_CODE_HOME",
    "COPILOT_HOME",
    "QODER_CONFIG_DIR",
    "QWEN_HOME",
    "CURSOR_CONFIG_DIR",
    "ANTIGRAVITY_CLI_CONFIG_DIR",
    "GROK_CONFIG_DIR",
    "GROK_HOME",
    "HERMES_HOME",
    "OPENCODE_CONFIG_DIR",
    "PI_CODING_AGENT_DIR",
    "PI_CONFIG_DIR",
];

fn app_dir_name() -> &'static str {
    if cfg!(debug_assertions) {
        "klark-dev"
    } else {
        "klark"
    }
}

/// A fake agent CLI on PATH, plus the config folder its first run creates.
fn add_fake_agent(base: &Path, name: &str) {
    fs::create_dir_all(base.join("home").join(format!(".{name}"))).unwrap();
    let path = base.join("bin").join(name);
    fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

struct Spawned {
    _master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl Drop for Spawned {
    fn drop(&mut self) {
        let pid = self.child.process_id();
        support::stop_spawned_klark(&mut *self.child);
        let _ = self.child.wait();
        unregister_spawned_klark_pid(pid);
    }
}

struct Sandbox {
    base: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let base = PathBuf::from(format!(
            "/tmp/klark-auto-int-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(base.join("home")).unwrap();
        fs::create_dir_all(base.join("bin")).unwrap();
        fs::create_dir_all(base.join("config").join(app_dir_name())).unwrap();
        fs::create_dir_all(base.join("runtime")).unwrap();
        fs::write(
            base.join("config").join(app_dir_name()).join("config.toml"),
            "onboarding = false\n",
        )
        .unwrap();
        add_fake_agent(&base, "claude");
        register_runtime_dir(&base.join("runtime"));
        Self { base }
    }

    fn api_socket(&self) -> PathBuf {
        self.base.join("runtime").join("klark.sock")
    }

    fn claude_settings(&self) -> PathBuf {
        self.base.join("home").join(".claude").join("settings.json")
    }

    fn remembered(&self) -> PathBuf {
        self.base
            .join("state")
            .join(app_dir_name())
            .join("integrations-auto.json")
    }

    fn logs(&self) -> String {
        let dir = self.base.join("config").join(app_dir_name());
        ["klark-client.log", "klark-server.log"]
            .iter()
            .map(|name| {
                let text = fs::read_to_string(dir.join(name)).unwrap_or_default();
                let tail: Vec<&str> = text.lines().rev().take(15).collect();
                format!(
                    "--- {name}\n{}",
                    tail.into_iter().rev().collect::<Vec<_>>().join("\n")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn command(&self, args: &[&str]) -> CommandBuilder {
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_klark"));
        support::isolate_klark_test_process(&mut cmd);
        // This test is about the automatic install, so turn it back on.
        cmd.env_remove("KLARK_NO_AUTO_INTEGRATIONS");
        for var in AGENT_DIR_VARS {
            cmd.env_remove(var);
        }
        cmd.args(args);
        cmd.env("HOME", self.base.join("home"));
        cmd.env(
            "PATH",
            format!("{}:/usr/bin:/bin", self.base.join("bin").display()),
        );
        cmd.env("XDG_CONFIG_HOME", self.base.join("config"));
        cmd.env("XDG_STATE_HOME", self.base.join("state"));
        cmd.env("XDG_RUNTIME_DIR", self.base.join("runtime"));
        cmd.env("KLARK_SOCKET_PATH", self.api_socket());
        cmd.env_remove("KLARK_CLIENT_SOCKET_PATH");
        cmd.env_remove("KLARK_ENV");
        cmd.env("KLARK_DISABLE_SOUND", "1");
        cmd.env("SHELL", "/bin/sh");
        cmd
    }

    fn spawn(&self, args: &[&str]) -> Spawned {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let child = pair.slave.spawn_command(self.command(args)).unwrap();
        register_spawned_klark_pid(child.process_id());
        drop(pair.slave);
        // Read the client's output, or it blocks on a full pty buffer.
        let mut reader = pair.master.try_clone_reader().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while matches!(std::io::Read::read(&mut reader, &mut buf), Ok(n) if n > 0) {}
        });
        Spawned {
            _master: pair.master,
            child,
        }
    }

    fn run(&self, args: &[&str]) {
        let mut child = self.spawn(args);
        let _ = child.child.wait();
    }
}

fn has_klark_hook(settings: &Path) -> bool {
    fs::read_to_string(settings).is_ok_and(|text| text.contains("klark-agent-state"))
}

#[test]
fn start_adds_missing_hook_and_respects_removal() {
    let sandbox = Sandbox::new();
    let server = sandbox.spawn(&["server"]);
    wait_for_socket(&sandbox.api_socket(), Duration::from_secs(10));
    wait_for_socket(
        &sandbox.base.join("runtime").join("klark-client.sock"),
        Duration::from_secs(10),
    );

    // First start: claude is on PATH with no hook, so klark adds it.
    let client = sandbox.spawn(&["client"]);
    assert!(
        wait_until(Duration::from_secs(15), Duration::from_millis(100), || {
            has_klark_hook(&sandbox.claude_settings())
                && fs::read_to_string(sandbox.remembered()).is_ok_and(|t| t.contains("claude"))
        }),
        "klark should add its hook to claude and remember it\n{}",
        sandbox.logs()
    );
    drop(client);

    // The user removes the hook, and installs codex. The next start adds
    // codex (proof the check ran) but not claude.
    sandbox.run(&["integration", "uninstall", "claude"]);
    assert!(!has_klark_hook(&sandbox.claude_settings()));
    add_fake_agent(&sandbox.base, "codex");
    let codex_hooks = sandbox.base.join("home").join(".codex").join("hooks.json");
    let client = sandbox.spawn(&["client"]);
    assert!(
        wait_until(Duration::from_secs(15), Duration::from_millis(100), || {
            has_klark_hook(&codex_hooks)
        }),
        "klark should add its hook to the newly installed codex"
    );
    assert!(
        !has_klark_hook(&sandbox.claude_settings()),
        "a hook the user removed must stay removed"
    );
    drop(client);

    drop(server);
    cleanup_test_base(&sandbox.base);
}
