//! The name characters that the XML decoder takes are Go 1.27.0's, as
//! `encoding/xml` reads them. This runs Go's generator and compares its output
//! with the committed table, so the Rust table cannot drift from Go's data. It
//! fails, and does not skip, when Go is missing.

use std::path::Path;
use std::process::Command;

#[test]
fn xml_name_tables_are_go_encoding_xml() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let generator = root.join("porting/evidence/wp-4-2/xml-name-table");
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
    let committed = std::fs::read_to_string(root.join("crates/gorse-server/src/rest/xmlname.rs"))
        .expect("read the committed tables");
    let generated = String::from_utf8(output.stdout).expect("the generator writes UTF-8");
    assert!(
        committed == generated,
        "the committed xmlname.rs differs from Go's output; regenerate it with `go run .` in {}",
        generator.display()
    );
}
