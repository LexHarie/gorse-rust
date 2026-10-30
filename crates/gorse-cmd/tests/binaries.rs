// Ports: cmd/gorse-master/main.go, cmd/gorse-server/main.go, cmd/gorse-worker/main.go, cmd/version/version.go @ add21b5 (the process-level behaviour: exit codes, the banner, the SSL rule, and `config.MkDir` in `init`)

//! Runs the four binaries as child processes, with `HOME` set to a temporary
//! directory, and checks what Go's `main` does before it starts a node.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

/// Runs `bin` with `args`, with `home` as the home directory.
fn run(bin: &str, home: &Path, args: &[&str]) -> Output {
    Command::new(bin)
        .args(args)
        .env("HOME", home)
        .output()
        .unwrap_or_else(|err| panic!("run {bin}: {err}"))
}

/// Both streams of a run, since the logger writes to stdout and clap to stderr.
fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn version_prints_the_banner_and_exits_zero() {
    let home = TempDir::new().expect("a temporary home");
    let output = run(env!("CARGO_BIN_EXE_gorse-master"), home.path(), &["-v"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("Version:\t "), "{stdout}");
    assert!(stdout.contains("API version:\t v0.2.7\n"), "{stdout}");
}

#[test]
fn version_ignores_stray_arguments() {
    let home = TempDir::new().expect("a temporary home");
    let output = run(
        env!("CARGO_BIN_EXE_gorse-server"),
        home.path(),
        &["--version", "stray"],
    );
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("Version:\t "));
}

#[test]
fn help_creates_the_default_cache_directory() {
    // `config.MkDir` runs while the flags are defined, so even `--help` creates it.
    let home = TempDir::new().expect("a temporary home");
    let output = run(env!("CARGO_BIN_EXE_gorse-master"), home.path(), &["--help"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert!(home.path().join(".gorse/var/lib/master").is_dir());
}

#[test]
fn help_does_not_create_a_non_default_cache_path() {
    let home = TempDir::new().expect("a temporary home");
    let other = home.path().join("elsewhere");
    let other_text = other.display().to_string();
    let output = run(
        env!("CARGO_BIN_EXE_gorse-master"),
        home.path(),
        &["--help", "--cache-path", &other_text],
    );
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert!(!other.exists());
}

#[test]
fn usage_error_exits_one_as_go_does() {
    let home = TempDir::new().expect("a temporary home");
    let output = run(
        env!("CARGO_BIN_EXE_gorse-master"),
        home.path(),
        &["--no-such-flag"],
    );
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("--no-such-flag"),
        "{}",
        text(&output)
    );
}

#[test]
fn a_flag_that_needs_a_value_without_one_is_a_usage_error() {
    let home = TempDir::new().expect("a temporary home");
    let output = run(
        env!("CARGO_BIN_EXE_gorse-master"),
        home.path(),
        &["--config"],
    );
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
}

#[test]
fn server_with_part_of_the_ssl_flags_is_fatal() {
    let home = TempDir::new().expect("a temporary home");
    let output = run(
        env!("CARGO_BIN_EXE_gorse-server"),
        home.path(),
        &["--ssl-ca", "ca.pem"],
    );
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("incomplete SSL configuration"),
        "{}",
        text(&output)
    );
}

#[test]
fn worker_with_part_of_the_ssl_flags_is_fatal() {
    let home = TempDir::new().expect("a temporary home");
    let output = run(
        env!("CARGO_BIN_EXE_gorse-worker"),
        home.path(),
        &["--ssl-cert", "cert.pem", "--ssl-key", "key.pem"],
    );
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("incomplete SSL configuration"),
        "{}",
        text(&output)
    );
}

#[test]
fn a_bare_playground_flag_does_not_take_the_next_word() {
    // pflag's NoOptDefVal: `--playground --help` is the bare flag and then help,
    // and `--playground ml-100k` is the bare flag with `ml-100k` as a stray word.
    let home = TempDir::new().expect("a temporary home");
    for args in [
        &["--playground", "--help"][..],
        &["--playground", "ml-100k", "--help"][..],
    ] {
        let output = run(env!("CARGO_BIN_EXE_gorse-in-one"), home.path(), args);
        assert_eq!(output.status.code(), Some(0), "{args:?}: {}", text(&output));
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("Usage:"),
            "{args:?}"
        );
    }
    assert!(!home.path().join(".gorse/etc/config.toml").exists());
}
