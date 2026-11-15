//! `[[hooks.start]]` / `[[hooks.stop]]`: machine services klark starts with the
//! first session server and stops after the last one (issue #11). The
//! supervisor that runs them lives in `crate::hooks`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// What happens when a start hook's command ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    /// Leave it stopped.
    Never,
    /// Start it again when it exits with an error.
    OnFailure,
    /// Start it again whenever it ends (the default).
    #[default]
    Always,
}

/// One hook: a shell command klark runs and, for start hooks, supervises.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HookDef {
    /// Unique among all hooks; names the log file and the CLI target.
    pub name: String,
    /// The command, run through the user's shell (`sec run NAME -- cmd` works).
    pub run: String,
    /// Where it runs; `~` is the home folder. Default: the home folder.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Start hooks only.
    #[serde(default)]
    pub restart: RestartPolicy,
    /// Extra environment. Values are never logged or shown.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// `[hooks]`: the start and stop lists.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct HooksConfig {
    pub start: Vec<HookDef>,
    pub stop: Vec<HookDef>,
}

impl HooksConfig {
    pub fn is_empty(&self) -> bool {
        self.start.is_empty() && self.stop.is_empty()
    }

    /// Every hook with whether it is a start hook.
    pub fn all(&self) -> impl Iterator<Item = (&HookDef, bool)> {
        self.start
            .iter()
            .map(|h| (h, true))
            .chain(self.stop.iter().map(|h| (h, false)))
    }

    pub fn find(&self, name: &str) -> Option<(&HookDef, bool)> {
        self.all().find(|(h, _)| h.name == name)
    }

    /// Problems `klark config check` reports; a hook with one is not run.
    pub fn diagnostics(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for (hook, start) in self.all() {
            let kind = if start { "hooks.start" } else { "hooks.stop" };
            if let Err(e) = validate_name(&hook.name) {
                out.push(format!("{kind}: hook name {:?}: {e}", hook.name));
            } else if !seen.insert(hook.name.as_str()) {
                out.push(format!("{kind}: hook name {:?} is used twice", hook.name));
            }
            if hook.run.trim().is_empty() {
                out.push(format!("{kind}: hook {:?} has an empty run", hook.name));
            }
            if !start && hook.restart != RestartPolicy::Always {
                out.push(format!(
                    "hooks.stop: hook {:?}: restart applies to start hooks only",
                    hook.name
                ));
            }
            for key in hook.env.keys() {
                if key.is_empty() || key.contains('=') || key.contains('\0') {
                    out.push(format!(
                        "{kind}: hook {:?}: bad env name {key:?}",
                        hook.name
                    ));
                }
            }
        }
        out
    }

    /// The hooks that pass `diagnostics`, the ones the supervisor runs.
    pub fn valid(&self) -> HooksConfig {
        let mut seen = std::collections::BTreeSet::new();
        let keep = |h: &HookDef, seen: &mut std::collections::BTreeSet<String>| {
            validate_name(&h.name).is_ok()
                && !h.run.trim().is_empty()
                && seen.insert(h.name.clone())
        };
        let start = self
            .start
            .iter()
            .filter(|h| keep(h, &mut seen))
            .cloned()
            .collect();
        let stop = self
            .stop
            .iter()
            .filter(|h| keep(h, &mut seen))
            .cloned()
            .collect();
        HooksConfig { start, stop }
    }
}

/// Letters, digits, `.`, `_` and `-`, at most 64, not starting with `.`.
pub fn validate_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() || name.len() > 64 {
        return Err("must be 1-64 characters");
    }
    if name.starts_with('.') {
        return Err("must not start with '.'");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err("may only contain letters, digits, '.', '_' and '-'");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_start_and_stop_hooks() {
        let c: HooksConfig = toml::from_str(
            r#"
[[start]]
name = "os-dashboard"
run = "bun run serve"
cwd = "~/work/company-os"
restart = "always"
env = { PORT = "8788" }

[[stop]]
name = "flush"
run = "./scripts/flush.sh"
"#,
        )
        .unwrap();
        assert_eq!(c.start[0].restart, RestartPolicy::Always);
        assert_eq!(c.start[0].env["PORT"], "8788");
        assert_eq!(c.stop[0].restart, RestartPolicy::Always);
        assert!(c.diagnostics().is_empty());
    }

    #[test]
    fn reports_bad_names_duplicates_and_empty_runs() {
        let c: HooksConfig = toml::from_str(
            r#"
[[start]]
name = "a"
run = "x"
[[start]]
name = "a"
run = "y"
[[stop]]
name = "bad name"
run = " "
restart = "never"
"#,
        )
        .unwrap();
        let d = c.diagnostics().join("\n");
        assert!(d.contains("used twice"), "{d}");
        assert!(d.contains("letters, digits"), "{d}");
        assert!(d.contains("empty run"), "{d}");
        assert!(d.contains("start hooks only"), "{d}");
        let v = c.valid();
        assert_eq!(v.start.len(), 1);
        assert!(v.stop.is_empty());
    }

    #[test]
    fn unknown_keys_are_refused() {
        assert!(
            toml::from_str::<HooksConfig>("[[start]]\nname=\"a\"\nrun=\"x\"\nport=1\n").is_err()
        );
    }
}
