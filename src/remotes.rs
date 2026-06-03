//! Remote registry: reads/writes `~/.roost/remotes.json`.
//!
//! On any error (missing file, corrupt JSON) `load()` falls back to an empty
//! registry — fail-open by design.  Writes are atomic: we write to a sibling
//! `.tmp` file and rename it into place so a mid-write crash cannot corrupt the
//! existing registry.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::history::roost_home;

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// A single remote machine entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    /// Human-readable label for this machine (unique key for upsert/lookup).
    pub origin: String,
    /// SSH target, e.g. `user@host`.
    pub target: String,
    /// Absolute path of the roost socket on the remote machine.
    pub remote_sock: String,
    /// ISO-8601 / RFC-3339 timestamp string recording when this entry was added.
    pub added_at: String,
    /// Architecture tag, e.g. `linux-x86_64` or `darwin-aarch64`.
    pub arch: String,
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// The full on-disk registry: an ordered list of remote entries.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub remotes: Vec<Remote>,
}

// ---------------------------------------------------------------------------
// Public thin wrappers (use real ~/.roost path)
// ---------------------------------------------------------------------------

/// Path to the remotes file (`~/.roost/remotes.json`).
pub fn remotes_path() -> PathBuf {
    roost_home().join("remotes.json")
}

/// Load the registry from disk.  Any I/O or parse error returns an empty
/// [`Registry`].
///
/// Never panics, never returns `Err`.
pub fn load() -> Registry {
    load_from(&remotes_path())
}

/// Persist `registry` to [`remotes_path()`] as pretty-printed JSON.
///
/// Creates the parent directory if it does not exist.
pub fn save(registry: &Registry) -> Result<()> {
    save_to(&remotes_path(), registry)
}

// ---------------------------------------------------------------------------
// Internal path-parameterised helpers (used directly by tests)
// ---------------------------------------------------------------------------

/// Read registry from an explicit `path`.  Missing or corrupt file → empty
/// registry (warn to stderr, no panic).
pub(crate) fn load_from(path: &Path) -> Registry {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("[roost] remotes: could not read {}: {e}", path.display());
            }
            return Registry::default();
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[roost] remotes: corrupt JSON in {}: {e}", path.display());
            Registry::default()
        }
    }
}

/// Write registry to an explicit `path` atomically (temp + rename).
pub(crate) fn save_to(path: &Path, registry: &Registry) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(registry)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &json)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Mutation helpers on Registry
// ---------------------------------------------------------------------------

impl Registry {
    /// Insert or update a remote entry.
    ///
    /// Uniqueness is by `origin`.  If an entry with the same `origin` already
    /// exists it is replaced in-place; otherwise the entry is appended.
    pub fn upsert(&mut self, remote: Remote) {
        if let Some(pos) = self.remotes.iter().position(|r| r.origin == remote.origin) {
            self.remotes[pos] = remote;
        } else {
            self.remotes.push(remote);
        }
    }

    /// Remove the entry whose `origin` equals `key`, returning it if found.
    pub fn remove_by_origin(&mut self, key: &str) -> Option<Remote> {
        if let Some(pos) = self.remotes.iter().position(|r| r.origin == key) {
            Some(self.remotes.remove(pos))
        } else {
            None
        }
    }

    /// Remove the entry whose `target` equals `key`, returning it if found.
    pub fn remove_by_target(&mut self, key: &str) -> Option<Remote> {
        if let Some(pos) = self.remotes.iter().position(|r| r.target == key) {
            Some(self.remotes.remove(pos))
        } else {
            None
        }
    }

    /// Look up a single entry by `origin`.
    pub fn find_by_origin(&self, key: &str) -> Option<&Remote> {
        self.remotes.iter().find(|r| r.origin == key)
    }

    /// Look up a single entry by `target`.
    pub fn find_by_target(&self, key: &str) -> Option<&Remote> {
        self.remotes.iter().find(|r| r.target == key)
    }

    /// Return a slice of all entries.
    pub fn all(&self) -> &[Remote] {
        &self.remotes
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: build a Remote with minimal boilerplate.
    fn make_remote(origin: &str, target: &str) -> Remote {
        Remote {
            origin: origin.to_string(),
            target: target.to_string(),
            remote_sock: "/tmp/roost.sock".to_string(),
            added_at: "2026-06-03T00:00:00Z".to_string(),
            arch: "linux-x86_64".to_string(),
        }
    }

    // ① Insert a new entry and read it back.
    #[test]
    fn insert_and_find() {
        let mut reg = Registry::default();
        let r = make_remote("box1", "alice@box1.example.com");
        reg.upsert(r.clone());

        assert_eq!(reg.all().len(), 1);
        assert_eq!(reg.find_by_origin("box1"), Some(&r));
        assert_eq!(reg.find_by_target("alice@box1.example.com"), Some(&r));
    }

    // ② Upsert replaces an existing entry with the same origin.
    #[test]
    fn upsert_replaces_existing() {
        let mut reg = Registry::default();
        reg.upsert(make_remote("box1", "alice@box1.example.com"));

        let updated = Remote {
            origin: "box1".to_string(),
            target: "bob@box1.example.com".to_string(),
            remote_sock: "/var/run/roost.sock".to_string(),
            added_at: "2026-06-03T01:00:00Z".to_string(),
            arch: "linux-aarch64".to_string(),
        };
        reg.upsert(updated.clone());

        // Still only one entry.
        assert_eq!(reg.all().len(), 1);
        assert_eq!(reg.find_by_origin("box1"), Some(&updated));
    }

    // ③ Upsert is idempotent when called twice with identical data.
    #[test]
    fn upsert_idempotent() {
        let mut reg = Registry::default();
        let r = make_remote("idm", "user@idm.host");
        reg.upsert(r.clone());
        reg.upsert(r.clone());
        assert_eq!(reg.all().len(), 1);
        assert_eq!(reg.find_by_origin("idm"), Some(&r));
    }

    // ④ Remove by origin.
    #[test]
    fn remove_by_origin_found() {
        let mut reg = Registry::default();
        reg.upsert(make_remote("box1", "alice@box1.example.com"));
        reg.upsert(make_remote("box2", "alice@box2.example.com"));

        let removed = reg.remove_by_origin("box1");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().origin, "box1");
        assert_eq!(reg.all().len(), 1);
        assert!(reg.find_by_origin("box1").is_none());
    }

    // ⑤ Remove by origin when not present → None, registry unchanged.
    #[test]
    fn remove_by_origin_missing() {
        let mut reg = Registry::default();
        reg.upsert(make_remote("box1", "alice@box1.example.com"));
        assert!(reg.remove_by_origin("nonexistent").is_none());
        assert_eq!(reg.all().len(), 1);
    }

    // ⑥ Remove by target.
    #[test]
    fn remove_by_target_found() {
        let mut reg = Registry::default();
        reg.upsert(make_remote("box1", "alice@box1.example.com"));

        let removed = reg.remove_by_target("alice@box1.example.com");
        assert!(removed.is_some());
        assert!(reg.all().is_empty());
    }

    // ⑦ Remove by target when not present → None.
    #[test]
    fn remove_by_target_missing() {
        let mut reg = Registry::default();
        assert!(reg.remove_by_target("nobody@nowhere").is_none());
    }

    // ⑧ save_to + load_from round-trip (uses tempfile, never touches ~/.roost).
    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remotes.json");

        let mut reg = Registry::default();
        reg.upsert(make_remote("alpha", "user@alpha.host"));
        reg.upsert(make_remote("beta", "user@beta.host"));

        save_to(&path, &reg).unwrap();
        let loaded = load_from(&path);

        assert_eq!(reg, loaded);
    }

    // ⑨ Missing file → load_from returns empty registry (no panic).
    #[test]
    fn missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no_such_file.json");

        let reg = load_from(&path);
        assert!(reg.all().is_empty());
    }

    // ⑩ Corrupt JSON → load_from returns empty registry (no panic).
    #[test]
    fn corrupt_json_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remotes.json");
        fs::write(&path, b"{{{{ not valid json").unwrap();

        let reg = load_from(&path);
        assert!(reg.all().is_empty());
    }

    // ⑪ Atomic write: after save_to the temp file must not exist.
    #[test]
    fn atomic_write_no_tmp_leftover() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remotes.json");

        let mut reg = Registry::default();
        reg.upsert(make_remote("box1", "user@box1"));
        save_to(&path, &reg).unwrap();

        let tmp = path.with_extension("json.tmp");
        assert!(!tmp.exists(), "temp file should have been renamed away");
        assert!(path.exists(), "final file should exist");
    }

    // ⑫ Multiple remotes, query all().
    #[test]
    fn list_all() {
        let mut reg = Registry::default();
        for i in 0..5 {
            reg.upsert(make_remote(&format!("box{i}"), &format!("u@box{i}.h")));
        }
        assert_eq!(reg.all().len(), 5);
    }

    // ⑬ find_by_origin for non-existent key → None.
    #[test]
    fn find_missing_origin_returns_none() {
        let reg = Registry::default();
        assert!(reg.find_by_origin("ghost").is_none());
    }

    // ⑭ find_by_target for non-existent key → None.
    #[test]
    fn find_missing_target_returns_none() {
        let reg = Registry::default();
        assert!(reg.find_by_target("ghost@host").is_none());
    }
}
