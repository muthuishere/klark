#[derive(Clone, Copy)]
pub(crate) enum ConfigEdit<'a> {
    Theme(&'a str),
    StatusIndicators(super::StatusIndicatorStyle),
    Sound(bool),
    ToastDelivery(super::ToastDelivery),
    /// One `[feeds]` key and its value, already written as TOML.
    FeedSetting {
        key: &'a str,
        value: &'a str,
    },
}

impl ConfigEdit<'_> {
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Theme(_) => "theme",
            Self::StatusIndicators(_) => "status indicators",
            Self::Sound(_) => "sound setting",
            Self::ToastDelivery(_) => "toast setting",
            Self::FeedSetting { .. } => "feed setting",
        }
    }

    pub(crate) fn apply(self, content: &str) -> String {
        match self {
            Self::Theme(name) => {
                let content =
                    super::upsert_section_value(content, "theme", "name", &format!("\"{name}\""));
                super::upsert_section_bool(&content, "theme", "auto_switch", false)
            }
            Self::StatusIndicators(style) => super::upsert_section_value(
                content,
                "ui",
                "status_indicators",
                &format!("\"{}\"", style.as_str()),
            ),
            Self::Sound(enabled) => {
                super::upsert_section_bool(content, "ui.sound", "enabled", enabled)
            }
            Self::ToastDelivery(delivery) => {
                let value = match delivery {
                    super::ToastDelivery::Off => "\"off\"",
                    super::ToastDelivery::Klark => "\"klark\"",
                    super::ToastDelivery::Terminal => "\"terminal\"",
                    super::ToastDelivery::System => "\"system\"",
                };
                let content = super::upsert_section_value(content, "ui.toast", "delivery", value);
                super::remove_section_key(&content, "ui.toast", "enabled")
            }
            Self::FeedSetting { key, value } => {
                super::upsert_section_value(content, "feeds", key, value)
            }
        }
    }
}

pub(crate) fn update_file_at(
    path: &std::path::Path,
    description: &str,
    update: impl FnOnce(&str) -> Result<String, String>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create config directory: {error}"))?;
    }
    let content = match super::io::read_optional_config(path) {
        Ok(Some(content)) => content,
        Ok(None) => String::new(),
        Err(error) => {
            return Err(format!(
                "failed to read config before saving {description}: {error}"
            ));
        }
    };
    let updated = update(&content)?;
    std::fs::write(path, updated).map_err(|error| format!("failed to save {description}: {error}"))
}

impl ConfigEdit<'_> {
    /// `apply`, refused when it would turn a valid file invalid (a `[feeds]`
    /// written as dotted keys or an inline table, which the line editor does
    /// not recognise): an invalid file costs every setting, not just this one.
    fn checked_apply(&self, content: &str) -> Result<String, String> {
        let updated = self.apply(content);
        if matches!(self, Self::FeedSetting { .. })
            && toml::from_str::<toml::Table>(content).is_ok()
            && toml::from_str::<toml::Table>(&updated).is_err()
        {
            return Err(format!(
                "cannot save {}: [feeds] in {} is written in a form this command does not edit; change it by hand",
                self.description(),
                super::config_path().display()
            ));
        }
        Ok(updated)
    }
}

pub(crate) fn write_edit(edit: ConfigEdit<'_>) -> Result<(), String> {
    update_file_at(&super::config_path(), edit.description(), |content| {
        edit.checked_apply(content)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_set(content: &str, key: &str, value: &str) -> Result<String, String> {
        ConfigEdit::FeedSetting { key, value }.checked_apply(content)
    }

    /// `feed set` keeps comments, other sections, and valid TOML: a header
    /// with a comment is the same section, not a second `[feeds]`.
    #[test]
    fn feed_setting_keeps_the_file_valid() {
        let src = "# mine\n[feeds] # defaults\nrestore = true # keep\n\n[terminal] # shell\ndefault_shell = \"/bin/zsh\"\n";
        let out = feed_set(src, "max_bytes", "300").unwrap();
        let t: toml::Table = toml::from_str(&out).expect(&out);
        assert_eq!(t["feeds"]["max_bytes"].as_integer(), Some(300), "{out}");
        assert_eq!(t["feeds"]["restore"].as_bool(), Some(true), "{out}");
        assert!(t["terminal"].get("max_bytes").is_none(), "{out}");
        assert!(out.contains("# mine") && out.contains("# keep"), "{out}");
        let out = feed_set(&out, "restore", "false").unwrap();
        let t: toml::Table = toml::from_str(&out).expect(&out);
        assert_eq!(t["feeds"]["restore"].as_bool(), Some(false), "{out}");
        // A form the line editor does not know: refused, not corrupted.
        assert!(feed_set("feeds.restore = true\n", "restore", "false").is_err());
        assert!(feed_set("feeds = { restore = true }\n", "restore", "false").is_err());
    }

    #[test]
    fn update_file_at_does_not_move_a_leading_bom_into_the_file() {
        let dir = std::env::temp_dir().join(format!("klark-config-bom-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            b"\xEF\xBB\xBF[terminal]\ndefault_shell = \"pwsh.exe\"\n",
        )
        .unwrap();

        update_file_at(&path, "onboarding setting", |content| {
            Ok(crate::config::upsert_top_level_bool(
                content,
                "onboarding",
                false,
            ))
        })
        .unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_dir_all(dir);

        assert!(
            !written.contains('\u{feff}'),
            "unexpected BOM in {written:?}"
        );
        assert!(
            toml::from_str::<toml::Value>(&written).is_ok(),
            "written config is not valid TOML: {written:?}"
        );
    }
}
