// Ports: none. This writes the table that crates/gorse-server/src/rest/printable.rs
// holds, from Go's own unicode.IsPrint, so the Rust quoting of error texts
// matches Go's for every code point. The conformance test
// (crates/gorse-conformance/tests/go_printable.rs) runs it and compares the
// output with the committed file.
package main

import (
	"bufio"
	"fmt"
	"os"
	"unicode"
)

func main() {
	out := bufio.NewWriter(os.Stdout)
	defer out.Flush()
	fmt.Fprintf(out, "//! The code points that Go's `unicode.IsPrint` takes, as sorted, inclusive\n")
	fmt.Fprintf(out, "//! ranges. Generated from Go %s by\n", "1.27.0")
	fmt.Fprintf(out, "//! `porting/evidence/wp-4-2/printable-table`, which the conformance test\n")
	fmt.Fprintf(out, "//! `go_printable` regenerates and compares with this file. Do not edit it.\n\n")
	fmt.Fprintf(out, "/// The printable code points, as inclusive ranges (`unicode.IsPrint`, Unicode %s).\n", unicode.Version)
	fmt.Fprintf(out, "pub(super) const PRINTABLE: &[(u32, u32)] = &[\n")
	start := -1
	for r := 0; r <= unicode.MaxRune+1; r++ {
		printable := r <= unicode.MaxRune && unicode.IsPrint(rune(r))
		switch {
		case printable && start < 0:
			start = r
		case !printable && start >= 0:
			fmt.Fprintf(out, "    (0x%04x, 0x%04x),\n", start, r-1)
			start = -1
		}
	}
	fmt.Fprintf(out, "];\n\n")
	fmt.Fprintf(out, "/// Whether Go's `unicode.IsPrint` takes `c`.\n")
	fmt.Fprintf(out, "pub(super) fn is_print(c: char) -> bool {\n")
	fmt.Fprintf(out, "    let code = u32::from(c);\n")
	fmt.Fprintf(out, "    let index = PRINTABLE.partition_point(|&(_, high)| high < code);\n")
	fmt.Fprintf(out, "    PRINTABLE.get(index).is_some_and(|&(low, _)| low <= code)\n")
	fmt.Fprintf(out, "}\n")
}
