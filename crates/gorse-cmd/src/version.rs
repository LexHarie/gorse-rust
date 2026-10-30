// Ports: cmd/version/version.go @ add21b5

//! The version banner that `-v` and `--version` print (`cmd/version/version.go`).
//!
//! Go sets the version and the git commit with `ldflags`. This port has no
//! `ldflags`: the version is the crate's, and the commit and the build time keep
//! Go's defaults. The Go toolchain line is left out, since this is a Rust build.

/// The version of this build. Go's default is `unknown-version`; this port
/// reports the crate's version instead (`porting/notes/WP-4-5.md`, deviation 1).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The API version the nodes speak. The same value as Go's `APIVersion`.
pub const API_VERSION: &str = "v0.2.7";

/// The git commit of the build. Go's default, since there are no `ldflags`.
pub const GIT_COMMIT: &str = "unknown-commit";

/// The build time of the binary. Go's default, since there are no `ldflags`.
pub const BUILD_TIME: &str = "unknown-buildtime";

/// Go's `BuildInfo`: the banner with the same labels, tabs and spacing.
pub fn build_info() -> String {
    format!(
        "Version:\t {VERSION}\nAPI version:\t {API_VERSION}\nGit commit:\t {GIT_COMMIT}\nBuilt:\t\t {BUILD_TIME}\nOS/Arch:\t {}/{}\n",
        go_os(),
        go_arch(),
    )
}

/// The operating system in Go's spelling (`GOOS`).
fn go_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    }
}

/// The architecture in Go's spelling (`GOARCH`).
fn go_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_has_go_labels_in_go_order() {
        let info = build_info();
        let labels: Vec<&str> = info
            .lines()
            .map(|line| line.split(":\t").next().unwrap_or_default())
            .collect();
        assert_eq!(
            labels,
            ["Version", "API version", "Git commit", "Built", "OS/Arch"]
        );
    }

    #[test]
    fn banner_keeps_go_spacing() {
        let info = build_info();
        assert!(info.starts_with("Version:\t 0."));
        assert!(info.contains("\nAPI version:\t v0.2.7\n"));
        assert!(info.contains("\nGit commit:\t unknown-commit\n"));
        assert!(info.contains("\nBuilt:\t\t unknown-buildtime\n"));
        assert!(info.ends_with('\n'));
    }
}
