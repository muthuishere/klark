use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const PRODUCT_ANNOUNCEMENTS_PATH: &str = "product-announcements.json";

#[derive(Debug, Clone, Deserialize)]
pub struct ManifestAnnouncement {
    pub id: String,
    pub title: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductAnnouncement {
    pub version: String,
    pub id: String,
    pub title: String,
    pub body: String,
    pub preview: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredProductAnnouncement {
    pub version: String,
    pub id: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AnnouncementStore {
    #[serde(default)]
    latest: Option<StoredProductAnnouncement>,
    #[serde(default)]
    seen: BTreeSet<String>,
}

impl From<&ProductAnnouncement> for StoredProductAnnouncement {
    fn from(announcement: &ProductAnnouncement) -> Self {
        Self {
            version: announcement.version.clone(),
            id: announcement.id.clone(),
            title: announcement.title.clone(),
            body: announcement.body.clone(),
        }
    }
}

fn seen_key(version: &str, id: &str) -> String {
    format!("{version}/{id}")
}

pub fn store_path() -> PathBuf {
    crate::config::state_dir().join(PRODUCT_ANNOUNCEMENTS_PATH)
}

pub fn save_manifest_announcement(
    manifest_version: &str,
    announcement: Option<&ManifestAnnouncement>,
) -> io::Result<Option<ProductAnnouncement>> {
    let Some(announcement) = announcement else {
        clear_latest_for_version(&store_path(), manifest_version)?;
        return Ok(None);
    };

    let Some(product_announcement) = announcement_from_manifest(manifest_version, announcement)
    else {
        clear_latest_for_version(&store_path(), manifest_version)?;
        return Ok(None);
    };

    save_latest_to_path(&store_path(), product_announcement.clone())?;
    Ok(Some(product_announcement))
}

pub fn mark_seen(version: &str, id: &str) -> io::Result<()> {
    mark_seen_at(&store_path(), version, id)
}

fn announcement_from_manifest(
    manifest_version: &str,
    announcement: &ManifestAnnouncement,
) -> Option<ProductAnnouncement> {
    let id = announcement.id.trim();
    let body = normalize_body(&announcement.body);
    if id.is_empty() || body.is_empty() {
        return None;
    }

    let title = announcement
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("announcement")
        .to_string();

    Some(ProductAnnouncement {
        version: manifest_version.trim().to_string(),
        id: id.to_string(),
        title,
        body,
        preview: false,
    })
}

fn save_latest_to_path(path: &Path, announcement: ProductAnnouncement) -> io::Result<()> {
    let mut store = load_store_from_path(path).unwrap_or_default();
    store.latest = Some(StoredProductAnnouncement::from(&announcement));
    write_store_to_path(path, &store)
}

fn clear_latest_for_version(path: &Path, version: &str) -> io::Result<()> {
    let mut store = load_store_from_path(path).unwrap_or_default();
    if store
        .latest
        .as_ref()
        .is_some_and(|announcement| announcement.version == version)
    {
        store.latest = None;
        write_store_to_path(path, &store)?;
    }
    Ok(())
}

fn mark_seen_at(path: &Path, version: &str, id: &str) -> io::Result<()> {
    let mut store = load_store_from_path(path).unwrap_or_default();
    store.seen.insert(seen_key(version, id));
    write_store_to_path(path, &store)
}

fn load_store_from_path(path: &Path) -> Option<AnnouncementStore> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn write_store_to_path(path: &Path, store: &AnnouncementStore) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(store).map_err(io::Error::other)?;
    let tmp_path = path.with_extension(format!("json.tmp.{}", std::process::id()));
    fs::write(&tmp_path, json)?;
    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    Ok(())
}

fn normalize_body(body: &str) -> String {
    body.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "klark-product-announcements-{name}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn clearing_manifest_announcement_removes_matching_latest() {
        let path = temp_path("clear");
        save_latest_to_path(
            &path,
            ProductAnnouncement {
                version: "1.2.3".into(),
                id: "keymap-v2".into(),
                title: "Keymap changed".into(),
                body: "### Changed\n- One".into(),
                preview: false,
            },
        )
        .unwrap();
        clear_latest_for_version(&path, "1.2.3").unwrap();

        assert_eq!(load_store_from_path(&path).unwrap().latest, None);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn manifest_announcement_requires_id_and_body() {
        assert!(announcement_from_manifest(
            "1.2.3",
            &ManifestAnnouncement {
                id: " ".into(),
                title: None,
                body: "hello".into(),
            },
        )
        .is_none());
        assert!(announcement_from_manifest(
            "1.2.3",
            &ManifestAnnouncement {
                id: "notice".into(),
                title: None,
                body: " ".into(),
            },
        )
        .is_none());
    }
}
