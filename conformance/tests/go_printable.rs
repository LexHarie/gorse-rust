//! The table that quotes error texts is Go 1.27.0's `unicode.IsPrint`. This
//! runs Go's generator and compares its output with the committed table, so the
//! Rust table cannot drift from Go's data. It fails, and does not skip, when Go
//! is missing.

use std::path::Path;
use std::process::Command;

#[test]
fn printable_table_is_go_unicode_isprint() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let generator = root.join("porting/evidence/wp-4-2/printable-table");
    let output = Command::new("go")
        .args(["run", "."])
        .current_dir(&generator)
        .output()
        .expect("run Go, which the test needs");
    assert!(
        output.status.success(),
        "the Go generator failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let committed = std::fs::read_to_string(root.join("crates/gorse-server/src/rest/printable.rs"))
        .expect("read the committed table");
    let generated = String::from_utf8(output.stdout).expect("the generator writes UTF-8");
    assert!(
        committed == generated,
        "the committed printable.rs differs from Go's output; regenerate it with `go run .` in {}",
        generator.display()
    );
}
