//! Shared hook source path discovery.

use std::path::{Path, PathBuf};

use xai_grok_config::resolve_global_hook_sources;
use xai_grok_hooks::discovery::HookSource;
use xai_grok_hooks::error::HookError;

/// Owned paths for hook sources. Callers borrow via `as_sources()`.
pub struct HookSourcePaths {
    pub global: Vec<PathBuf>,
    pub project: Vec<PathBuf>,
}

impl HookSourcePaths {
    /// Borrow as `HookSource` refs. Project sources are excluded when untrusted.
    pub fn as_sources(&self, include_project: bool) -> (Vec<HookSource<'_>>, Vec<HookSource<'_>>) {
        let global = self.global.iter().map(|p| path_to_source(p)).collect();
        let project = if include_project {
            self.project.iter().map(|p| path_to_source(p)).collect()
        } else {
            vec![]
        };
        (global, project)
    }
}

fn path_to_source(p: &Path) -> HookSource<'_> {
    if p.is_dir() {
        HookSource::Directory(p)
    } else {
        HookSource::SettingsFile(p)
    }
}

fn include_claude_hooks(compat: &xai_grok_tools::types::compat::CompatConfig) -> bool {
    compat.claude.hooks
        && !crate::claude_import::is_claude_import_marked_with_log("discover_hook_source_paths")
}

fn include_cursor_hooks(compat: &xai_grok_tools::types::compat::CompatConfig) -> bool {
    compat.cursor.hooks
}

/// Global + project hook source paths. Registry file is never a discovery
/// source; Claude/Cursor globals are appended when gates are on.
pub fn discover_hook_source_paths(
    git_root: Option<&Path>,
    compat: &xai_grok_tools::types::compat::CompatConfig,
) -> HookSourcePaths {
    let grok = xai_grok_config::user_grok_home();
    let home = dirs::home_dir();
    let include_claude = include_claude_hooks(compat);
    let include_cursor = include_cursor_hooks(compat);

    // Soft hooks-paths I/O keeps fixed slots; hard resolve omits Grok globals.
    let mut global: Vec<PathBuf> =
        match resolve_global_hook_sources(grok.as_deref(), /* reject_symlinks */ false) {
            Ok(resolved) => {
                if let Some(e) = &resolved.configured_error {
                    tracing::warn!(
                        error = %e,
                        "hooks-paths unreadable; retaining fixed Grok hook discovery sources only"
                    );
                }
                resolved
                    .discovery_sources()
                    .map(|s| s.path.clone())
                    .collect()
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "global hook source resolve hard-failed; omitting Grok global sources"
                );
                Vec::new()
            }
        };

    if let Some(h) = home.as_deref() {
        if include_claude {
            global.push(h.join(".claude").join("settings.json"));
            global.push(h.join(".claude").join("settings.local.json"));
        }
        if include_cursor {
            global.push(h.join(".cursor").join("hooks.json"));
        }
    }

    let mut project = Vec::new();
    if let Some(root) = git_root {
        if include_claude {
            project.push(root.join(".claude").join("settings.json"));
            project.push(root.join(".claude").join("settings.local.json"));
        }
        project.push(root.join(".grok").join("hooks"));
        if include_cursor {
            project.push(root.join(".cursor").join("hooks.json"));
        }
    }

    HookSourcePaths { global, project }
}

/// Single load entry point: build compat-aware sources, gate project sources on
/// trust, then load. Every session-startup and mid-session reload site routes
/// through here so the source policy stays in one place.
pub fn discover_hooks(
    git_root: Option<&Path>,
    compat: &xai_grok_tools::types::compat::CompatConfig,
    trusted: bool,
) -> (xai_grok_hooks::discovery::HookRegistry, Vec<HookError>) {
    let source_paths = discover_hook_source_paths(git_root, compat);
    let (global_sources, project_sources) = source_paths.as_sources(trusted);
    xai_grok_hooks::discovery::load_hooks_from_sources(&global_sources, &project_sources)
}

#[cfg(test)]
mod claude_user_hook_tests {
    use super::*;
    use xai_grok_hooks::event::HookEventName;
    use xai_grok_tools::types::compat::CompatConfig;
    use xai_grok_test_support::EnvGuard;

    /// Write a Kookr-shaped session home + an operator `~/.claude/settings.json`
    /// carrying one `"Bash"`-matched PreToolUse hook. Returns the two tempdirs
    /// (kept alive by the caller) so the fixture is not reaped mid-test.
    fn kookr_fixture() -> (tempfile::TempDir, tempfile::TempDir) {
        let home = tempfile::tempdir().unwrap();
        let claude_dir = home.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join("settings.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"/home/jean/.claude/hooks/claim-gate.sh"}]}]}}"#,
        )
        .unwrap();

        // Isolated GROK_HOME with only a hooks/ dir (no config.toml => no marker),
        // mirroring the Kookr grok-home-composer output.
        let grok_home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(grok_home.path().join("hooks")).unwrap();
        (home, grok_home)
    }

    /// Reproduce the environment a Kookr-launched Grok session runs under with
    /// Claude hook compat left ON: the operator's real `HOME` (so
    /// `~/.claude/settings.json` is reachable), an isolated empty `GROK_HOME`
    /// (Kookr's composed session home), and no `[claude_compat] imported` marker.
    /// Under this shape the fork DOES discover, load, and matcher-alias the
    /// user's global Claude hooks — proving Claude-compat works in the fork and
    /// the fork is not the layer that drops them.
    #[test]
    #[serial_test::serial]
    fn claude_compat_on_loads_and_matches_user_hooks() {
        let (home, grok_home) = kookr_fixture();
        let _home_guard = EnvGuard::set("HOME", home.path());
        let _grok_guard = EnvGuard::set("GROK_HOME", grok_home.path());

        // Claude hook compat ON is the fork default.
        let compat = CompatConfig::default();
        assert!(compat.claude.hooks, "fork default must enable claude hooks");

        // 1. The global source list includes the operator's ~/.claude settings.
        let paths = discover_hook_source_paths(None, &compat);
        assert!(
            paths
                .global
                .iter()
                .any(|p| p.ends_with(".claude/settings.json")),
            "compat ON must discover ~/.claude/settings.json as a global hook \
             source; got {:?}",
            paths.global
        );

        // 2. The hook actually loads into the registry.
        let (registry, errors) = discover_hooks(None, &compat, /* trusted */ true);
        assert!(errors.is_empty(), "unexpected load errors: {errors:?}");
        let pre = registry.hooks_for(HookEventName::PreToolUse);
        assert_eq!(
            pre.len(),
            1,
            "the Claude Bash PreToolUse hook from ~/.claude/settings.json must load"
        );

        // 3. The `"Bash"` matcher aliases onto Grok's run_terminal_command so the
        //    loaded hook actually fires on Grok tool calls.
        let matcher = pre[0]
            .matcher
            .as_ref()
            .expect("Bash matcher must compile");
        assert!(
            matcher.is_match("run_terminal_command"),
            "Claude \"Bash\" matcher must alias onto Grok's run_terminal_command"
        );
    }

    /// The exact behavior a Kookr-launched session sees: Kookr sets
    /// `GROK_CLAUDE_HOOKS_ENABLED=0` in the child env
    /// (kookr `src/adapters/grok-launch-args.ts:141`), which the fork resolves
    /// to `compat.claude.hooks = false`. With the cell off, discovery drops the
    /// `~/.claude/settings.json` global source entirely — so the operator's
    /// Claude hooks never load. This is the fork faithfully honoring Kookr's
    /// toggle, i.e. the drop is intentional and originates in Kookr, not a fork
    /// regression.
    #[test]
    #[serial_test::serial]
    fn claude_compat_off_drops_user_hooks() {
        let (home, grok_home) = kookr_fixture();
        let _home_guard = EnvGuard::set("HOME", home.path());
        let _grok_guard = EnvGuard::set("GROK_HOME", grok_home.path());

        // The env var Kookr sets resolves the claude.hooks cell to false: env
        // override beats the default-ON. This ties Kookr's toggle to the bool.
        let _hooks_guard = EnvGuard::set("GROK_CLAUDE_HOOKS_ENABLED", "0");
        let resolved = crate::agent::config::resolve_compat_cell_with_env(
            xai_grok_config::env_bool("GROK_CLAUDE_HOOKS_ENABLED"),
            /* cfg    */ None,
            /* remote */ None,
            /* default*/ true,
        );
        assert!(
            !resolved.value,
            "GROK_CLAUDE_HOOKS_ENABLED=0 must resolve claude.hooks to false"
        );

        // A compat with the cell off (what the fork threads at launch under Kookr).
        let mut compat = CompatConfig::default();
        compat.claude.hooks = false;

        let paths = discover_hook_source_paths(None, &compat);
        assert!(
            !paths
                .global
                .iter()
                .any(|p| p.ends_with(".claude/settings.json")),
            "compat OFF must NOT discover ~/.claude/settings.json; got {:?}",
            paths.global
        );

        let (registry, errors) = discover_hooks(None, &compat, /* trusted */ true);
        assert!(errors.is_empty(), "unexpected load errors: {errors:?}");
        assert_eq!(
            registry.hooks_for(HookEventName::PreToolUse).len(),
            0,
            "with claude hooks disabled, the operator's Claude hook must not load"
        );
    }
}
