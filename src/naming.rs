use std::path::Path;
use std::process::Command;

/// Resolve session display name from cwd.
/// Uses git to find the repository root name, with monorepo subdirectory enhancement.
pub fn resolve_name(cwd: &str) -> String {
    let toplevel = git_toplevel(cwd);
    resolve_name_with(cwd, |_| toplevel.clone())
}

/// Get the git toplevel for a given directory.
fn git_toplevel(cwd: &str) -> Option<String> {
    Command::new("git")
        .args(["-C", cwd, "rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8(o.stdout)
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
}

/// Pure logic name resolver, injectable git_toplevel_fn for testing.
/// spec §7.1
pub fn resolve_name_with<F>(cwd: &str, git_toplevel_fn: F) -> String
where
    F: Fn(&str) -> Option<String>,
{
    let cwd_path = Path::new(cwd);

    match git_toplevel_fn(cwd) {
        Some(toplevel) => {
            let toplevel_path = Path::new(&toplevel);
            let repo_name = toplevel_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");

            // Check if cwd is deeper than toplevel (monorepo)
            if cwd_path != toplevel_path {
                // Get relative path from toplevel to cwd
                if let Ok(rel) = cwd_path.strip_prefix(toplevel_path) {
                    // Take the last component of the relative path
                    if let Some(subdir) = rel.components().next_back() {
                        let subdir_name = subdir.as_os_str().to_str().unwrap_or("?");
                        let combined = format!("{repo_name}/{subdir_name}");
                        // Truncate to 16 chars per spec
                        return truncate_name(&combined, 16);
                    }
                }
            }

            repo_name.to_string()
        }
        None => {
            // Fallback: use cwd last segment (spec §7.1 method A)
            cwd_basename(cwd)
        }
    }
}

fn cwd_basename(cwd: &str) -> String {
    let path = Path::new(cwd);

    // Special cases for home/root
    if cwd == "/" {
        return "/".to_string();
    }

    // Normalize home dir
    if let Ok(home) = std::env::var("HOME")
        && (cwd == home || cwd == home.trim_end_matches('/'))
    {
        return "~".to_string();
    }

    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cwd)
        .to_string()
}

fn truncate_name(name: &str, max_chars: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max_chars {
        name.to_string()
    } else {
        chars[..max_chars].iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_as_toplevel_returns_repo_name() {
        let name = resolve_name_with("/a/b/payments-api", |_| {
            Some("/a/b/payments-api".to_string())
        });
        assert_eq!(name, "payments-api");
    }

    #[test]
    fn deeper_than_toplevel_monorepo_subdir() {
        let name = resolve_name_with("/a/b/bigmono/packages/api", |_| {
            Some("/a/b/bigmono".to_string())
        });
        assert_eq!(name, "bigmono/api");
    }

    #[test]
    fn non_git_fallback_to_cwd_last_segment() {
        let name = resolve_name_with("/a/b/web", |_| None);
        assert_eq!(name, "web");
    }

    #[test]
    fn cwd_root_returns_slash() {
        let name = resolve_name_with("/", |_| None);
        assert_eq!(name, "/");
    }

    #[test]
    fn cwd_home_returns_tilde() {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            let name = resolve_name_with(&home, |_| None);
            assert_eq!(name, "~");
        }
    }

    #[test]
    fn monorepo_name_truncated_to_16() {
        // "verylongreponame" + "/" + "subdir" would be > 16
        let name = resolve_name_with("/repos/verylongrepository/packages/subdir", |_| {
            Some("/repos/verylongrepository".to_string())
        });
        // "verylongrepository/subdir" = 25 chars, should be truncated to 16
        assert!(name.chars().count() <= 16);
    }

    #[test]
    fn toplevel_none_uses_last_segment() {
        let name = resolve_name_with("/home/user/my-project", |_| None);
        assert_eq!(name, "my-project");
    }

    // ---------- Phase 2: monorepo deep-path and spec §7.1 conformance ----------

    #[test]
    fn monorepo_deep_path_uses_last_subdir_segment() {
        // cwd = /root/bigmono/packages/api/src — deep inside the monorepo.
        // Spec §7.1: show `repo/last-component-of-relative-path` = `bigmono/src`.
        let name = resolve_name_with("/root/bigmono/packages/api/src", |_| {
            Some("/root/bigmono".to_string())
        });
        // The last component of the relative path (packages/api/src) is "src".
        assert_eq!(name, "bigmono/src");
    }

    #[test]
    fn monorepo_single_level_subdir() {
        // cwd == toplevel + one level (most common monorepo case).
        let name = resolve_name_with("/work/myrepo/frontend", |_| {
            Some("/work/myrepo".to_string())
        });
        assert_eq!(name, "myrepo/frontend");
    }

    #[test]
    fn non_monorepo_at_exact_toplevel_returns_repo_name() {
        // cwd exactly equals the git toplevel — no subdir, just the repo name.
        let name = resolve_name_with("/work/myrepo", |_| Some("/work/myrepo".to_string()));
        assert_eq!(name, "myrepo");
    }

    #[test]
    fn monorepo_name_within_16_chars_not_truncated() {
        // "short/api" = 9 chars — should not be truncated.
        let name = resolve_name_with("/repos/short/packages/api", |_| {
            Some("/repos/short".to_string())
        });
        assert_eq!(name, "short/api");
        assert!(name.chars().count() <= 16);
    }

    #[test]
    fn fallback_deeply_nested_non_git_uses_last_segment() {
        let name = resolve_name_with("/a/b/c/d/e/my-service", |_| None);
        assert_eq!(name, "my-service");
    }
}
