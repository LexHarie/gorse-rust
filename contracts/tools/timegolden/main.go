// Copyright 2026 gorse Project Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Command timegolden writes what Go's time package and
// github.com/araddon/dateparse do, for the Rust port of both (WP-1-9,
// crates/gorse-common/src/gotime and src/dateparse):
//
//   - -tables: Rust source with the Unicode predicates dateparse and
//     strconv.Quote use (unicode.IsLetter, unicode.IsDigit, strconv.IsPrint)
//     and the runes unicode.ToLower maps into ASCII, enumerated over every
//     code point by this Go toolchain.
//   - -errno: Rust source with the text of every syscall.Errno.
//   - -zoneinfo: a directory with the TZif files of the zones the tests use,
//     copied from $GOROOT/lib/time/zoneinfo.zip.
//   - -goroot: a stand-in GOROOT whose lib/time/zoneinfo.zip is a stored zip
//     of the same zones, which the Rust ports of Go's tests load the way Go's
//     tests load their GOROOT's zip.
//   - -time-out: Format, String, Parse, ParseInLocation, Date, Unix,
//     Duration.String, ParseDuration and JSON results (gotime_golden.json).
//   - -dateparse-out: ParseAny, ParseIn, ParseLocal, ParseStrict and
//     ParseFormat results over the dateparse test inputs, Go-written cache
//     time strings and a seeded corpus of mutated and generated inputs
//     (dateparse_golden.json).
//
// Locations come from the TZif files in -zoneinfo, loaded with
// time.LoadLocationFromTZData, so the Rust tests can load the same bytes and
// do not depend on the tzdata of the machine they run on. "Local" is the
// time.Local variable, set per case the way the Rust tests set theirs.
//
// Regenerate (from the repository root):
//
//	go run ./contracts/tools/timegolden \
//	  -tables crates/gorse-common/src/dateparse/tables.rs \
//	  -errno crates/gorse-common/src/gotime/errno.rs \
//	  -zoneinfo crates/gorse-common/tests/data/zoneinfo \
//	  -goroot crates/gorse-common/tests/data/goroot \
//	  -time-out crates/gorse-common/tests/data/gotime_golden.json \
//	  -dateparse-out crates/gorse-common/tests/data/dateparse_golden.json
package main

import (
	"archive/zip"
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"math"
	"math/rand"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strconv"
	"strings"
	"syscall"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/araddon/dateparse"
)

// zones are the IANA zones the goldens and the Rust tests load.
var zones = []string{
	"America/Los_Angeles",
	"America/Denver",
	"America/New_York",
	"America/Sao_Paulo",
	"America/Nuuk",
	"America/Blanc-Sablon",
	"Asia/Baghdad",
	"Asia/Shanghai",
	"Asia/Kolkata",
	"Asia/Kathmandu",
	"Asia/Manila",
	"Asia/Singapore",
	"Asia/Tokyo",
	"Asia/Jerusalem",
	"Asia/Gaza",
	"Australia/Brisbane",
	"Australia/Lord_Howe",
	"Australia/Sydney",
	"Europe/Berlin",
	"Europe/Dublin",
	"Europe/London",
	"Europe/Moscow",
	"Europe/Zurich",
	"Pacific/Chatham",
	"Pacific/Fakaofo",
	"Africa/Casablanca",
	"Etc/GMT+1",
	"PST8PDT",
	"US/Eastern",
	"UTC",
}

var tzdata = map[string][]byte{}

func main() {
	tables := flag.String("tables", "", "write the dateparse Unicode tables (Rust) here")
	errno := flag.String("errno", "", "write the syscall.Errno texts (Rust) here")
	zoneinfo := flag.String("zoneinfo", "", "write the TZif files into this directory")
	goroot := flag.String("goroot", "", "write lib/time/zoneinfo.zip under this directory")
	timeOut := flag.String("time-out", "", "write the time package golden JSON here")
	dateparseOut := flag.String("dateparse-out", "", "write the dateparse golden JSON here")
	flag.Parse()

	if err := loadTzdata(); err != nil {
		fail(err)
	}
	if *tables != "" {
		must(os.WriteFile(*tables, []byte(unicodeTables()), 0o644))
	}
	if *errno != "" {
		must(os.WriteFile(*errno, []byte(errnoTable()), 0o644))
	}
	if *zoneinfo != "" {
		must(writeZoneinfo(*zoneinfo))
	}
	if *goroot != "" {
		must(writeZoneinfoZip(*goroot))
	}
	if *timeOut != "" {
		must(writeJSON(*timeOut, timeGolden()))
	}
	if *dateparseOut != "" {
		must(writeJSON(*dateparseOut, dateparseGolden()))
	}
}

func fail(err error) {
	fmt.Fprintln(os.Stderr, err)
	os.Exit(1)
}

func must(err error) {
	if err != nil {
		fail(err)
	}
}

// writeJSON writes v, a struct, as a JSON object with one line per scalar
// field and one line per element of each slice field, so that a regenerated
// golden diffs line by line.
func writeJSON(path string, v any) error {
	compact := func(x any) (string, error) {
		var buf bytes.Buffer
		enc := json.NewEncoder(&buf)
		enc.SetEscapeHTML(false)
		if err := enc.Encode(x); err != nil {
			return "", err
		}
		return strings.TrimSuffix(buf.String(), "\n"), nil
	}
	var b strings.Builder
	b.WriteString("{\n")
	rv := reflect.ValueOf(v)
	rt := rv.Type()
	for i := 0; i < rt.NumField(); i++ {
		name, _, _ := strings.Cut(rt.Field(i).Tag.Get("json"), ",")
		fmt.Fprintf(&b, "%q: ", name)
		f := rv.Field(i)
		if f.Kind() == reflect.Slice && f.Type().Elem().Kind() == reflect.Struct {
			b.WriteString("[\n")
			for j := 0; j < f.Len(); j++ {
				line, err := compact(f.Index(j).Interface())
				if err != nil {
					return err
				}
				b.WriteString(line)
				if j+1 < f.Len() {
					b.WriteString(",")
				}
				b.WriteString("\n")
			}
			b.WriteString("]")
		} else {
			line, err := compact(f.Interface())
			if err != nil {
				return err
			}
			b.WriteString(line)
		}
		if i+1 < rt.NumField() {
			b.WriteString(",")
		}
		b.WriteString("\n")
	}
	b.WriteString("}\n")
	return os.WriteFile(path, []byte(b.String()), 0o644)
}

// loadTzdata reads the TZif files of zones from the toolchain's zoneinfo.zip.
func loadTzdata() error {
	r, err := zip.OpenReader(filepath.Join(runtime.GOROOT(), "lib", "time", "zoneinfo.zip"))
	if err != nil {
		return err
	}
	defer r.Close()
	want := map[string]bool{}
	for _, z := range zones {
		want[z] = true
	}
	for _, f := range r.File {
		if !want[f.Name] {
			continue
		}
		rc, err := f.Open()
		if err != nil {
			return err
		}
		data, err := io.ReadAll(rc)
		rc.Close()
		if err != nil {
			return err
		}
		tzdata[f.Name] = data
	}
	for _, z := range zones {
		if tzdata[z] == nil {
			return fmt.Errorf("zone %s not in zoneinfo.zip", z)
		}
	}
	return nil
}

func writeZoneinfo(dir string) error {
	for _, z := range zones {
		path := filepath.Join(dir, filepath.FromSlash(z))
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			return err
		}
		if err := os.WriteFile(path, tzdata[z], 0o644); err != nil {
			return err
		}
	}
	return nil
}

// writeZoneinfoZip writes goroot/lib/time/zoneinfo.zip with every zone. Go's
// zip reader only reads stored entries, like the zoneinfo.zip Go ships.
func writeZoneinfoZip(goroot string) error {
	dir := filepath.Join(goroot, "lib", "time")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	var buf bytes.Buffer
	w := zip.NewWriter(&buf)
	for _, z := range zones {
		hdr := &zip.FileHeader{Name: z, Method: zip.Store}
		hdr.Modified = time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC)
		f, err := w.CreateHeader(hdr)
		if err != nil {
			return err
		}
		if _, err := f.Write(tzdata[z]); err != nil {
			return err
		}
	}
	if err := w.Close(); err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(dir, "zoneinfo.zip"), buf.Bytes(), 0o644)
}

// location returns the *time.Location a golden case names: "UTC", "Local"
// (the time.Local variable), "Fixed:<name>:<offset>", or an IANA zone loaded
// from the committed TZif bytes.
func location(name string) *time.Location {
	switch {
	case name == "UTC":
		return time.UTC
	case name == "Local":
		return time.Local
	case strings.HasPrefix(name, "Fixed:"):
		rest := strings.TrimPrefix(name, "Fixed:")
		i := strings.LastIndexByte(rest, ':')
		offset, err := strconv.Atoi(rest[i+1:])
		if err != nil {
			fail(err)
		}
		return time.FixedZone(rest[:i], offset)
	}
	data, ok := tzdata[name]
	if !ok {
		fail(fmt.Errorf("unknown zone %q", name))
	}
	loc, err := time.LoadLocationFromTZData(name, data)
	if err != nil {
		fail(err)
	}
	return loc
}

// setLocal sets the time.Local variable the way the Rust tests call
// gotime::set_local.
func setLocal(name string) {
	if name == "UTC" {
		time.Local = time.UTC
		return
	}
	time.Local = location(name)
}

// ---------------------------------------------------------------------------
// Tables

func ranges(pred func(rune) bool) [][2]rune {
	var out [][2]rune
	start := rune(-1)
	for r := rune(0); r <= unicode.MaxRune+1; r++ {
		in := r <= unicode.MaxRune && !(r >= 0xD800 && r <= 0xDFFF) && pred(r)
		switch {
		case in && start < 0:
			start = r
		case !in && start >= 0:
			out = append(out, [2]rune{start, r - 1})
			start = -1
		}
	}
	return out
}

func writeRanges(b *strings.Builder, name, doc string, rs [][2]rune) {
	fmt.Fprintf(b, "/// %s\npub(super) static %s: &[(u32, u32)] = &[\n", doc, name)
	for _, r := range rs {
		fmt.Fprintf(b, "    (0x%x, 0x%x),\n", r[0], r[1])
	}
	b.WriteString("];\n\n")
}

func unicodeTables() string {
	var b strings.Builder
	b.WriteString("// Code generated by contracts/tools/timegolden -tables; DO NOT EDIT.\n")
	fmt.Fprintf(&b, "// Ports: unicode.IsLetter, unicode.IsDigit, strconv.IsPrint and unicode.ToLower @ %s (Unicode %s)\n\n", runtime.Version(), unicode.Version)
	b.WriteString("//! Unicode predicates as Go answers them, enumerated over every code point\n")
	b.WriteString("//! (surrogates excluded: a Rust `char` cannot hold one, and Go's UTF-8\n")
	b.WriteString("//! decoder never yields one). Inclusive ranges, sorted.\n\n")
	writeRanges(&b, "LETTER", "Code points for which Go's `unicode.IsLetter` is true.", ranges(unicode.IsLetter))
	writeRanges(&b, "DIGIT", "Code points for which Go's `unicode.IsDigit` is true.", ranges(unicode.IsDigit))
	writeRanges(&b, "PRINT", "Code points for which Go's `strconv.IsPrint` is true.", ranges(strconv.IsPrint))
	b.WriteString("/// Non-ASCII code points that Go's `unicode.ToLower` maps to an ASCII byte.\n")
	b.WriteString("pub(super) static LOWER_TO_ASCII: &[(u32, u8)] = &[\n")
	for r := rune(utf8.RuneSelf); r <= unicode.MaxRune; r++ {
		if r >= 0xD800 && r <= 0xDFFF {
			continue
		}
		if l := unicode.ToLower(r); l < utf8.RuneSelf {
			fmt.Fprintf(&b, "    (0x%x, 0x%02x),\n", r, l)
		}
	}
	b.WriteString("];\n")
	return b.String()
}

func errnoTable() string {
	var b strings.Builder
	b.WriteString("// Code generated by contracts/tools/timegolden -errno; DO NOT EDIT.\n")
	fmt.Fprintf(&b, "// Ports: syscall.Errno.Error (zerrors_%s_%s.go) @ %s\n\n", runtime.GOOS, runtime.GOARCH, runtime.Version())
	b.WriteString("//! The text Go's `syscall.Errno.Error` gives for each errno, which is what\n")
	b.WriteString("//! `time.LoadLocation` reports when reading a zone file fails.\n\n")
	b.WriteString("/// `(errno, text)`, sorted by errno.\n")
	b.WriteString("pub(super) static ERRNO_TEXT: &[(i32, &str)] = &[\n")
	for i := 1; i < 256; i++ {
		text := syscall.Errno(i).Error()
		if text == "errno "+strconv.Itoa(i) {
			continue
		}
		fmt.Fprintf(&b, "    (%d, %s),\n", i, strconv.Quote(text))
	}
	b.WriteString("];\n")
	return b.String()
}

// ---------------------------------------------------------------------------
// Time results

// TimeResult describes a time.Time so that both sides can compare it exactly.
type TimeResult struct {
	Unix     string `json:"unix"`
	Nsec     int    `json:"nsec"`
	Zone     string `json:"zone"`
	Offset   int    `json:"offset"`
	Location string `json:"location"`
	String   string `json:"string"`
}

func describe(t time.Time) *TimeResult {
	name, offset := t.Zone()
	return &TimeResult{
		Unix:     strconv.FormatInt(t.Unix(), 10),
		Nsec:     t.Nanosecond(),
		Zone:     name,
		Offset:   offset,
		Location: t.Location().String(),
		String:   t.String(),
	}
}

func catch(f func()) (panicked string) {
	defer func() {
		if r := recover(); r != nil {
			panicked = fmt.Sprint(r)
		}
	}()
	f()
	return ""
}

// ---------------------------------------------------------------------------
// time package golden

// FormatCase is one instant in one location, formatted with every layout.
type FormatCase struct {
	Unix     string   `json:"unix"`
	Nsec     int      `json:"nsec"`
	Location string   `json:"location"`
	Outputs  []string `json:"outputs"`
	String   string   `json:"string"`
	Date     [3]int   `json:"date"`
	Clock    [3]int   `json:"clock"`
	Weekday  int      `json:"weekday"`
	YearDay  int      `json:"yday"`
	JSON     string   `json:"json"`
	JSONErr  string   `json:"json_err"`
}

// ParseCase is one Parse or ParseInLocation call.
type ParseCase struct {
	Local    string      `json:"local"`
	Layout   string      `json:"layout"`
	Value    string      `json:"value"`
	In       string      `json:"in"`
	Result   *TimeResult `json:"result"`
	Err      string      `json:"err"`
	ErrElems []string    `json:"err_elems"`
}

// DateCase is one time.Date call.
type DateCase struct {
	Args     [7]int      `json:"args"`
	Location string      `json:"location"`
	Result   *TimeResult `json:"result"`
}

// DurationCase is one Duration.String call.
type DurationCase struct {
	D      string `json:"d"`
	String string `json:"string"`
}

// ParseDurationCase is one ParseDuration call.
type ParseDurationCase struct {
	In  string `json:"in"`
	D   string `json:"d"`
	Err string `json:"err"`
}

// UnmarshalCase is one Time.UnmarshalJSON call.
type UnmarshalCase struct {
	Local  string      `json:"local"`
	In     string      `json:"in"`
	Result *TimeResult `json:"result"`
	Err    string      `json:"err"`
}

// TimeGolden is the content of gotime_golden.json.
type TimeGolden struct {
	GoVersion      string              `json:"go_version"`
	Local          string              `json:"local"`
	Layouts        []string            `json:"layouts"`
	Format         []FormatCase        `json:"format"`
	Parse          []ParseCase         `json:"parse"`
	Date           []DateCase          `json:"date"`
	DurationString []DurationCase      `json:"duration_string"`
	ParseDuration  []ParseDurationCase `json:"parse_duration"`
	Unmarshal      []UnmarshalCase     `json:"unmarshal"`
}

var layouts = []string{
	time.Layout, time.ANSIC, time.UnixDate, time.RubyDate, time.RFC822, time.RFC822Z,
	time.RFC850, time.RFC1123, time.RFC1123Z, time.RFC3339, time.RFC3339Nano,
	time.Kitchen, time.Stamp, time.StampMilli, time.StampMicro, time.StampNano,
	time.DateTime, time.DateOnly, time.TimeOnly,
	"2006-01-02 15:04:05.999999999 -0700 MST",
	"Jan  2 002 __2 2",
	"2006 6 06 _6 __6 ___6 _2006",
	"Jan January 1 01 _1 Janet Monday Mon Month",
	"2 02 _2 __2 002",
	"15 3 03 _3 PM pm",
	"4 04 _4 5 05 _5",
	".0 .00 .000 .000000 .000000000 .9 .99 .999 .999999 .999999999",
	",0 ,00 ,000 ,999 ,999999999",
	"15:04:05,000 15:04:05,999",
	"Z0700 Z07:00 Z07 Z070000 Z07:00:00",
	"-0700 -07:00 -07 -070000 -07:00:00",
	"MST",
	"2006-01-02T15:04:05.000Z07:00",
	"2006-002",
	"200601021504",
	"3:4:5",
	"",
	"no layout here",
}

func timeGolden() TimeGolden {
	const local = "America/Los_Angeles"
	setLocal(local)
	g := TimeGolden{GoVersion: runtime.Version(), Local: local, Layouts: layouts}

	instants := [][2]int64{
		{0, 0},
		{0, 1},
		{-1, 999999999},
		{1233810057, 12345600},
		{1136239445, 0},
		{1221681866, 300000000},
		{2159200800, 0},
		{2152173599, 0},
		{2152173600, 0},
		{2172733199, 0},
		{2172733200, 0},
		{951782400, 500000000},    // 2000-02-29
		{-62135596800, 0},         // year 1
		{-62135596801, 0},         // year 0
		{-62198755200, 123},       // year -1
		{253402300799, 999999999}, // 9999-12-31
		{253402300800, 0},         // 10000-01-01
		{-11644473600, 0},         // 1601
		{-2208988800, 0},          // 1900
		{1792209524, 902094534},   // 2026-10-17
		{4102444800, 1},           // 2100
		{1 << 40, 7},              // far future
		{-1 << 40, 7},             // far past
		{math.MaxInt64 / 2, 0},    // wraps in absolute time
		{math.MinInt64 / 2, 0},    // wraps in absolute time
	}
	locs := []string{
		"UTC", "Local", "Fixed::0", "Fixed:XYZ:0", "Fixed::3600", "Fixed:LMT:-19845",
		"Fixed::-1", "Fixed:OTO:15600", "Fixed::86400", "Fixed:BIG:-443520",
		"Asia/Kolkata", "Australia/Lord_Howe", "Europe/Dublin", "Asia/Kathmandu",
		"Pacific/Chatham", "America/Sao_Paulo",
	}
	for _, in := range instants {
		for _, l := range locs {
			t := time.Unix(in[0], in[1]).In(location(l))
			c := FormatCase{Unix: strconv.FormatInt(in[0], 10), Nsec: int(in[1]), Location: l, String: t.String()}
			for _, layout := range layouts {
				c.Outputs = append(c.Outputs, t.Format(layout))
			}
			y, m, d := t.Date()
			c.Date = [3]int{y, int(m), d}
			h, mi, s := t.Clock()
			c.Clock = [3]int{h, mi, s}
			c.Weekday = int(t.Weekday())
			c.YearDay = t.YearDay()
			if b, err := t.MarshalJSON(); err != nil {
				c.JSONErr = err.Error()
			} else {
				c.JSON = string(b)
			}
			g.Format = append(g.Format, c)
		}
	}

	g.Parse = parseCases()
	g.Date = dateCases()

	durations := []int64{
		0, 1, 999, 1000, 1001, 1100, 999999, 1000000, 2200000, 999999999, 1e9, 3300000000,
		245000000000, 245001000000, 18367001000000, 480000000001, math.MaxInt64, math.MinInt64,
		-1, -1000, -1e9, -3600e9, 3600e9, 86400e9 * 365, 1<<53 + 1, 123456789012345,
	}
	for _, d := range durations {
		g.DurationString = append(g.DurationString, DurationCase{strconv.FormatInt(d, 10), time.Duration(d).String()})
	}
	for _, s := range parseDurationInputs() {
		c := ParseDurationCase{In: s}
		if d, err := time.ParseDuration(s); err != nil {
			c.Err = err.Error()
		} else {
			c.D = strconv.FormatInt(int64(d), 10)
		}
		g.ParseDuration = append(g.ParseDuration, c)
	}

	for _, local := range []string{"UTC", "America/Los_Angeles"} {
		setLocal(local)
		for _, in := range unmarshalInputs() {
			c := UnmarshalCase{Local: local, In: in}
			var t time.Time
			if err := t.UnmarshalJSON([]byte(in)); err != nil {
				c.Err = err.Error()
			} else {
				c.Result = describe(t)
			}
			g.Unmarshal = append(g.Unmarshal, c)
		}
	}
	setLocal(local)
	return g
}

func unmarshalInputs() []string {
	return []string{
		`null`, `{}`, `[]`, `""`, `"x"`, `1`,
		`"2000-01-01T1:12:34Z"`, `"2000-01-01T00:00:00,000Z"`, `"2000-01-01T00:00:00+24:00"`,
		`"2000-01-01T00:00:00+00:60"`, `"2000-01-01T00:00:00+123:45"`,
		`"9999-04-12T23:20:50.52Z"`, `"1996-12-19T16:39:57-08:00"`, `"1996-12-19T16:39:57-07:00"`,
		`"0000-01-01T00:00:00.000000001+00:01"`, `"2020-01-01T00:00:00+23:59"`,
		`"2026-10-17T02:58:44.902094534Z"`, `"2026-10-17T02:58:44.9020945341234Z"`,
		`"2026-10-17T02:58:44+05:30"`, `"2026-10-17t02:58:44Z"`, `"2026-10-17T02:58:44z"`,
		`"2026-10-17T02:58:44"`, `"2026-02-29T00:00:00Z"`, `"2024-02-29T00:00:00Z"`,
		`"2026-10-17T24:00:00Z"`, `"2026-10-17T02:58:60Z"`, `"10000-01-01T00:00:00Z"`,
		`"2026-10-17 02:58:44Z"`, `"2026-10-17T02:58:44.Z"`, `"2026-10-17T02:58:44.123-08:00"`,
	}
}

func parseDurationInputs() []string {
	return []string{
		"0", "5s", "30s", "1478s", "-5s", "+5s", "-0", "+0", "5.0s", "5.6s", "5.s", ".5s",
		"1.0s", "1.00s", "1.004s", "1.0040s", "100.00100s", "10ns", "11us", "12µs", "12μs",
		"13ms", "14s", "15m", "16h", "3h30m", "10.5s4m", "-2m3.4s", "1h2m3s4ms5us6ns",
		"39h9m14.425s", "52763797000ns", "0.3333333333333333333h", "9007199254740993ns",
		"9223372036854775807ns", "9223372036854775.807us", "9223372036s854ms775us807ns",
		"-9223372036854775808ns", "-9223372036854775.808us", "-9223372036s854ms775us808ns",
		"-2562047h47m16.854775808s", "0.100000000000000000000h", "0.830103483285477580700h",
		"", "3", "-", "s", ".", "-.", ".s", "+.s", "1d", "�", "� hello � world",
		"9223372036854775810ns", "9223372036854775808ns", "-9223372036854775809ns",
		"9223372036854776us", "3000000h", "9223372036854775.808us", "9223372036854ms775us808ns",
		"24h", "168h", "1h1h", "1.5h", "2562047h47m16.854775807s", "2562047h47m16.854775808s",
		"1e3s", "1 s", " 1s", "1s ", "1S", "1Ms", "1m1", "m", "1.2.3s", "00001s", "+-1s",
	}
}

func parseCases() []ParseCase {
	var cases []ParseCase
	add := func(local, layout, value, in string) {
		setLocal(local)
		c := ParseCase{Local: local, Layout: layout, Value: value, In: in}
		var t time.Time
		var err error
		if in == "" {
			t, err = time.Parse(layout, value)
		} else {
			t, err = time.ParseInLocation(layout, value, location(in))
		}
		if err != nil {
			c.Err = err.Error()
			if pe, ok := err.(*time.ParseError); ok {
				c.ErrElems = []string{pe.Layout, pe.Value, pe.LayoutElem, pe.ValueElem, pe.Message}
			}
		} else {
			c.Result = describe(t)
		}
		cases = append(cases, c)
	}
	type lv struct{ layout, value string }
	pairs := []lv{
		{time.ANSIC, "Thu Feb  4 21:00:57 2010"},
		{time.UnixDate, "Thu Feb  4 21:00:57 PST 2010"},
		{time.UnixDate, "Thu Feb  4 21:00:57 MST 2010"},
		{time.UnixDate, "Thu Feb  4 21:00:57 MDT 2010"},
		{time.UnixDate, "Thu Feb  4 21:00:57 UTC 2010"},
		{time.UnixDate, "Thu Feb  4 21:00:57 CET 2010"},
		{time.UnixDate, "Thu Feb  4 21:00:57 CEST 2010"},
		{time.UnixDate, "Thu Feb  4 21:00:57 IST 2010"},
		{time.UnixDate, "Thu Feb  4 21:00:57 GMT 2010"},
		{time.UnixDate, "Fri Feb  5 05:00:57 GMT-8 2010"},
		{time.UnixDate, "Fri Feb  5 05:00:57 GMT+12 2010"},
		{time.UnixDate, "Fri Feb  5 05:00:57 GMT+24 2010"},
		{time.UnixDate, "Fri Feb  5 05:00:57 +03 2010"},
		{time.UnixDate, "Fri Feb  5 05:00:57 -11 2010"},
		{time.UnixDate, "Fri Feb  5 05:00:57 ChST 2010"},
		{time.UnixDate, "Fri Feb  5 05:00:57 WITA 2010"},
		{time.UnixDate, "Fri Feb  5 05:00:57 ESAST 2010"},
		{time.UnixDate, "Fri Feb  5 05:00:57 ESASTT 2010"},
		{time.RubyDate, "Thu Feb 04 21:00:57 -0800 2010"},
		{time.RubyDate, "Thu Feb 04 21:00:57 -0700 2010"},
		{time.RubyDate, "Thu Feb 04 21:00:57 +0000 2010"},
		{time.RubyDate, "Thu Feb 04 21:00:57 -0000 2010"},
		{time.RubyDate, "Thu Feb 04 21:00:57 +1130 2010"},
		{time.RubyDate, "Mon Jan 02 15:04:05 +0123 2006"},
		{time.RFC850, "Thursday, 04-Feb-10 21:00:57 PST"},
		{time.RFC850, "Thursday, 04-Feb-1 21:00:57 PST"},
		{time.RFC1123, "Thu, 04 Feb 2010 21:00:57 PST"},
		{time.RFC1123, "Thu, 04 Feb 2010 22:00:57 PDT"},
		{time.RFC1123, "Thu, 4 Feb 2010 22:00:57 PDT"},
		{time.RFC1123Z, "Thu, 04 Feb 2010 21:00:57 -0800"},
		{time.RFC3339, "2010-02-04T21:00:57-08:00"},
		{time.RFC3339, "2010-02-04T21:00:57-07:00"},
		{time.RFC3339, "2010-02-04T21:00:57Z"},
		{time.RFC3339, "2010-02-04T21:00:57.012345678-08:00"},
		{time.RFC3339, "2010-02-04T21:00:57.0123456789-08:00"},
		{time.RFC3339, "2010-02-04T21:00:57,5Z"},
		{time.RFC3339, "2010-02-04T1:00:57Z"},
		{time.RFC3339, "2010-02-04T21:00:57+24:00"},
		{time.RFC3339, "2010-02-04T21:00:57+00:60"},
		{time.RFC3339, "2010-02-30T21:00:57Z"},
		{time.RFC3339, "2006-01-02T15:04:05Z07:00"},
		{time.RFC3339, "2006-01-02T15:04_abc"},
		{time.RFC3339, "2006-01-02T15:04:05_abc"},
		{time.RFC3339, "2006-01-02T15:04:05Z_abc"},
		{time.RFC3339, "2010-02-04T21:00:67.012345678-08:00"},
		{time.RFC3339, "0000-01-01T00:00:.0+00:00"},
		{time.RFC3339, "\""},
		{time.RFC3339, "0000-01-01T00:00:00+00:+0"},
		{time.RFC3339, "0000-01-01T00:00:00+-0:00"},
		{time.RFC3339Nano, "2021-09-29T16:04:33.9999999999999999Z"},
		{time.RFC3339Nano, "2021-09-29T16:04:33.000123456789Z"},
		{"2006-01-02 15:04:05-07", "2010-02-04 21:00:57-08"},
		{"2006-01-02 15:04:05", "2010-02-04 21:00:57.0"},
		{"2006-01-02 15:04:05", "2010-02-04 21:00:57,123456"},
		{"2006.01.02.15.04.05.0", "2010.02.04.21.00.57.0"},
		{"2006.01.02.15.04.05.00", "2010.02.04.21.00.57.01"},
		{"Hi Janet, the Month is January: Jan _2 15:04:05 2006", "Hi Janet, the Month is February: Feb  4 21:00:57 2010"},
		{"2006-01-02 15:04:05.9999 -0700 MST", "2010-02-04 21:00:57 -0800 PST"},
		{"2006-01-02 15:04:05.999999999 -0700 MST", "2010-02-04 21:00:57.0123 -0800 PST"},
		{"2006-01-02 15:04:05,9999 -0700 MST", "2010-02-04 21:00:57.012345678 -0800 PST"},
		{"2006-01-02 15:04:05.999999999 -0700 MST", "2026-10-17 02:58:44.902094534 +0000 UTC"},
		{"2006-01-02 15:04:05.999999999 -0700 MST", "2026-10-17 10:58:44.902094534 +0800 CST"},
		{"2006-01-02 15:04:05.999999999 -0700 MST", "2026-10-17 10:58:44.902094534 +0800 +08"},
		{"2006-01-02 15:04:05.999999999 -0700 MST", "2026-10-16 19:58:44.902094534 -0700 PDT"},
		{"2006-01-02 15:04:05.999999999 -0700 MST", "2026-10-16 19:58:44.902094534 -0800 PDT"},
		{"2006-01-02 15:04:05.999999999 -0700 MST", "2026-01-16 19:58:44 -0800 PST"},
		{time.StampNano, "Feb  4 21:00:57.012345678"},
		{time.StampNano, "Dec  7 11:22:01.000000"},
		{time.StampNano, "Dec  7 11:22:01.0000000000"},
		{"Jan _2 15:04:05.999", "Feb  4 21:00:57.012300000"},
		{"Jan _2 15:04:05.999999999", "Feb  4 21:00:57.0123"},
		{"2006-01-02 002 15:04:05", "2010-02-04 035 21:00:57"},
		{"2006-01 002 15:04:05", "2010-02 035 21:00:57"},
		{"2006-002 15:04:05", "2010-035 21:00:57"},
		{"200600201 15:04:05", "201003502 21:00:57"},
		{"2006-002", "2020-060"},
		{"2006-002", "2020-366"},
		{"2006-002", "2021-366"},
		{"2006-002", "2021-000"},
		{"Jan _2 002 2006", "Feb  4 034 2006"},
		{"Jan _2 002 2006", "Feb  4 004 2006"},
		{"2006-01-02T15:04:05Z07", "2010-02-04T21:00:57Z"},
		{"2006-01-02T15:04:05Z07", "2010-02-04T21:00:57+08"},
		{"2006-01-02T15:04:05Z0700", "2010-02-04T21:00:57-0800"},
		{"2006-01-02T15:04:05Z070000", "2010-02-04T21:00:57+080000"},
		{"2006-01-02T15:04:05Z07:00:00", "2010-02-04T21:00:57-08:00:00"},
		{"2006-01-02T15:04:05-070000", "1871-01-01T05:33:02-003408"},
		{"2006-01-02T15:04:05-07:00:00", "1871-01-01T05:33:02+00:34:08"},
		{"-07", "-25"},
		{"-07:00", "+25:00"},
		{"-07:00", "-23:61"},
		{"-07:00:00", "+23:59:61"},
		{"Z07:00", "+25:00"},
		{"Z07", "-25"},
		{"-0700", "+2400"},
		{"-0700", "x0100"},
		{"01-02", "00-01"},
		{"01-02", "13-01"},
		{"01-02", "01-01"},
		{"01 MST", "0 MST"},
		{"01 MST", "1 MST"},
		{"3:04PM", "12:00PM"},
		{"03:04PM", "12:00AM"},
		{"03:04pm", "12:03pM"},
		{"03:04PM", "12:03pM"},
		{"3:04PM", "13:00PM"},
		{"15:04_20060102", "14:38_20150618"},
		{"2006-01-02", "22-10-25"},
		{"06-01-02", "a2-10-25"},
		{"06-01-02", "68-10-25"},
		{"06-01-02", "69-10-25"},
		{"_2 Jan 06 15:04 MST", "4 --- 00 00:00 GMT"},
		{"_2 January 06 15:04 MST", "4 --- 00 00:00 GMT"},
		{"_2 January 06 15:04 MST", "4 jANUARY 00 00:00 GMT"},
		{`"2006-01-02T15:04:05Z07:00"`, "0"},
		{time.ANSIC, "Thu Jan 99 21:00:57 2010"},
		{time.ANSIC, "Thu Feb 29 21:00:57 2012"},
		{time.ANSIC, "Thu Feb 29 21:00:57 2010"},
		{time.ANSIC, "Thu Dec 00 21:00:57 2010"},
		{time.ANSIC, "THU FEB 4 21:00:57 2010"},
		{time.ANSIC, "thu feb 4 21:00:57 2010"},
		{time.ANSIC, "Thu      Feb     4     21:00:57     2010"},
		{time.ANSIC, "Feb  4 21:00:60 2010"},
		{time.ANSIC, "Thu Feb  4 21:00:57 @2010"},
		{time.ANSIC, "Thu Feb  4 21:00:60 2010"},
		{time.ANSIC, "Thu Feb  4 21:61:57 2010"},
		{time.ANSIC, "Thu Feb  4 24:00:60 2010"},
		{"Mon Jan _2 15:04:05.000 2006", "Thu Feb  4 23:00:59x01 2010"},
		{"Mon Jan _2 15:04:05.000 2006", "Thu Feb  4 23:00:59.xxx 2010"},
		{"Mon Jan _2 15:04:05.000 2006", "Thu Feb  4 23:00:59.-123 2010"},
		{"Mon Jan _2 15:04:05,000000 2006", "Thu Feb  4 21:00:57.012345 2010"},
		{"2006", "0000"},
		{"2006", "-001"},
		{"2006", "99999"},
		{"2006 MST", "2010 ABCDEF"},
		{"2006 MST", "2010 AB"},
		{"2006 MST", "2010 ABC1"},
		{"2006 -0700 MST", "2010 -0800 PST"},
		{"2006 -0700 MST", "2010 -0700 PST"},
		{"2006 -0700 MST", "2010 -0800 XYZ"},
		{"", ""},
		{"", "x"},
		{"x", ""},
		{"Jan 2006", "Febé 2010"},
		{"Jan 2006", "é 2010"},
		{"Mon 2006", "Kon 2010"},
	}
	for _, local := range []string{"UTC", "America/Los_Angeles", "America/Denver", "Europe/Berlin"} {
		for _, p := range pairs {
			add(local, p.layout, p.value, "")
		}
	}
	inLocs := []string{"UTC", "Local", "America/Denver", "Asia/Baghdad", "America/Blanc-Sablon", "Fixed:XYZ:3600", "Asia/Shanghai"}
	for _, in := range inLocs {
		for _, p := range pairs {
			add("America/Los_Angeles", p.layout, p.value, in)
		}
	}
	add("UTC", "Jan 02 2006 MST", "Feb 01 2013 AST", "Asia/Baghdad")
	add("UTC", "Jan 02 2006 MST", "Feb 01 2013 AST", "America/Blanc-Sablon")
	// Every day of a leap year through the day-of-year path.
	for i := 1; i <= 366; i += 7 {
		add("UTC", "2006-002", fmt.Sprintf("2020-%03d", i), "")
	}

	// Formatted then mutated values, through the layout that wrote them.
	rng := rand.New(rand.NewSource(1403))
	setLocal("America/Los_Angeles")
	for i := 0; i < 400; i++ {
		layout := layouts[rng.Intn(len(layouts))]
		t := time.Unix(rng.Int63n(1<<35)-(1<<34), rng.Int63n(1e9)).In(location([]string{"UTC", "Local", "Asia/Kolkata", "Fixed:ABC:-12345"}[rng.Intn(4)]))
		value := mutate(rng, t.Format(layout))
		add("America/Los_Angeles", layout, value, "")
	}
	return cases
}

func dateCases() []DateCase {
	setLocal("America/Los_Angeles")
	args := [][7]int{
		{2011, 11, 6, 1, 0, 0, 0},
		{2011, 11, 6, 1, 59, 59, 0},
		{2011, 11, 6, 2, 0, 0, 0},
		{2011, 3, 13, 1, 0, 0, 0},
		{2011, 3, 13, 1, 59, 59, 0},
		{2011, 3, 13, 3, 0, 0, 0},
		{2011, 3, 13, 2, 30, 0, 0},
		{2012, 12, 24, 0, 0, 0, 0},
		{2011, 11, 18, 7, 56, 35, 0},
		{2011, 11, 19, -17, 56, 35, 0},
		{2011, 11, 17, 31, 56, 35, 0},
		{2011, 11, 18, 6, 116, 35, 0},
		{2011, 10, 49, 7, 56, 35, 0},
		{2011, 11, 18, 7, 55, 95, 0},
		{2011, 11, 18, 7, 56, 34, 1e9},
		{2011, 12, -12, 7, 56, 35, 0},
		{2012, 1, -43, 7, 56, 35, 0},
		{2012, -1, 18, 7, 56, 35, 0},
		{2010, 23, 18, 7, 56, 35, 0},
		{1970, 1, 15297, 7, 56, 35, 0},
		{1970, 1, -25508, 0, 0, 0, 0},
		{0, 1, 1, 0, 0, 0, 0},
		{-1, 1, 1, 0, 0, 0, 0},
		{-400, 3, 1, 0, 0, 0, 0},
		{1, 1, 1, 0, 0, 0, -1},
		{9999, 12, 31, 23, 59, 59, 999999999},
		{math.MaxInt32, 1, 1, 0, 0, 0, 0},
		{math.MinInt32, 1, 1, 0, 0, 0, 0},
		{2026, 10, 17, 2, 58, 44, 902094534},
		{2026, 3, 29, 1, 30, 0, 0},
		{2026, 10, 25, 1, 30, 0, 0},
		{1991, 4, 14, 2, 30, 0, 0},
		{1991, 9, 15, 1, 30, 0, 0},
		{2026, 2, 29, 0, 0, 0, 0},
		{2026, 13, 1, 0, 0, 0, 0},
		{2026, 0, 0, 0, 0, 0, 0},
		{2026, 1, 1, -1, -1, -1, -1},
		{2026, 1, 1, 0, 0, 0, math.MaxInt32},
		{2026, 1, 1, 0, 0, 0, math.MinInt32},
	}
	locs := []string{"UTC", "Local", "Fixed:LMT:-19845", "Europe/Berlin", "Europe/Dublin", "Asia/Shanghai", "Australia/Lord_Howe", "Pacific/Chatham", "America/Sao_Paulo"}
	var cases []DateCase
	for _, a := range args {
		for _, l := range locs {
			t := time.Date(a[0], time.Month(a[1]), a[2], a[3], a[4], a[5], a[6], location(l))
			cases = append(cases, DateCase{Args: a, Location: l, Result: describe(t)})
		}
	}
	return cases
}

// mutate applies one random edit to s, at byte granularity like the parsers
// see it, keeping the result valid UTF-8 so both sides can take it as a
// string.
func mutate(rng *rand.Rand, s string) string {
	const alphabet = "0123456789 -+:.,/TZtzAPMamp年月日'()_"
	for tries := 0; tries < 10; tries++ {
		b := []byte(s)
		switch op := rng.Intn(5); {
		case op == 0 && len(b) > 0: // delete
			i := rng.Intn(len(b))
			b = append(b[:i:i], b[i+1:]...)
		case op == 1: // insert
			i := rng.Intn(len(b) + 1)
			c := string([]rune(alphabet)[rng.Intn(utf8.RuneCountInString(alphabet))])
			b = append(b[:i:i], append([]byte(c), b[i:]...)...)
		case op == 2 && len(b) > 0: // replace
			i := rng.Intn(len(b))
			c := string([]rune(alphabet)[rng.Intn(utf8.RuneCountInString(alphabet))])
			b = append(b[:i:i], append([]byte(c), b[i+1:]...)...)
		case op == 3 && len(b) > 1: // swap
			i := rng.Intn(len(b) - 1)
			b[i], b[i+1] = b[i+1], b[i]
		case op == 4 && len(b) > 0: // truncate
			b = b[:rng.Intn(len(b))]
		}
		if utf8.Valid(b) {
			return string(b)
		}
	}
	return s
}

// ---------------------------------------------------------------------------
// dateparse golden

// DateparseCase is one dateparse call.
type DateparseCase struct {
	Local  string      `json:"local"`
	Func   string      `json:"func"`
	In     string      `json:"in"`
	Opts   []string    `json:"opts,omitempty"`
	Loc    string      `json:"loc,omitempty"`
	Result *TimeResult `json:"result,omitempty"`
	Format string      `json:"format,omitempty"`
	Err    string      `json:"err,omitempty"`
	Panic  string      `json:"panic,omitempty"`
}

// DateparseGolden is the content of dateparse_golden.json.
type DateparseGolden struct {
	GoVersion string          `json:"go_version"`
	Cases     []DateparseCase `json:"cases"`
}

func options(names []string) []dateparse.ParserOption {
	var opts []dateparse.ParserOption
	for _, n := range names {
		switch n {
		case "PreferMonthFirst(true)":
			opts = append(opts, dateparse.PreferMonthFirst(true))
		case "PreferMonthFirst(false)":
			opts = append(opts, dateparse.PreferMonthFirst(false))
		case "RetryAmbiguousDateWithSwap(true)":
			opts = append(opts, dateparse.RetryAmbiguousDateWithSwap(true))
		case "RetryAmbiguousDateWithSwap(false)":
			opts = append(opts, dateparse.RetryAmbiguousDateWithSwap(false))
		default:
			fail(fmt.Errorf("unknown option %q", n))
		}
	}
	return opts
}

func runDateparse(local, fn, in, loc string, opts []string) DateparseCase {
	setLocal(local)
	c := DateparseCase{Local: local, Func: fn, In: in, Opts: opts, Loc: loc}
	var t time.Time
	var err error
	c.Panic = catch(func() {
		switch fn {
		case "ParseAny":
			t, err = dateparse.ParseAny(in, options(opts)...)
		case "ParseIn":
			t, err = dateparse.ParseIn(in, location(loc), options(opts)...)
		case "ParseLocal":
			t, err = dateparse.ParseLocal(in, options(opts)...)
		case "ParseStrict":
			t, err = dateparse.ParseStrict(in, options(opts)...)
		case "ParseFormat":
			c.Format, err = dateparse.ParseFormat(in, options(opts)...)
		default:
			fail(fmt.Errorf("unknown function %q", fn))
		}
	})
	if c.Panic != "" {
		return c
	}
	if err != nil {
		c.Err = err.Error()
	} else if fn != "ParseFormat" {
		c.Result = describe(t)
	}
	return c
}

// dateparseInputs are the inputs of dateparse's own tests
// (parseany_test.go @ 6b43995a97de), in their order.
var dateparseInputs = []string{
	"oct 7, 1970", "oct 7, '70", "Oct 7, '70", "Oct. 7, '70", "oct. 7, '70", "oct. 7, 1970",
	"Sept. 7, '70", "sept. 7, 1970", "Feb 8, 2009 5:57:51 AM", "May 8, 2009 5:57:51 PM",
	"May 8, 2009 5:57:1 PM", "May 8, 2009 5:7:51 PM", "May 8, 2009, 5:7:51 PM", "7 oct 70",
	"7 oct 1970", "7 May 1970", "7 Sep 1970", "7 June 1970", "7 September 1970",
	"Mon Jan  2 15:04:05 2006", "Thu May 8 17:57:51 2009", "Thu May  8 17:57:51 2009",
	"Mon 02 Jan 2006 03:04:05 PM UTC", "Mon 30 Sep 2018 09:09:09 PM UTC",
	"Mon Jan 02 15:04:05 -0700 2006", "Thu May 08 11:57:51 -0700 2009",
	"Mon Jan  2 15:04:05 MST 2006", "Thu May  8 17:57:51 MST 2009", "Thu May  8 17:57:51 PST 2009",
	"Thu May 08 17:57:51 PST 2009", "Thu May 08 17:57:51 CEST 2009", "Thu May 08 05:05:07 PST 2009",
	"Thu May 08 5:5:7 PST 2009", "Mon Aug 10 15:44:11 UTC+0000 2015", "Mon Aug 10 15:44:11 PST-0700 2015",
	"Mon Aug 10 15:44:11 CEST+0200 2015", "Mon Aug 1 15:44:11 CEST+0200 2015",
	"Mon Aug 1 5:44:11 CEST+0200 2015", "Fri Jul 03 2015 18:04:07 GMT+0100 (GMT Daylight Time)",
	"Fri Jul 3 2015 06:04:07 GMT+0100 (GMT Daylight Time)",
	"Fri Jul 3 2015 06:04:07 PST-0700 (Pacific Daylight Time)",
	"September 17, 2012 at 5:00pm UTC-05", "September 17, 2012 at 10:09am PST-08",
	"September 17, 2012, 10:10:09", "May 17, 2012 at 10:09am PST-08", "May 17, 2012 AT 10:09am PST-08",
	"September 17, 2012 5:00pm UTC-05", "September 17, 2012 10:09am PST-08",
	"September 17, 2012 09:01:00", "September 17 2012 5:00pm UTC-05", "September 17 2012 5:00pm UTC-0500",
	"September 17 2012 10:09am PST-08", "September 17 2012 5:00PM UTC-05",
	"September 17 2012 10:09AM PST-08", "September 17 2012 09:01:00", "May 17, 2012 10:10:09",
	"September 17, 2012", "May 7, 2012", "June 7, 2012", "June 7 2012", "September 17th, 2012",
	"September 17th 2012", "September 7th, 2012", "September 7th 2012", "September 7tH 2012",
	"May 1st 2012", "May 1st, 2012", "May 21st 2012", "May 21st, 2012", "May 23rd 2012",
	"May 23rd, 2012", "June 2nd, 2012", "June 2nd 2012", "June 22nd, 2012", "June 22nd 2012",
	"Fri, 03 Jul 2015 08:08:08 MST", "Fri, 03 Jul 2015 08:08:08 PST", "Fri, 3 Jul 2015 08:08:08 MST",
	"Fri, 03 Jul 2015 8:08:08 MST", "Fri, 03 Jul 2015 8:8:8 MST", "Thu, 03 Jul 2017 08:08:04 +0100",
	"Thu, 03 Jul 2017 08:08:04 -0100", "Thu, 3 Jul 2017 08:08:04 +0100", "Thu, 03 Jul 2017 8:08:04 +0100",
	"Thu, 03 Jul 2017 8:8:4 +0100", "Tue, 11 Jul 2017 04:08:03 +0200 (CEST)",
	"Tue, 5 Jul 2017 04:08:03 -0700 (CEST)", "Fri, 03-Jul-15 08:08:08 MST", "Fri, 03-Jul-15 08:08:08 PST",
	"Fri, 03-Jul 2015 08:08:08 PST", "Fri, 3-Jul-15 08:08:08 MST", "Fri, 03-Jul-15 8:08:08 MST",
	"Fri, 03-Jul-15 8:8:8 MST", "Sun, 3 Jan 2021 00:12:23 +0800 (GMT+08:00)",
	"Wednesday, 07-May-09 08:00:43 MST", "Wednesday, 28-Feb-18 09:01:00 MST",
	"Monday, 02 Jan 2006 15:04:05 +0100", "Wednesday, 28 Feb 2018 09:01:00 -0300",
	"Wednesday, 2 Feb 2018 09:01:00 -0300", "Wednesday, 2 Feb 2018 9:01:00 -0300",
	"Wednesday, 2 Feb 2018 09:1:00 -0300", "07 Feb 2004, 09:07", "07 Feb 2004, 09:07:07",
	"7 Feb 2004, 09:07:07", "07 Feb 2004, 9:7:7", "07 Feb 2004 09:07:08", "07 Feb 2004 09:07",
	"7 Feb 2004 9:7:8", "07 Feb 2004 09:07:08.123", "07 Feb 2004, 09:07:07 GMT",
	"07 Feb 2004, 09:07:07 +0100", "07-Feb-2004 09:07:07 +0100", "07-Feb-04 09:07:07 +0100",
	"2013-Feb-03", "03 February 2013", "3 February 2013", "2014年04月08日", "2014年04月08日 19:17:22",
	"03/31/2014", "3/31/2014", "3/5/2014", "08/08/71", "8/8/71", "04/02/2014 04:08:09",
	"4/2/2014 04:08:09", "04/02/2014 4:08:09", "04/02/2014 4:8:9", "04/02/2014 04:08", "04/02/2014 4:8",
	"04/02/2014 04:08:09.123", "04/02/2014 04:08:09.12312", "04/02/2014 04:08:09.123123",
	"04:02:2014 04:08:09", "4:2:2014 04:08:09", "04:02:2014 4:08:09", "04:02:2014 4:8:9",
	"04:02:2014 04:08", "04:02:2014 4:8", "04:02:2014 04:08:09.123", "04:02:2014 04:08:09.12312",
	"04:02:2014 04:08:09.123123", "04/02/2014 04:08:09 AM", "04/02/2014 04:08:09 PM",
	"04/02/2014 04:08 AM", "04/02/2014 04:08 PM", "04/02/2014 4:8 AM", "04/02/2014 4:8 PM",
	"04/02/2014 04:08:09.123 AM", "04/02/2014 04:08:09.123 PM", "2014/04/02", "2014/03/31", "2014/4/2",
	"2014/04/02 04:08", "2014/03/31 04:08", "2014/4/2 04:08", "2014/04/02 4:8", "2014/04/02 04:08:09",
	"2014/03/31 04:08:09", "2014/4/2 04:08:09", "2014/04/02 04:08:09.123", "2014/04/02 04:08:09.123123",
	"2014/04/02 04:08:09 AM", "2014/03/31 04:08:09 AM", "2014/4/2 04:08:09 AM",
	"2014/04/02 04:08:09.123 AM", "2014/04/02 04:08:09.123 PM", "06/May/2008:08:11:17 -0700",
	"30/May/2008:08:11:17 -0700", "2014-04-02", "2014-03-31", "2014-4-2", "2020-07-20+08:00",
	"2020-07-20+0800", "28-Feb-02", "15-Jan-18", "15-Jan-2017", "2014-04", "2014-04-02 04:08",
	"2014-03-31 04:08", "2014-4-2 04:08", "2014-04-02 4:8", "2014-04-02 04:08:09", "2014-03-31 04:08:09",
	"2014-4-2 04:08:09", "2014-04-02 04:08:09.123", "2014-04-02 04:08:09.123123",
	"2014-04-02 04:08:09.12312312", "2014-04-02 04:08:09 AM", "2014-03-31 04:08:09 AM",
	"2014-04-26 05:24:37 PM", "2014-4-2 04:08:09 AM", "2014-04-02 04:08:09.123 AM",
	"2014-04-02 04:08:09.123 PM", "2014-05-11 08:20:13,787", "2012-08-03 18:31:59 +0000",
	"2012-08-03 13:31:59 -0600", "2012-08-03 18:31:59.257000000 +0000", "2012-08-03 8:1:59.257000000 +0000",
	"2012-8-03 18:31:59.257000000 +0000", "2012-8-3 18:31:59.257000000 +0000",
	"2014-04-26 17:24:37.123456 +0000", "2014-04-26 17:24:37.12 +0000", "2014-04-26 17:24:37.1 +0000",
	"2014-05-11 08:20:13 +0000", "2014-05-11 08:20:13 +0530", "2018-06-29 19:09:57.77297118 +0300 +03",
	"2018-06-29 19:09:57.77297118 +0300 +0300", "2018-06-29 19:09:57 +0300 +03",
	"2018-06-29 19:09:57 +0300 +0300", "2012-08-03 18:31:59 +00:00", "2014-05-01 08:02:13 +00:00",
	"2014-5-01 08:02:13 +00:00", "2014-05-1 08:02:13 +00:00", "2012-08-03 13:31:59 -06:00",
	"2012-08-03 18:31:59.257000000 +00:00", "2012-08-03 8:1:59.257000000 +00:00",
	"2012-8-03 18:31:59.257000000 +00:00", "2012-8-3 18:31:59.257000000 +00:00",
	"2014-04-26 17:24:37.123456 +00:00", "2014-04-26 17:24:37.12 +00:00", "2014-04-26 17:24:37.1 +00:00",
	"2012-08-03 18:31:59 +0000 UTC", "2012-08-03 13:31:59 -0600 MST", "2015-02-18 00:12:00 +0000 UTC",
	"2015-02-18 00:12:00 +0000 GMT", "2015-02-08 03:02:00 +0200 CEST", "2015-02-08 03:02:00 +0300 MSK",
	"2015-2-08 03:02:00 +0300 MSK", "2015-02-8 03:02:00 +0300 MSK", "2015-2-8 03:02:00 +0300 MSK",
	"2012-08-03 18:31:59.257000000 +0000 UTC", "2012-08-03 8:1:59.257000000 +0000 UTC",
	"2012-8-03 18:31:59.257000000 +0000 UTC", "2012-8-3 18:31:59.257000000 +0000 UTC",
	"2014-04-26 17:24:37.123456 +0000 UTC", "2014-04-26 17:24:37.12 +0000 UTC",
	"2014-04-26 17:24:37.1 +0000 UTC", "2015-02-08 03:02:00 +0200 CEST m=+0.000000001",
	"2015-02-08 03:02:00 +0300 MSK m=+0.000000001", "2015-02-08 03:02:00.001 +0300 MSK m=+0.000000001",
	"2012-08-03 18:31:59 UTC", "2014-12-16 06:20:00 GMT", "2012-08-03 13:31:59 MST",
	"2012-08-03 18:31:59.257000000 UTC", "2012-08-03 8:1:59.257000000 UTC",
	"2012-8-03 18:31:59.257000000 UTC", "2012-8-3 18:31:59.257000000 UTC",
	"2014-04-26 17:24:37.123456 UTC", "2014-04-26 17:24:37.12 UTC", "2014-04-26 17:24:37.1 UTC",
	"2014-04-26 05:24:37 PST", "2012-08-03 18:31:59+00:00", "2017-07-19 03:21:51+00:00",
	"2012:08:03 18:31:59+00:00", "08:03:2012 18:31:59+00:00", "2012-08-03 18:31:59.000+00:00 PST",
	"2012-08-03 18:31:59 +00:00 UTC", "2012-08-03 13:31:51 -07:00 MST",
	"2012-08-03 18:31:59.257000000 +00:00 UTC", "2012-08-03 13:31:51.123 -08:00 PST",
	"2012-08-03 13:31:51.123 +02:00 CEST", "2012-08-03 8:1:59.257000000 +00:00 UTC",
	"2012-8-03 18:31:59.257000000 +00:00 UTC", "2012-8-3 18:31:59.257000000 +00:00 UTC",
	"2014-04-26 17:24:37.123456 +00:00 UTC", "2014-04-26 17:24:37.12 +00:00 UTC",
	"2014-04-26 17:24:37.1 +00:00 UTC", "2009-08-12T22:15:09", "2009-08-08T02:08:08", "2009-08-08T2:8:8",
	"2009-08-12T22:15:09.123", "2009-08-12T22:15:09.123456", "2009-08-12T22:15:09.12",
	"2009-08-12T22:15:09.1", "2014-04-26 17:24:37.3186369", "2009-08-12T22:15:09-07:00",
	"2009-08-12T22:15:09-03:00", "2009-08-12T22:15:9-07:00", "2009-08-12T22:15:09.123-07:00",
	"2016-06-21T19:55:00+01:00", "2016-06-21T19:55:00.799+01:00", "2019-05-29T08:41-04",
	"2009-08-12T22:15:09-0700", "2009-08-12T22:15:09-0300", "2009-08-12T22:15:9-0700",
	"2009-08-12T22:15:09.123-0700", "2016-06-21T19:55:00+0100", "2016-06-21T19:55:00.799+0100",
	"2016-06-21T19:55:00-0700", "2016-06-21T19:55+0100", "2016-06-21T19:55+0130",
	"2012-08-17T18:31:59:257+0100", "2009-08-12T22:15Z", "2009-08-12T22:15:09Z", "2009-08-12T22:15:09.99Z",
	"2009-08-12T22:15:09.9999Z", "2009-08-12T22:15:09.99999999Z", "2009-08-12T22:15:9.99999999Z",
	"2014.05", "2018.09.30", "3.31.2014", "3.3.2014", "03.31.2014", "08.21.71", "2014", "20140601",
	"20140722105203", "171113 14:14:20", "1332151919", "1384216367111", "1384216367111222",
	"1384216367111222333",
	// TestParse, TestParseErrors, TestParseLayout, TestParseStrict, TestInLocation,
	// TestPreferMonthFirst, TestRetryAmbiguousDateWithSwap, TestOne.
	"INVALID", "NOT GONNA HAPPEN", time.RFC3339, `{"ts":"now"}`, "138421636711122233311111", "-1314",
	"2014-13-13 08:20:13,787", "3", `{"hello"}`, "2009-15-12T22:15Z", "5,000-9,999", "xyzq-baad",
	"oct.-7-1970", "septe. 7, 1970", "SeptemberRR 7th, 1970", "29-06-2016", " 2018-01-02 17:08:09 -07:00",
	"2019-05-29T08:41-047", "06/May/2008 15:04:05 -0700", "14 May 2019 19:11:40.164", "May 05, 2015, 05:05:07",
	"2012-08-03 18:31:59 CEST", "03-03-14", "08.09.71", "2013-02-01 00:00:00", "18 January 2018",
	"2013-04-01 00:00:00", "Tue, 5 Jul 2017 16:28:13 -0700 (MST)", "Monday, 02-Jan-06 15:04:05 MST",
	"04/02/2014 04:08:09 +0000 UTC", "13/02/2014 04:08:09 +0000 UTC",
}

// extraInputs exercise branches and Unicode handling the test inputs miss,
// and the shapes REST clients send.
var extraInputs = []string{
	"", " ", "0", "00", "000", "0000", "00000", "000000", "1234567", "123456789", "12345678901",
	"123456789012", "123456789012345", "12345678901234567", "123456789012345678",
	"99999999999999999999", "9223372036854775807", "9223372036854775808",
	"2026-10-17T02:58:44Z", "2026-10-17T02:58:44.902Z", "2026-10-17T02:58:44.902094+00:00",
	"2026-10-17T02:58:44.902094534+08:00", "2026-10-17", "2026-10-17 02:58:44",
	"2026-10-17T02:58:44.123456789123Z", "2026-10-17T02:58:44+05:45", "2026-10-17T02:58:44-0930",
	"Sat, 17 Oct 2026 02:58:44 GMT", "Saturday, 17-Oct-26 02:58:44 GMT", "Sat Oct 17 02:58:44 2026",
	"Sat Oct 17 2026 10:58:44 GMT+0800 (China Standard Time)", "2026/10/17 02:58:44",
	"10/17/2026", "17/10/2026", "2026.10.17", "17.10.2026", "20261017", "20261017025844",
	"1792209524", "1792209524902", "1792209524902094", "1792209524902094534",
	"2026-10-17 02:58:44.902094534 +0000 UTC", "2026-10-17 02:58:44.902094534 +0000 UTC m=+39.779705272",
	"2026-01-02 03:04:05.678901 +0000 UTC", "0001-01-01 00:00:00 +0000 UTC",
	"2026-10-17 10:58:44.902094534 +0800 CST m=+39.779705272", "2026-10-17 10:58:44 +0800 +08",
	"2026-10-17 08:28:44.9 +0530 IST m=+0.5", "2026-10-17 08:43:44.9 +0545 +0545 m=+0.5",
	"2026-10-16 19:58:44.902094534 -0700 PDT m=+1.000000001", "2026-10-16 23:58:44 -0300 -03",
	"2026-10-17 13:58:44 +1100 +11 m=+3.5", "2026-10-17 16:43:44 +1345 +1345",
	"2026-10-17 02:58:44.9 -0000 UTC", "2026-10-17 02:58:44 +0000 XYZ", "2026-10-17 02:58:44 +0000 GMT+1",
	"2026-10-17 02:58:44 PDT", "2026-10-17 02:58:44 MDT", "2026-10-17 02:58:44 MST",
	"2026-10-17 02:58:44 CET", "2026-10-17 02:58:44 CEST", "2026-10-17 02:58:44 BST",
	"2026-10-17 02:58:44 IST", "2026-10-17 02:58:44 GMT+8", "2026-10-17 02:58:44 GMT-24",
	"2026-10-17 02:58:44 ChST", "2026-10-17 02:58:44 Z",
	"April 8, 2009", "APRIL 8, 2009", "april 8th 2009", "AprİL 8, 2009", "Frİday, 02 Jan 2006 15:04:05 MST",
	"Kon Jan  2 15:04:05 2006", "Monİ 02 Jan", "Ápr 8, 2009", "Sépt. 7, 1970", "ABé. 7, 1970",
	"Δεκ 7, 1970", "日 7, 1970", "7 Δεκεμβρίου 1970", "٢٠١٤-04-02", "２０１４-04-02", "2014年4月8日",
	"2014年04月08日 星期二", "Monday, 2 January 2006", "Mon, 02 Jan 2006", "Mon,02 Jan 2006",
	"mon, 02 jan 2006 15:04:05 mst", "Tue 05 May 2020, 05:05:05", "May 05 2005, 05:05:05",
	"May  8 17:57:51 2009", "May 08 17:57:51 2009", "May 8 2009 5:57:51 PM", "oct 1, 1970",
	"oct 7, '70 12:00", "sept 7, 1970", "sept. 28, 2017", "jan. 28, 2017", "January 02, 2006, 15:04:05",
	"January 02 2006, 15:04:05", "January 2nd, 2006, 15:04:05", "January 2nd 2006, 15:04:05",
	"January 02, 2006 at 3:04pm MST-07", "12 Feb 2006, 19:17", "12 Feb 2006, 19:17:22", "1 jan 18",
	"8 jan 2018", "02 Jan 2018 23:59", "02 Jan 2018 23:59:34", "2006-01-02T15:04:05Z07:00",
	"2020-08-17T17:00:00:000+0100", "2013-04-01 22:43:22", "2006-01", "2006-1-02", "2006-1-2",
	"2006-01-2", "13-Feb-03", "2014/07/10 06:55:38.156283", "2014:07:10 06:55:38.156283",
	"03:19:2012 10:11:59", "3:1:2012 10:11:59", "4:8:2014 22:05", "3:1:2014", "10:13:2014",
	"1:2:06", "1/2/06", "3/1/2012 10:11:59", "4/8/2014 22:05", "31/03/2005", "08/21/71 12:00",
	"2014-05-11 08:20:13,787,1", "22:43:22", "22:43", "05:24:37 PM", "2014-04-02 22:18+0530",
	"2014-04-02 22:18:00 +0000 UTC m=+0.000000001", "2014-04-02 22:18:00.001 +0000 UTC m=+0.000000001",
	"2014-04-02 22:18:00 -0700 -07", "2014-04-02 15:04:05 -07:00", "2014-04-02 17:57:51 -0700 2009",
	"2014-04-02 00:12:00 +00:00 UTC", "2014-04-02 00:12:00 2008", "2014-04-02 15:04:05.99Z",
	"2014-04-02 03:21:51+00:00", "2014-04-02 19:55:00+0100", "2014-04-02 19:55:00+01",
	"2014-04-02 19:55:00+1", "2014-04-02 19:55:00+", "2014-04-02 19:55:00+010", "2014-04-02 19:55:00+01000",
	"2014-04-02 19:55:00.799+0100", "2014-04-02 15:04:05.999-07:00", "2014-04-02 13:31:51.999-07:00 MST",
	"2014-04-02 13:31:51.999 -07:00 MST", "2014-04-02 13:31:51.999 -07:00 CEST",
	"2014-04-02 00:07:31.945167 +0000", "2014-04-02 06:20:00.000 UTC", "2014-04-02 06:20:00 UTC-05",
	"2014-04-02 15:44:11 UTC+0100 2015", "2014-04-02 18:04:07 GMT+0100 (GMT Daylight Time)",
	"2014-04-02 17:57:51 MST 2009", "2014-04-02 06:20:00 UTCX", "2014-04-02 06:20:00 ABCD",
	"2014-04-02 06:20:00 AB", "2014-04-02 06:20:00 A", "2014-04-02 06:20:00 P", "2014-04-02 06:20:00 PMX",
	"2014-04-02 06:20:00 am", "2014-04-02 06:20:00am", "2014-04-02 06:20:00pm", "2014-04-02 6pm",
	"2014-04-02 06:20:00 AMT", "2014-04-02 06:20:00 PDT", "2014-04-02T06:20:00Z07:00",
	"2014-04-02T06:20:00ZZ", "2014-04-02T06:20:00Z+0100", "2014-04-02T06:20:00.000Z00",
	"2014-04-02T06", "2014-04-02T", "2014-04-02 ", "2014-04-02  06:20", "2014-04-02\t06:20",
	"2014-04-02T06:20:00.", "2014-04-02T06:20:00.Z", "2014-04-02T06:20:00,5Z", "2014-04-02T06:20:00:5Z",
	"2014-04-02T06:20:00:12:34", "2014-04-02 06:20:00 at", "2014-04-02 06:20:00 at 5", "May 8 at 5pm",
	"2014-02-29", "2016-02-29", "2014-02-30", "2014-04-31", "2014-00-10", "2014-10-00", "2014-10-32",
	"0000-01-01", "9999-12-31", "10000-01-01", "−2014-01-01", "2014−01−01", "2014−01-01",
	"+2014-01-01", "-2014-01-01", "2014-1", "2014-", "2014--01", "2014-01-", "2014-Feb", "2014-Feb-",
	"2014-Feb-3", "2014-February-03", "2014-feb-03", "14-Feb-03 12:00", "14-Feb-2003 12:00",
	"14-Feb-200", "14-Feb-", "14-", "14-1", "1-Feb-03", "Feb-03-2014", "Feb/03/2014",
	"03/Feb/2014", "03/Feb/14", "03/Feb/2014 10:00", "03/Feb", "2014/Feb/03", "3/Feb/2014",
	"03/February/2014", "1/2/3", "1/2", "1/", "/1/2", "12/12/12/12", "1.2.3", "1.2", "1.",
	"12.12.12.12", "2014.1.2", "2014.12.31 12:00", "1:2", "1:", "12:12:12:12", "2014:1:2",
	"2014:01:02:03", "14 May 2019", "14 May", "14 Maybe 2019", "14  May 2019", "141414 12:00",
	"141414", "1414141", "171113 14:14:20 extra", "2014年", "2014年04月", "年", "2014年04月08日 19:17",
	"May", "May 8", "May 8,", "May 8, ", "May 8, 2009,", "May 8, 2009, ", "May 8 2009", "May 8th",
	"May 8st 2009", "May 8xx 2009", "May 8thx 2009", "May 8t", "May 8n", "Mayday 8 2009",
	"Septembe 7, 1970", "Sept 7, 1970", "Sep. 7, 1970", "Sept., 7, 1970", "Sept. , 1970",
	"S. 7, 1970", "Se. 7, 1970", "Sep.7, 1970", "Monday", "Mon", "Mon ", "Mon,", "Mon, ",
	"Mon, 1", "Mon, 1 Jan", "Monday,", "Monday, 1", "Monday, 1 Jan 2006", "Wed, 31 Dec 1969 23:59:59 GMT",
	"Thu, 01 Jan 1970 00:00:00 +0000", "Thu, 01 Jan 1970 00:00:00 +0000 (UTC)",
	"Thu, 01 Jan 1970 00:00:00", "Thu, 01-Jan-1970 00:00:00 GMT", "Thursday, 01 January 1970",
	"Fri Jul 03 2015", "Fri Jul 03 2015 18:04:07", "Fri Jul 03 18:04:07 2015", "Fri Jul 03 18:04:07 PDT 2015",
	"Fri Jul 3 18:04:07 GMT+0100 2015", "Jul 03 2015 18:04:07 GMT+0100", "Jul 03 18:04", "Jul 3",
	"a", "ab", "abc", "abcd", "abcde", "abc def", "abc, def", "abc. def", "a.b", "AB.", "ABC.", "ABCD.",
	"ABCDE.", "!", "@2014", "#", "2014!", "2014-04-02!", "2014-04-02 06:20:00!", "\x00", "2014\x00",
	"2014-04-02\x0006:20", "2014-04-02 06:20:00 é", "é", "é2014", "2é", "2014-é",
	"2014-04-é", "2014-04-02 é", "14 é 2019", "14 ééé 2019",
	"0/é/2014", "03/ét/2014", "3/Ã/14", "30/Ãbc/2014", "1/\xc3\xa9b/2014",
	"日本 2014", "中文", "2014年04月08日　", "😀", "2014-04-02 😀", "May 😀, 2009",
	"Septembre 7, 1970", "Mars 7, 1970", "March 7 1970 12:00", "march 7th, 1970 12:00 PM",
	"MARCH 7TH, 1970", "March 07 1970", "March 7, 70", "March 7, '70", "March 7, '70 5pm",
	"December 31, 9999 23:59:59", "January 1, 0001", "January 1, 0000", "January 0, 2000",
	"February 30, 2000", "June 31st 2012", "June 31 2012",
}

func dateparseGolden() DateparseGolden {
	g := DateparseGolden{GoVersion: runtime.Version()}
	var corpus []string
	corpus = append(corpus, dateparseInputs...)
	corpus = append(corpus, extraInputs...)

	// Cache time values written by Go nodes in various local zones
	// (cache.Time stores t.String(); a time.Now value carries a monotonic
	// reading, which String appends as " m=±<seconds>"). A fixed instant
	// stands in for time.Now so that the golden is reproducible; Go's API
	// cannot set a monotonic reading, so the two suffixes are appended in the
	// form Time.String writes them.
	for _, l := range []string{"UTC", "America/Los_Angeles", "Asia/Shanghai", "Asia/Kolkata",
		"Asia/Kathmandu", "Asia/Manila", "Asia/Singapore", "America/Sao_Paulo",
		"Australia/Lord_Howe", "Pacific/Chatham", "Europe/Dublin", "Europe/Moscow"} {
		setLocal(l)
		now := time.Date(2026, 10, 17, 8, 1, 18, 567105868, time.UTC).In(time.Local)
		corpus = append(corpus,
			now.String()+" m=+0.054329743",
			now.Add(-90*24*time.Hour).String()+" m=-7775999.945670257",
			time.Date(2026, 1, 2, 3, 4, 5, 678901000, time.Local).String(),
			time.Date(2026, 7, 2, 3, 4, 5, 0, time.Local).String(),
			time.Date(1996, 4, 8, 0, 0, 0, 0, time.UTC).In(time.Local).String(),
			now.Format(time.RFC3339),
			now.Format(time.RFC3339Nano),
			now.Format(time.RFC1123),
			now.Format(time.UnixDate),
		)
	}

	// Seeded corpus: mutations of the inputs above and token soup.
	rng := rand.New(rand.NewSource(20260)) // fixed seed: the corpus is part of the golden
	base := append([]string(nil), corpus...)
	for i := 0; i < 1600; i++ {
		s := base[rng.Intn(len(base))]
		for n := rng.Intn(3) + 1; n > 0; n-- {
			s = mutate(rng, s)
		}
		corpus = append(corpus, s)
	}
	tokens := []string{
		"2014", "14", "04", "4", "02", "2", "31", "12", "00", "1332151919", "-", "/", ":", ".", ",", " ",
		"  ", "T", "Z", "+", "+0800", "-07:00", "+01", "UTC", "GMT", "PST", "MST", "CEST", "AM", "PM",
		"am", "pm", "Jan", "Feb", "May", "September", "oct.", "Mon", "Monday", "th", "nd", "st", "rd",
		"at", "年", "月", "日", "m=+0.5", "(", ")", "'", "15:04:05", "08:20:13,787", ".123456",
	}
	for i := 0; i < 900; i++ {
		var b strings.Builder
		for n := rng.Intn(9) + 1; n > 0; n-- {
			b.WriteString(tokens[rng.Intn(len(tokens))])
		}
		corpus = append(corpus, b.String())
	}

	seen := map[string]bool{}
	var uniq []string
	for _, s := range corpus {
		if !seen[s] {
			seen[s] = true
			uniq = append(uniq, s)
		}
	}

	for _, s := range uniq {
		g.Cases = append(g.Cases, runDateparse("UTC", "ParseAny", s, "", nil))
	}
	for _, s := range uniq {
		g.Cases = append(g.Cases, runDateparse("America/Denver", "ParseAny", s, "", nil))
	}
	for _, local := range []string{"Europe/Berlin", "Asia/Shanghai"} {
		for _, s := range dateparseInputs {
			g.Cases = append(g.Cases, runDateparse(local, "ParseAny", s, "", nil))
		}
		for _, s := range extraInputs {
			g.Cases = append(g.Cases, runDateparse(local, "ParseAny", s, "", nil))
		}
	}
	optionSets := [][]string{
		{"PreferMonthFirst(false)"},
		{"RetryAmbiguousDateWithSwap(true)"},
		{"PreferMonthFirst(false)", "RetryAmbiguousDateWithSwap(true)"},
	}
	for _, opts := range optionSets {
		for _, s := range dateparseInputs {
			g.Cases = append(g.Cases, runDateparse("UTC", "ParseAny", s, "", opts))
		}
		for _, s := range extraInputs {
			g.Cases = append(g.Cases, runDateparse("America/Denver", "ParseAny", s, "", opts))
		}
	}
	for _, loc := range []string{"America/Denver", "America/Los_Angeles", "Europe/Berlin", "UTC", "Fixed:XYZ:3600"} {
		for _, s := range dateparseInputs {
			g.Cases = append(g.Cases, runDateparse("UTC", "ParseIn", s, loc, nil))
		}
	}
	for _, s := range extraInputs {
		g.Cases = append(g.Cases, runDateparse("Asia/Shanghai", "ParseIn", s, "America/Denver", nil))
	}
	for _, local := range []string{"UTC", "America/Denver"} {
		for _, s := range append(append([]string(nil), dateparseInputs...), extraInputs...) {
			g.Cases = append(g.Cases, runDateparse(local, "ParseLocal", s, "", nil))
			g.Cases = append(g.Cases, runDateparse(local, "ParseStrict", s, "", nil))
		}
	}
	for _, s := range uniq {
		g.Cases = append(g.Cases, runDateparse("UTC", "ParseFormat", s, "", nil))
	}
	return g
}
