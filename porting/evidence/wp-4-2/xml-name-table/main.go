// Ports: none. This writes the tables that crates/gorse-server/src/rest/xmlname.rs
// holds: the code points that encoding/xml takes as the first character of a
// name and as a later character. encoding/xml keeps its tables unexported, so
// they are read through the decoder's own Token: `<c/>` gives a start element
// named c exactly when c is a valid first character, and `<ac/>` when c is a
// valid later character. The ASCII characters that are not name bytes are
// left out, because encoding/xml's readName never puts them in a name. The
// conformance test (conformance/tests/go_xmlname.rs) runs this and compares
// the output with the committed file.
package main

import (
	"bufio"
	"encoding/xml"
	"fmt"
	"os"
	"strings"
)

// startElement reports whether the decoder reads text as a start element.
func startElement(text string) bool {
	tok, err := xml.NewDecoder(strings.NewReader(text)).Token()
	if err != nil {
		return false
	}
	_, ok := tok.(xml.StartElement)
	return ok
}

// isNameByte reports whether encoding/xml's readName takes the ASCII byte c.
func isNameByte(c rune) bool {
	return 'A' <= c && c <= 'Z' || 'a' <= c && c <= 'z' || '0' <= c && c <= '9' ||
		c == '_' || c == ':' || c == '.' || c == '-'
}

// isNameStart reports whether encoding/xml takes c as the first character of a name.
func isNameStart(c rune) bool {
	if c < 0x80 && !isNameByte(c) {
		return false
	}
	return startElement("<" + string(c) + "/>")
}

// isNameChar reports whether encoding/xml takes c as a later character of a name.
func isNameChar(c rune) bool {
	if c < 0x80 && !isNameByte(c) {
		return false
	}
	return startElement("<a" + string(c) + "/>")
}

// ranges writes the code points that take pred, as sorted inclusive ranges.
func ranges(out *bufio.Writer, name string, pred func(rune) bool) {
	fmt.Fprintf(out, "pub(super) const %s: &[(u32, u32)] = &[\n", name)
	start := -1
	for r := 0; r <= 0x10ffff+1; r++ {
		taken := r <= 0x10ffff && !(0xd800 <= r && r <= 0xdfff) && pred(rune(r))
		switch {
		case taken && start < 0:
			start = r
		case !taken && start >= 0:
			fmt.Fprintf(out, "    (0x%04x, 0x%04x),\n", start, r-1)
			start = -1
		}
	}
	fmt.Fprintf(out, "];\n\n")
}

func main() {
	out := bufio.NewWriter(os.Stdout)
	defer out.Flush()
	fmt.Fprintf(out, "//! The characters that Go's `encoding/xml` takes in names, as sorted, inclusive\n")
	fmt.Fprintf(out, "//! ranges. Generated from Go %s by\n", "1.27.0")
	fmt.Fprintf(out, "//! `porting/evidence/wp-4-2/xml-name-table`, which the conformance test\n")
	fmt.Fprintf(out, "//! `go_xmlname` regenerates and compares with this file. Do not edit it.\n\n")
	ranges(out, "NAME_START", isNameStart)
	ranges(out, "NAME_CHAR", isNameChar)
	fmt.Fprintf(out, "/// Whether `encoding/xml` takes `c` as the first character of a name.\n")
	fmt.Fprintf(out, "pub(super) fn is_name_start(c: char) -> bool {\n")
	fmt.Fprintf(out, "    in_ranges(NAME_START, c)\n")
	fmt.Fprintf(out, "}\n\n")
	fmt.Fprintf(out, "/// Whether `encoding/xml` takes `c` as a character after the first of a name.\n")
	fmt.Fprintf(out, "pub(super) fn is_name_char(c: char) -> bool {\n")
	fmt.Fprintf(out, "    in_ranges(NAME_CHAR, c)\n")
	fmt.Fprintf(out, "}\n\n")
	fmt.Fprintf(out, "/// Whether `c` lies in one of the sorted, inclusive `ranges`.\n")
	fmt.Fprintf(out, "fn in_ranges(ranges: &[(u32, u32)], c: char) -> bool {\n")
	fmt.Fprintf(out, "    let code = u32::from(c);\n")
	fmt.Fprintf(out, "    let index = ranges.partition_point(|&(_, high)| high < code);\n")
	fmt.Fprintf(out, "    ranges.get(index).is_some_and(|&(low, _)| low <= code)\n")
	fmt.Fprintf(out, "}\n")
}
