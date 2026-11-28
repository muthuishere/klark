use std::path::PathBuf;

use tracing::{info, warn};

#[cfg(windows)]
use crate::protocol::ClientInputEvent;
use crate::protocol::MAX_CLIPBOARD_IMAGE_PAYLOAD;
use crate::protocol::{ClientClipboardImageTarget, ClientMessage};

use super::{write_to_server, ClientError};

pub(super) fn write_remote_image_to_server(
    stream: &mut impl super::ClientMessageSink,
    target: ClientClipboardImageTarget,
    image: crate::platform::ClipboardImage,
    source: &'static str,
) -> Result<(), ClientError> {
    if image.bytes.len() > MAX_CLIPBOARD_IMAGE_PAYLOAD {
        warn!(
            bytes = image.bytes.len(),
            max = MAX_CLIPBOARD_IMAGE_PAYLOAD,
            source,
            "local image is too large to bridge"
        );
        return Ok(());
    }

    info!(
        bytes = image.bytes.len(),
        extension = image.extension,
        source,
        "bridging local image to remote server"
    );
    write_to_server(
        stream,
        &ClientMessage::ClipboardImage {
            target,
            extension: image.extension.to_owned(),
            data: image.bytes,
        },
    )
    .map_err(ClientError::ConnectionLost)
}

pub(super) fn client_remote_image_paste_key(
    config: &crate::config::Config,
) -> Option<(crossterm::event::KeyCode, crossterm::event::KeyModifiers)> {
    match config.remote_image_paste_key() {
        Ok(key) => key,
        Err(diagnostic) => {
            warn!(diagnostic = %diagnostic, "local remote image paste key config diagnostic");
            None
        }
    }
}

pub(super) fn endpoint_accepts_local_images(
    remote_client_process: bool,
    endpoint_id: &super::endpoint::ClientEndpointId,
    active_surface_available: bool,
) -> bool {
    active_surface_available && (remote_client_process || !endpoint_id.is_local())
}

#[cfg(unix)]
pub(super) fn should_bridge_clipboard_image_paste(
    data: &[u8],
    is_remote_client: bool,
    remote_image_paste_key: Option<(crossterm::event::KeyCode, crossterm::event::KeyModifiers)>,
) -> bool {
    if !is_remote_client {
        return false;
    }
    if data == b"\x1b[200~\x1b[201~" {
        return true;
    }

    let Some(remote_image_paste_key) = remote_image_paste_key else {
        return false;
    };

    let events = crate::raw_input::parse_framed_input(data);
    matches!(
        events.as_slice(),
        [crate::raw_input::RawInputEvent::Key(key)]
            if key.kind == crossterm::event::KeyEventKind::Press
                && crate::config::terminal_key_matches_combo(key, remote_image_paste_key)
    )
}

#[cfg(windows)]
pub(super) fn should_bridge_clipboard_image_events(
    events: &[ClientInputEvent],
    is_remote_client: bool,
    remote_image_paste_key: Option<(crossterm::event::KeyCode, crossterm::event::KeyModifiers)>,
) -> bool {
    if !is_remote_client {
        return false;
    }
    if matches!(events, [ClientInputEvent::Paste { text }] if text.is_empty()) {
        return true;
    }

    let Some(remote_image_paste_key) = remote_image_paste_key else {
        return false;
    };
    matches!(
        events,
        [event]
            if matches!(
                event.to_raw_input_event(),
                crate::raw_input::RawInputEvent::Key(key)
                    if key.kind == crossterm::event::KeyEventKind::Press
                        && crate::config::terminal_key_matches_combo(
                            &key,
                            remote_image_paste_key,
                        )
            )
    )
}

#[cfg(unix)]
pub(super) fn read_image_file_from_terminal_drop(
    data: &[u8],
    is_remote_client: bool,
) -> Option<crate::platform::ClipboardImage> {
    let (path, extension) = image_path_from_terminal_drop(data, is_remote_client)?;
    read_image_file(path, extension)
}

#[cfg(windows)]
pub(super) fn read_image_file_from_client_events(
    events: &[ClientInputEvent],
    is_remote_client: bool,
) -> Option<crate::platform::ClipboardImage> {
    let [ClientInputEvent::Paste { text }] = events else {
        return None;
    };
    let text = normalized_terminal_drop_text(text)?;
    let (path, extension) =
        image_path_from_drop_text(strip_matching_path_quotes(text), is_remote_client)?;
    read_image_file(path, extension)
}

fn read_image_file(
    path: PathBuf,
    extension: &'static str,
) -> Option<crate::platform::ClipboardImage> {
    let metadata = std::fs::metadata(&path).ok()?;
    if !metadata.is_file() {
        return None;
    }

    let file = std::fs::File::open(&path).ok()?;
    let bytes =
        match crate::platform::read_limited_reader(file, MAX_CLIPBOARD_IMAGE_PAYLOAD).ok()? {
            crate::platform::LimitedRead::Complete(bytes) => bytes,
            crate::platform::LimitedRead::Empty => return None,
            crate::platform::LimitedRead::Oversized => {
                warn!(
                    max = MAX_CLIPBOARD_IMAGE_PAYLOAD,
                    "local image file drop is too large to bridge"
                );
                return None;
            }
        };

    Some(crate::platform::ClipboardImage { bytes, extension })
}

#[cfg(unix)]
pub(super) fn image_path_from_terminal_drop(
    data: &[u8],
    is_remote_client: bool,
) -> Option<(PathBuf, &'static str)> {
    let bytes = bracketed_paste_payload(data).unwrap_or(data);
    let text = std::str::from_utf8(bytes).ok()?;
    let text = normalized_terminal_drop_text(text)?;
    let text = unescape_terminal_drop_path(strip_matching_path_quotes(text));
    image_path_from_drop_text(&text, is_remote_client)
}

fn normalized_terminal_drop_text(text: &str) -> Option<&str> {
    let text = text.trim_end_matches(['\r', '\n']);
    (!text.is_empty() && !text.contains(['\r', '\n'])).then_some(text)
}

fn image_path_from_drop_text(
    text: &str,
    is_remote_client: bool,
) -> Option<(PathBuf, &'static str)> {
    if !is_remote_client {
        return None;
    }
    let path = PathBuf::from(text);
    if !path.is_absolute() {
        return None;
    }
    let extension = recognized_image_extension(path.extension()?.to_str()?)?;
    Some((path, extension))
}

#[cfg(unix)]
fn bracketed_paste_payload(data: &[u8]) -> Option<&[u8]> {
    const START: &[u8] = b"\x1b[200~";
    const END: &[u8] = b"\x1b[201~";
    data.strip_prefix(START)?.strip_suffix(END)
}

fn strip_matching_path_quotes(text: &str) -> &str {
    if text.len() < 2 {
        return text;
    }

    let bytes = text.as_bytes();
    match (bytes.first(), bytes.last()) {
        (Some(b'\''), Some(b'\'')) | (Some(b'"'), Some(b'"')) => &text[1..text.len() - 1],
        _ => text,
    }
}

#[cfg(unix)]
fn unescape_terminal_drop_path(text: &str) -> String {
    let mut unescaped = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(escaped) = chars.next() {
                unescaped.push(escaped);
            } else {
                unescaped.push(ch);
            }
        } else {
            unescaped.push(ch);
        }
    }
    unescaped
}

fn recognized_image_extension(extension: &str) -> Option<&'static str> {
    if extension.eq_ignore_ascii_case("png") {
        Some("png")
    } else if extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg") {
        Some("jpg")
    } else if extension.eq_ignore_ascii_case("gif") {
        Some("gif")
    } else if extension.eq_ignore_ascii_case("webp") {
        Some("webp")
    } else if extension.eq_ignore_ascii_case("bmp") {
        Some("bmp")
    } else {
        None
    }
}

// ---- `[paste] image_as_path`: paste a clipboard image as a saved file path (local panes) ----

/// Days since 1970-01-01 -> (year, month, day), proleptic Gregorian.
#[cfg(any(unix, test))]
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(any(unix, test))]
pub(super) fn paste_image_path(
    pastes_dir: &std::path::Path,
    unix_millis: u128,
    extension: &str,
) -> PathBuf {
    let (y, m, d) = civil_from_days((unix_millis / 86_400_000) as i64);
    pastes_dir
        .join(format!("{y:04}-{m:02}-{d:02}"))
        .join(format!("paste-{unix_millis}.{extension}"))
}

#[cfg(any(unix, test))]
pub(super) fn prune_old_pastes(pastes_dir: &std::path::Path, max_age: std::time::Duration) {
    let now = std::time::SystemTime::now();
    let Ok(days) = std::fs::read_dir(pastes_dir) else {
        return;
    };
    for day in days.flatten() {
        let day_path = day.path();
        let Ok(files) = std::fs::read_dir(&day_path) else {
            continue;
        };
        for file in files.flatten() {
            let old = file
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age > max_age);
            if old {
                let _ = std::fs::remove_file(file.path());
            }
        }
        let _ = std::fs::remove_dir(&day_path); // only succeeds when empty
    }
}

#[cfg(any(unix, test))]
pub(super) fn path_paste_bytes(path: &std::path::Path) -> Vec<u8> {
    format!("\x1b[200~@{} \x1b[201~", path.display()).into_bytes()
}

/// Saves the clipboard image under the state dir and returns the bytes to forward instead.
#[cfg(unix)]
pub(super) fn clipboard_image_as_path_input(keep_days: u32) -> Option<Vec<u8>> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let image = crate::platform::read_clipboard_image()?;
    let pastes_dir = crate::config::state_dir().join("pastes");
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let path = paste_image_path(&pastes_dir, millis, image.extension);
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        f.write_all(&image.bytes)
    };
    if let Err(err) = write() {
        warn!(%err, path = %path.display(), "could not save pasted image");
        return None;
    }
    prune_old_pastes(
        &pastes_dir,
        std::time::Duration::from_secs(u64::from(keep_days) * 86_400),
    );
    info!(path = %path.display(), bytes = image.bytes.len(), "pasting clipboard image as file path");
    Some(path_paste_bytes(&path))
}

#[cfg(test)]
mod paste_tests {
    use super::*;

    #[test]
    fn path_is_dated_and_millis_named() {
        let p = paste_image_path(std::path::Path::new("/s/pastes"), 1_795_000_000_123, "png");
        assert_eq!(p, PathBuf::from("/s/pastes/2026-11-18/paste-1795000000123.png"));
    }

    #[test]
    fn prune_removes_only_old_files() {
        let root = std::env::temp_dir().join(format!("klark-paste-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let day = root.join("2026-01-01");
        std::fs::create_dir_all(&day).unwrap();
        let old = day.join("old.png");
        let new = day.join("new.png");
        std::fs::write(&old, b"x").unwrap();
        std::fs::write(&new, b"x").unwrap();
        let three_days_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 86_400);
        std::fs::File::options().write(true).open(&old).unwrap().set_modified(three_days_ago).unwrap();
        prune_old_pastes(&root, std::time::Duration::from_secs(2 * 86_400));
        assert!(!old.exists());
        assert!(new.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn path_paste_bytes_are_bracketed_with_at_prefix() {
        let b = path_paste_bytes(std::path::Path::new("/a/b.png"));
        assert_eq!(b, b"\x1b[200~@/a/b.png \x1b[201~".to_vec());
    }
}
