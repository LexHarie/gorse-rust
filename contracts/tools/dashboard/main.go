// Command dashboard vendors the Gorse dashboard bundle into the master crate.
//
// The Go master embeds the bundle through the blank import of
// github.com/gorse-io/dashboard (master/rest.go). That module keeps the bundle
// as a zip inside statik.go, as the string literal that rakyll/statik
// registers. This command fetches the pinned module version through the Go
// module proxy, checks its hash against go.sum, reads the literal out of
// statik.go with go/parser, and extracts the zip into the vendored directory
// that crates/gorse-master embeds with rust-embed. It writes README.md and
// MANIFEST.sha256 next to the files.
//
// Usage, from the repository root: go run ./contracts/tools/dashboard
// (contracts/tools/dashboard/fetch.sh runs it). Nothing else reads the network.
package main

import (
	"archive/zip"
	"bufio"
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
)

// The module and version the Go master pins (go.mod).
const (
	module  = "github.com/gorse-io/dashboard"
	version = "v0.5.20260714"
)

// manifestName is the file that lists the hash of every vendored file.
const manifestName = "MANIFEST.sha256"

// download is the part of `go mod download -json` that this command reads.
type download struct {
	Version string
	Sum     string
	Dir     string
}

func main() {
	out := flag.String("out", "crates/gorse-master/dashboard", "vendored directory, relative to the repository root")
	flag.Parse()
	if err := run(*out); err != nil {
		fmt.Fprintln(os.Stderr, "dashboard:", err)
		os.Exit(1)
	}
}

func run(out string) error {
	d, err := fetch()
	if err != nil {
		return err
	}
	want, err := goSumHash()
	if err != nil {
		return err
	}
	if d.Sum != want {
		return fmt.Errorf("hash %s of %s@%s does not match go.sum (%s)", d.Sum, module, version, want)
	}
	bundle, err := bundleFromStatik(filepath.Join(d.Dir, "statik.go"))
	if err != nil {
		return err
	}
	if err := os.RemoveAll(out); err != nil {
		return err
	}
	if err := extract(bundle, out); err != nil {
		return err
	}
	if err := writeReadme(out, want, len(bundle)); err != nil {
		return err
	}
	return writeManifest(out)
}

// fetch runs `go mod download -json` for the pinned version.
func fetch() (download, error) {
	var d download
	raw, err := exec.Command("go", "mod", "download", "-json", module+"@"+version).Output()
	if err != nil {
		return d, fmt.Errorf("go mod download: %w", err)
	}
	if err := json.Unmarshal(raw, &d); err != nil {
		return d, fmt.Errorf("go mod download output: %w", err)
	}
	if d.Version != version {
		return d, fmt.Errorf("fetched %s, want %s", d.Version, version)
	}
	return d, nil
}

// goSumHash returns the module's h1 hash from go.sum, not the go.mod hash.
func goSumHash() (string, error) {
	f, err := os.Open("go.sum")
	if err != nil {
		return "", err
	}
	defer f.Close()
	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		fields := strings.Fields(scanner.Text())
		if len(fields) == 3 && fields[0] == module && fields[1] == version {
			return fields[2], nil
		}
	}
	if err := scanner.Err(); err != nil {
		return "", err
	}
	return "", fmt.Errorf("go.sum has no line for %s %s", module, version)
}

// bundleFromStatik returns the zip bytes in the string literal that statik.go
// assigns to `data`.
func bundleFromStatik(file string) ([]byte, error) {
	fset := token.NewFileSet()
	tree, err := parser.ParseFile(fset, file, nil, 0)
	if err != nil {
		return nil, err
	}
	var literal *ast.BasicLit
	ast.Inspect(tree, func(n ast.Node) bool {
		assign, ok := n.(*ast.AssignStmt)
		if !ok || literal != nil || len(assign.Lhs) != 1 || len(assign.Rhs) != 1 {
			return true
		}
		ident, ok := assign.Lhs[0].(*ast.Ident)
		if !ok || ident.Name != "data" {
			return true
		}
		if lit, ok := assign.Rhs[0].(*ast.BasicLit); ok && lit.Kind == token.STRING {
			literal = lit
		}
		return true
	})
	if literal == nil {
		return nil, fmt.Errorf("%s: no string literal assigned to data", file)
	}
	value, err := strconv.Unquote(literal.Value)
	return []byte(value), err
}

// extract writes every regular file of the zip under out. Entries that would
// leave out, or are not local paths, fail the extraction.
func extract(bundle []byte, out string) error {
	reader, err := zip.NewReader(bytes.NewReader(bundle), int64(len(bundle)))
	if err != nil {
		return err
	}
	for _, entry := range reader.File {
		if entry.FileInfo().IsDir() {
			continue
		}
		if !filepath.IsLocal(filepath.FromSlash(entry.Name)) {
			return fmt.Errorf("zip entry %q is not a local path", entry.Name)
		}
		dest := filepath.Join(out, filepath.FromSlash(entry.Name))
		if err := os.MkdirAll(filepath.Dir(dest), 0o755); err != nil {
			return err
		}
		if err := copyEntry(entry, dest); err != nil {
			return err
		}
	}
	return nil
}

func copyEntry(entry *zip.File, dest string) error {
	src, err := entry.Open()
	if err != nil {
		return err
	}
	defer src.Close()
	dst, err := os.OpenFile(dest, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0o644)
	if err != nil {
		return err
	}
	if _, err := io.Copy(dst, src); err != nil {
		dst.Close()
		return err
	}
	return dst.Close()
}

// readmeTemplate is the README.md that writeReadme writes. Its arguments are
// the module, the version, the go.sum hash, the zip size and the manifest name.
const readmeTemplate = "# Vendored dashboard\n\n" +
	"The Vue dashboard that the master serves at /. It is embedded with rust-embed\n" +
	"(src/dashboard.rs) and no build changes it.\n\n" +
	"- Module: %[1]s\n" +
	"- Version: %[2]s\n" +
	"- Hash: %[3]s (the go.sum line for the module, checked before extraction)\n" +
	"- Bundle: the zip inside statik.go, %[4]d bytes\n" +
	"- Script: contracts/tools/dashboard/fetch.sh (runs go run ./contracts/tools/dashboard)\n" +
	"- Integrity: %[5]s lists the hash of every other file. Check it from the\n" +
	"  repository root with sha256sum -c crates/gorse-master/dashboard/%[5]s\n"

// writeReadme records where the vendored files came from.
func writeReadme(out, goSum string, zipSize int) error {
	text := fmt.Sprintf(readmeTemplate, module, version, goSum, zipSize, manifestName)
	return os.WriteFile(filepath.Join(out, "README.md"), []byte(text), 0o644)
}

// writeManifest lists every file under out except the manifest itself, in
// sha256sum format with paths relative to the repository root.
func writeManifest(out string) error {
	var files []string
	err := filepath.WalkDir(out, func(p string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() || filepath.Base(p) == manifestName {
			return nil
		}
		files = append(files, filepath.ToSlash(p))
		return nil
	})
	if err != nil {
		return err
	}
	sort.Strings(files)
	var buf strings.Builder
	for _, file := range files {
		content, err := os.ReadFile(filepath.FromSlash(file))
		if err != nil {
			return err
		}
		sum := sha256.Sum256(content)
		fmt.Fprintf(&buf, "%s  %s\n", hex.EncodeToString(sum[:]), file)
	}
	return os.WriteFile(filepath.Join(out, manifestName), []byte(buf.String()), 0o644)
}
