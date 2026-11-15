use std::io;
use std::path::Path;

#[cfg(any(windows, test))]
use base64::Engine;

pub(crate) fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// The command an agent runs to call klark's hook at `hook_path`.
///
/// On Windows this is the hook's 8.3 short path with forward slashes, unquoted
/// (`C:/Users/MUTHUI~1/.claude/hooks/klarkhk.cmd session`): the one form that
/// runs the same from cmd.exe, PowerShell and Git Bash (where Claude Code runs
/// hooks). It fails when no such path exists, so install never writes a broken
/// command.
pub(crate) fn hook_command(hook_path: &Path, action: Option<&str>) -> io::Result<String> {
    #[cfg(windows)]
    {
        windows_hook_command(hook_path, action)
    }

    #[cfg(not(windows))]
    {
        let mut command = format!(
            "bash {}",
            shell_single_quote(&hook_path.display().to_string())
        );
        if let Some(action) = action {
            command.push(' ');
            command.push_str(action);
        }
        Ok(command)
    }
}

/// Shortens only the hook's folder and appends the (8.3-clean) file name, so
/// the command is the same before the file exists, after it is written and
/// after it is removed.
#[cfg(windows)]
fn windows_hook_command(hook_path: &Path, action: Option<&str>) -> io::Result<String> {
    let (Some(dir), Some(name)) = (hook_path.parent(), hook_path.file_name()) else {
        return Err(io::Error::other(format!(
            "invalid hook path: {}",
            hook_path.display()
        )));
    };
    let short_dir = windows_short_path(dir)?;
    let short = format!(
        "{}\\{}",
        short_dir.trim_end_matches('\\'),
        name.to_string_lossy()
    );
    command_from_short_hook_path(&short, hook_path, action)
}

/// `GetShortPathNameW` for the longest existing ancestor of `path`, with the
/// missing rest appended unchanged.
#[cfg(windows)]
fn windows_short_path(path: &Path) -> io::Result<String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

    let mut existing = path;
    let mut missing = Vec::new();
    while !existing.exists() {
        let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) else {
            break;
        };
        missing.push(name.to_os_string());
        existing = parent;
    }

    let wide: Vec<u16> = existing
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is NUL-terminated; a null buffer of size 0 asks for the
    // required length.
    let needed = unsafe { GetShortPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0) };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buffer = vec![0u16; needed as usize];
    // SAFETY: `buffer` holds `needed` UTF-16 units, as passed.
    let written = unsafe { GetShortPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), needed) };
    if written == 0 || written >= needed {
        return Err(io::Error::last_os_error());
    }
    buffer.truncate(written as usize);
    let mut short = std::path::PathBuf::from(OsString::from_wide(&buffer));
    for name in missing.into_iter().rev() {
        short.push(name);
    }
    short
        .into_os_string()
        .into_string()
        .map_err(|_| io::Error::other(format!("{} is not valid Unicode", path.display())))
}

/// Builds the Windows hook command from an already shortened path. The path
/// goes into the agent's config unquoted, so every character must be safe
/// unquoted in cmd.exe, PowerShell and bash.
#[cfg(any(windows, test))]
pub(crate) fn command_from_short_hook_path(
    short_path: &str,
    hook_path: &Path,
    action: Option<&str>,
) -> io::Result<String> {
    // Verbatim prefixes: `\\?\C:\x` is `C:\x`, `\\?\UNC\host\share` is `\\host\share`.
    let short_path = match short_path.strip_prefix(r"\\?\UNC\") {
        Some(unc) => format!(r"\\{unc}"),
        None => short_path
            .strip_prefix(r"\\?\")
            .unwrap_or(short_path)
            .to_string(),
    };
    let forward = short_path.replace('\\', "/");
    let safe = !forward.is_empty()
        && forward.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '/' | ':' | '.' | '_' | '-' | '~')
        });
    if !safe {
        return Err(io::Error::other(format!(
            "cannot add klark's hook at {}: its short (8.3) path {forward} still has a space \
             or a character that is unsafe in a hook command (8.3 names may be turned off for \
             this drive). Use a config folder whose path has only letters, digits and . _ - ~",
            hook_path.display()
        )));
    }

    let mut command = forward;
    if let Some(action) = action {
        command.push(' ');
        command.push_str(action);
    }
    Ok(command)
}

/// The PowerShell `-File` command older Windows releases wrote. Kept so
/// install and uninstall can find and remove those entries.
#[cfg(any(windows, test))]
pub(crate) fn legacy_powershell_file_hook_command(
    hook_path: &Path,
    action: Option<&str>,
) -> String {
    let mut command = format!(
        "powershell -NoProfile -ExecutionPolicy Bypass -File {}",
        windows_command_quote(&hook_path.display().to_string())
    );
    if let Some(action) = action {
        command.push(' ');
        command.push_str(action);
    }
    command
}

/// The `-EncodedCommand` form older Windows releases wrote for MastraCode and
/// Antigravity CLI. Kept so install and uninstall can remove those entries.
#[cfg(any(windows, test))]
pub(crate) fn legacy_powershell_encoded_hook_command(hook_path: &Path, action: &str) -> String {
    let path = hook_path.display().to_string().replace('\'', "''");
    let script = format!("& '{path}' {action}");
    let encoded_script = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let encoded = base64::engine::general_purpose::STANDARD.encode(encoded_script);
    format!("powershell -NoProfile -ExecutionPolicy Bypass -EncodedCommand {encoded}")
}

pub(crate) fn legacy_bash_hook_command(hook_path: &Path, action: Option<&str>) -> String {
    let mut command = format!(
        "bash {}",
        shell_single_quote(&hook_path.display().to_string())
    );
    if let Some(action) = action {
        command.push(' ');
        command.push_str(action);
    }
    command
}

#[cfg(any(windows, test))]
fn windows_command_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}
