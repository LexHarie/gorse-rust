// Command cligolden is the Go oracle for the output layers of gorse-cli: the YAML
// `pipeline get` writes (gopkg.in/yaml.v3), the errors `pipeline patch` reports
// (github.com/evanphx/json-patch/v5) and the order sort.Slice gives tied items.
// The conformance crate runs it with `go run ./contracts/tools/cligolden MODE`
// and compares its output with the Rust port.
//
//	yaml       one JSON value per stdin line; prints "== line", the YAML, "---END---"
//	jsonpatch  one patch per stdin line, applied to doc; prints the decode or apply error, or the result
//	sort       one line per case of sortCases, the permutation sort.Slice leaves
package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"sort"

	jsonpatch "github.com/evanphx/json-patch/v5"
	"gopkg.in/yaml.v3"
)

const doc = `{"a":1,"arr":[1,2,3],"obj":{"x":"y"},"nul":null,"s":"str"}`

type item struct {
	id  int
	key int
}

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: cligolden yaml|jsonpatch|sort < cases")
		os.Exit(2)
	}
	switch os.Args[1] {
	case "yaml":
		yamlCases()
	case "jsonpatch":
		jsonpatchCases()
	case "sort":
		sortCases()
	default:
		fmt.Fprintf(os.Stderr, "unknown mode %q\n", os.Args[1])
		os.Exit(2)
	}
}

func lines(run func(line string)) {
	sc := bufio.NewScanner(os.Stdin)
	sc.Buffer(make([]byte, 1<<20), 1<<20)
	for sc.Scan() {
		run(sc.Text())
	}
}

func yamlCases() {
	lines(func(line string) {
		var v any
		if err := json.Unmarshal([]byte(line), &v); err != nil {
			fmt.Printf("bad input %s: %v\n", line, err)
			return
		}
		fmt.Printf("== %s\n", line)
		out, err := yaml.Marshal(v)
		if err != nil {
			fmt.Printf("encode error: %v\n", err)
		}
		fmt.Printf("%s", out)
		fmt.Printf("---END---\n")
	})
}

func jsonpatchCases() {
	lines(func(line string) {
		p, err := jsonpatch.DecodePatch([]byte(line))
		if err != nil {
			fmt.Printf("%s\n  decode-err: %s\n", line, err.Error())
			return
		}
		out, err := p.Apply([]byte(doc))
		if err != nil {
			fmt.Printf("%s\n  apply-err: %s\n", line, err.Error())
			return
		}
		fmt.Printf("%s\n  ok: %s\n", line, string(out))
	})
}

// sortCases prints, for each size, key modulus and direction, the ids in the
// order sort.Slice leaves them, as a hash of the whole order and the first
// twelve ids. Keys repeat often, so the ties decide most of the order.
func sortCases() {
	for _, n := range []int{0, 1, 2, 5, 12, 13, 16, 17, 30, 64, 100, 257, 1000} {
		for _, m := range []int{1, 2, 3, 7, 50} {
			for _, desc := range []bool{true, false} {
				items := make([]item, n)
				for i := range items {
					items[i] = item{id: i, key: (i * 7919) % m}
				}
				if desc {
					sort.Slice(items, func(i, j int) bool { return items[i].key > items[j].key })
				} else {
					sort.Slice(items, func(i, j int) bool { return items[i].key < items[j].key })
				}
				var hash uint64
				for _, it := range items {
					hash = hash*1000003 + uint64(it.id) + 1
				}
				head := make([]int, 0, 12)
				for i := 0; i < len(items) && i < 12; i++ {
					head = append(head, items[i].id)
				}
				fmt.Printf("n=%d m=%d desc=%t hash=%d head=%v\n", n, m, desc, hash, head)
			}
		}
	}
}
