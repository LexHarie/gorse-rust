// Command storagegolden records what Gorse's storage root package (storage/
// errors.go, options.go, scheme.go at the pinned commit) and the library code
// it relies on do, as golden data for the Rust port in
// crates/gorse-storage-core (WP-1-14):
//
//   - NewOptions with each option, and the TablePrefix table names;
//   - gorm's naming strategy as NewGORMConfig configures it (table, column,
//     join table, index, unique and check names) over Gorse's model and field
//     names and a seeded corpus;
//   - json_contains, run through modernc's SQLite exactly as Gorse registers
//     it (each case in a child process, because a panic inside the function
//     leaves the connection locked);
//   - AppendURLParams, and go-sql-driver/mysql's ParseDSN, FormatDSN and
//     AppendMySQLParams.
//
// It also writes, for gorse-common's gojson (also WP-1-14), encoding/json
// decoding into []any, as json_contains decodes its arguments, into any, as
// Gorse decodes labels, and into map[string]any, as viper's JSON codec
// decodes config files.
//
// Usage:
//
//	go run ./contracts/tools/storagegolden \
//		-out crates/gorse-storage-core/tests/data/storage_golden.json \
//		-gojson-out crates/gorse-common/tests/data/gojson_golden.json
//
// Zones for MySQL `loc=` come from the stand-in GOROOT zip that
// contracts/tools/timegolden writes for gorse-common's tests (ZONEINFO is set
// to it before anything loads a zone), and the corpus only names zones it
// holds, so the Rust tests can load the same data without system tzdata.
package main

import (
	"bytes"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"math"
	"math/big"
	"math/rand/v2"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime"
	"sort"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/go-sql-driver/mysql"
	"github.com/gorse-io/gorse/storage"
	"github.com/samber/lo"
	"golang.org/x/text/cases"
	"golang.org/x/text/language"
	"gorm.io/gorm/schema"
	_ "modernc.org/sqlite"
)

// bs is a Go string that may not be UTF-8: it is written as a JSON string when
// it is valid UTF-8, else as {"hex": "..."}.
type bs string

func (b bs) MarshalJSON() ([]byte, error) {
	if utf8.ValidString(string(b)) {
		return marshal(string(b))
	}
	return marshal(map[string]string{"hex": hex.EncodeToString([]byte(b))})
}

// marshal is json.Marshal without HTML escaping, so the file reads plainly.
func marshal(v any) ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return nil, err
	}
	return bytes.TrimSuffix(buf.Bytes(), []byte("\n")), nil
}

func main() {
	out := flag.String("out", "", "write the storage golden JSON here")
	jsonOut := flag.String("gojson-out", "", "write gorse-common's gojson golden JSON here")
	child := flag.Bool("json-contains-child", false, "internal: run one json_contains case from stdin")
	flag.Parse()
	if *child {
		runJSONContainsChild()
		return
	}
	if *out == "" || *jsonOut == "" {
		log.Fatal("-out and -gojson-out are required")
	}

	// Zones come from gorse-common's stand-in GOROOT zip (see the package doc).
	_, self, _, _ := runtime.Caller(0)
	root := filepath.Join(filepath.Dir(self), "..", "..", "..")
	zip := filepath.Join(root, "crates", "gorse-common", "tests", "data", "goroot", "lib", "time", "zoneinfo.zip")
	if _, err := os.Stat(zip); err != nil {
		log.Fatal(err)
	}
	if err := os.Setenv("ZONEINFO", zip); err != nil {
		log.Fatal(err)
	}

	g := map[string]any{
		"go_version":        runtime.Version(),
		"errors":            errorsGolden(),
		"options":           optionsGolden(),
		"table_prefix":      tablePrefixGolden(),
		"naming":            namingGolden(),
		"json_contains":     jsonContainsGolden(),
		"append_url_params": appendURLParamsGolden(),
		"mysql_dsn":         mysqlDSNGolden(),
	}
	writeGolden(*out, g)
	writeGolden(*jsonOut, map[string]any{"go_version": runtime.Version(), "json_decode": jsonDecodeGolden()})
}

func writeGolden(path string, g map[string]any) {
	data, err := marshal(g)
	if err != nil {
		log.Fatal(err)
	}
	var pretty bytes.Buffer
	if err := json.Indent(&pretty, data, "", " "); err != nil {
		log.Fatal(err)
	}
	pretty.WriteByte('\n')
	if err := os.WriteFile(path, pretty.Bytes(), 0o644); err != nil {
		log.Fatal(err)
	}
}

// ---------------------------------------------------------------- errors.go

func errorsGolden() map[string]string {
	return map[string]string{
		"ErrNotFound":        storage.ErrNotFound.Error(),
		"ErrNoDatabase":      storage.ErrNoDatabase.Error(),
		"ErrInvalidArgument": storage.ErrInvalidArgument.Error(),
		"ErrNotSupported":    storage.ErrNotSupported.Error(),
		"ErrAlreadyExists":   storage.ErrAlreadyExists.Error(),
	}
}

// --------------------------------------------------------------- options.go

type optionsOut struct {
	IsolationLevel        string `json:"isolation_level"`
	MaxOpenConns          int    `json:"max_open_conns"`
	MaxIdleConns          int    `json:"max_idle_conns"`
	ConnMaxLifetime       int64  `json:"conn_max_lifetime"`
	MaxSearchResults      int    `json:"max_search_results"`
	RedisClientName       string `json:"redis_client_name"`
	NumJobs               int    `json:"num_jobs"`
	SkipUnindexedSegments bool   `json:"skip_unindexed_segments"`
}

func optionsGolden() []map[string]any {
	cases := []struct {
		name string
		opts []storage.Option
	}{
		{"none", nil},
		{"all", []storage.Option{
			storage.WithNumJobs(4),
			storage.WithSkipUnindexedSegments(true),
			storage.WithIsolationLevel("SERIALIZABLE"),
			storage.WithMaxOpenConns(10),
			storage.WithMaxIdleConns(5),
			storage.WithConnMaxLifetime(90 * time.Second),
			storage.WithMaxSearchResults(7),
			storage.WithRedisClientName("gorse"),
		}},
		{"last wins", []storage.Option{storage.WithNumJobs(1), storage.WithNumJobs(2), storage.WithIsolationLevel("")}},
		{"negative", []storage.Option{storage.WithMaxOpenConns(-1), storage.WithMaxIdleConns(-2), storage.WithConnMaxLifetime(-time.Second), storage.WithMaxSearchResults(-3)}},
	}
	var out []map[string]any
	for _, c := range cases {
		o := storage.NewOptions(c.opts...)
		out = append(out, map[string]any{"case": c.name, "options": optionsOut{
			o.IsolationLevel, o.MaxOpenConns, o.MaxIdleConns, int64(o.ConnMaxLifetime),
			o.MaxSearchResults, o.RedisClientName, o.NumJobs, o.SkipUnindexedSegments,
		}})
	}
	return out
}

// ---------------------------------------------------------------- scheme.go

var tablePrefixes = []string{"", "gorse_", "public.", "a.b_", "x", "日本_", "\xff_"}

func tablePrefixGolden() []map[string]any {
	var out []map[string]any
	for _, p := range tablePrefixes {
		tp := storage.TablePrefix(p)
		out = append(out, map[string]any{
			"prefix":    bs(p),
			"values":    bs(tp.ValuesTable()),
			"sets":      bs(tp.SetsTable()),
			"documents": bs(tp.DocumentTable()),
			"points":    bs(tp.PointsTable()),
			"users":     bs(tp.UsersTable()),
			"items":     bs(tp.ItemsTable()),
			"feedback":  bs(tp.FeedbackTable()),
			"key":       bs(tp.Key("key")),
			"key_empty": bs(tp.Key("")),
		})
	}
	return out
}

// gorseNames are the model and field names Gorse hands gorm (storage/cache,
// storage/data, storage/meta, storage/vectors), gorm's own test inputs and
// edge cases.
var gorseNames = []string{
	// models
	"SQLValue", "SQLSet", "SQLUser", "SQLItem", "SQLFeedback", "SQLDocument", "PostgresDocument",
	"TimeSeriesPoint", "Values", "Sets", "Users", "Items", "Feedback", "Documents", "time_series_points",
	"Item", "User", "UserFeedback", "FeedbackKey", "Score", "Model", "SQLValues", "SQLSetX",
	"MySQLItem", "SQLUserSQLItem", "TimeSeriesPoints", "PostgresDocuments",
	// fields
	"ItemId", "IsHidden", "Categories", "Timestamp", "UpdateAt", "Labels", "Comment", "UserId",
	"FeedbackType", "Value", "Updated", "Name", "Member", "Collection", "Subset", "Id", "ID",
	"Score", "Embedding", "Key", "Ttl", "TTL", "ExpireAt", "Subscribe", "Neighbors", "Digest",
	// gorm naming_test.go inputs
	"", "x", "X", "userRestrictions", "ThisIsATest", "PFAndESI", "AbcAndJkl", "EmployeeID", "SKU_ID",
	"FieldX", "HTTPAndSMTP", "HTTPServerHandlerForURLID", "UUID", "HTTPURL", "HTTP_URL", "SHA256Hash",
	"SHA256HASH", "ThisIsActuallyATestSoWeMayBeAbleToUseThisCodeInGormPackageAlsoIdCanBeUsedAtTheEndAsID",
	"user_languages", "UserLanguage", "Company", "NameCID", "name",
	"thisIsAVeryVeryVeryVeryVeryVeryVeryVeryVeryLongString",
	// edge cases
	"A", "a", "_", "__", "_A", "A_", "a_B", "A_B", "ABC_DEF", "abcDEF", "IDs", "UserID", "HTTPSServer",
	"HTTPS", "UIDx", "UI", "UID", "APIURL", "ApiUrl", "JSONAPI", "XMLHTTPRequest", "X9", "9X", "A9B",
	"A1", "1A", "AB1C", "Ä", "aÄb", "ÄB", "BÄ", "ÉcoleID", "日本語", "日本ID", "IDÄ", "ÄID", "\xff",
	"A\xffB", "\xc3\x28", "a\xc3", "A\xc3", "AÄ", "aÄ", "Z", "ZZ", "Z[", "@A", "A@", "`a", "a{",
	"SQLItemSQLItem", "SQLDocumentID", "IDSQLUser", "PostgresDocumentSQLSet", "Time_Series_Point",
	"Hello World", "a.b", "A.B", "IP", "IPv6", "TLSConfig", "UTF8String", "XSRFToken", "QPSLimit",
	"VMName", "CPUs", "RAMSize", "LHSAndRHS", "EOFError", "DNSName", "SSHKey", "SMTPHost", "GUIDs",
	"URIs", "ASCIIText", "CSSClass", "HTMLBody", "RPCCall", "SLAs", "TTLs",
}

func randomNames(r *rand.Rand, n int) []string {
	alphabet := []string{"A", "B", "I", "D", "U", "L", "S", "Q", "a", "b", "d", "i", "s", "q", "_", "0", "9",
		".", " ", "Ä", "é", "\xff", "ID", "SQL", "HTTP", "URL", "Item", "User", "Value"}
	out := make([]string, 0, n)
	for i := 0; i < n; i++ {
		var b strings.Builder
		for j, k := 0, 1+r.IntN(8); j < k; j++ {
			b.WriteString(alphabet[r.IntN(len(alphabet))])
		}
		out = append(out, b.String())
	}
	return out
}

func namingGolden() map[string]any {
	// The initialism replacer pairs, as gorm's init builds them.
	commonInitialisms := []string{"API", "ASCII", "CPU", "CSS", "DNS", "EOF", "GUID", "HTML", "HTTP", "HTTPS", "ID", "IP", "JSON", "LHS", "QPS", "RAM", "RHS", "RPC", "SLA", "SMTP", "SSH", "TLS", "TTL", "UID", "UI", "UUID", "URI", "URL", "UTF8", "VM", "XML", "XSRF", "XSS"}
	var pairs [][2]string
	for _, initialism := range commonInitialisms {
		pairs = append(pairs, [2]string{initialism, cases.Title(language.Und).String(initialism)})
	}

	r := rand.New(rand.NewPCG(14, 1))
	names := append(append([]string{}, gorseNames...), randomNames(r, 400)...)

	var strategies []map[string]any
	for _, prefix := range []string{"", "gorse_", "public.", "a_very_long_table_prefix_so_that_index_names_pass_the_64_limit_"} {
		ns := storage.NewGORMConfig(prefix).NamingStrategy.(schema.NamingStrategy)
		tables := []string{ns.TableName("SQLDocument"), prefix + "items"}
		var rows []map[string]any
		for _, n := range names {
			row := map[string]any{
				"name":       bs(n),
				"table":      bs(ns.TableName(n)),
				"column":     bs(ns.ColumnName("", n)),
				"join_table": bs(ns.JoinTableName(n)),
			}
			var idx, uni, chk []bs
			for _, t := range tables {
				idx = append(idx, bs(ns.IndexName(t, n)))
				uni = append(uni, bs(ns.UniqueName(t, n)))
				chk = append(chk, bs(ns.CheckerName(t, n)))
			}
			row["index"], row["unique"], row["checker"] = idx, uni, chk
			rows = append(rows, row)
		}
		strategies = append(strategies, map[string]any{
			"table_prefix": prefix,
			"tables":       []bs{bs(tables[0]), bs(tables[1])},
			"names":        rows,
		})
	}

	// gorm's zero NamingStrategy (no replacer): toDBName alone.
	var plain []map[string]any
	ns := schema.NamingStrategy{}
	for _, n := range names {
		plain = append(plain, map[string]any{"name": bs(n), "column": bs(ns.ColumnName("", n))})
	}
	return map[string]any{
		"initialisms": pairs,
		"gorse":       strategies,
		"plain":       plain,
	}
}

// ------------------------------------------------------------ json decoding

// canon writes a decoded JSON value as a flat pre-order token list, so that
// every bit is visible and deep documents stay within the encoder's depth
// limit: ["null"], ["bool", b], ["num", float64 bits], ["str", s],
// ["arr", len] then the items, ["obj", len] then ["key", k] and the value for
// each key in sorted order.
func canon(v any, out [][]any) [][]any {
	switch x := v.(type) {
	case nil:
		return append(out, []any{"null"})
	case bool:
		return append(out, []any{"bool", x})
	case float64:
		return append(out, []any{"num", fmt.Sprintf("%016x", math.Float64bits(x))})
	case string:
		return append(out, []any{"str", bs(x)})
	case []any:
		out = append(out, []any{"arr", len(x)})
		for _, e := range x {
			out = canon(e, out)
		}
		return out
	case map[string]any:
		keys := lo.Keys(x)
		sort.Strings(keys)
		out = append(out, []any{"obj", len(x)})
		for _, k := range keys {
			out = append(out, []any{"key", bs(k)})
			out = canon(x[k], out)
		}
		return out
	default:
		panic(fmt.Sprintf("unexpected %T", v))
	}
}

var jsonEdgeCases = []string{
	``, ` `, "\n", "\t\r\n ", `[`, `]`, `[]`, `[ ]`, " [1] ", "\t[1]\n", `[1,2,3]`, `[1.0]`, `[1e0]`,
	`[-0]`, `[0]`, `[-0.0]`, `[1E2]`, `[1e-2]`, `[1.5e+10]`, `[123456789012345678901234567890]`,
	`[1e308]`, `[1.7976931348623157e308]`, `[1.7976931348623159e308]`, `[1e309]`, `[-1e400]`,
	`[1e-400]`, `[4.9e-324]`, `[2.4703282292062327e-324]`, `[2.4703282292062328e-324]`, `[0.1]`,
	`[9007199254740993]`, `[1E400]`, `[-0e0]`, `[0e-0]`, `[00]`, `[-01]`, `[1.0e]`, `[01]`, `[-]`,
	`[-a]`, `[1.]`, `[.1]`, `[1e]`, `[1e+]`, `[+1]`, `[NaN]`, `[Infinity]`, `[-Infinity]`, `[0x10]`,
	`["a"]`, `["\u00e9"]`, `["é"]`, `["\ud83d\ude00"]`, `["\ud83d"]`, `["\ude00"]`, `["\ud83dx"]`,
	`["\ud83d\u0041"]`, `["\ud83d\ud83d"]`, `["\u0000"]`, `["\/"]`, `["\b\f\n\r\t\"\\"]`, "[\"\xff\"]",
	"[\"\xc3\"]", "[\"\xed\xa0\x80\"]", "[\"\xf4\x90\x80\x80\"]", "[\"\xc0\xaf\"]", "[\"a\xffb\xfe\"]",
	`[true,false,null]`, `[{"a":1}]`, `[{"a":1,"a":2}]`, `[[1,[2]]]`, `null`, ` null `, `nul`, `nullx`,
	`{}`, `{"a":1}`, `"x"`, `1`, `-1`, `true`, `false`, `[1]x`, `[1]]`, `[1],`, `[1] [2]`, `[1,]`, `[,1]`,
	`[1 2]`, `[tru]`, `[truex]`, `[nul]`, `[nulll]`, `[fals]`, `[t]`, `[f]`, `[n]`, `["abc`, `["\`,
	`["\u12`, `["\u12zz"]`, `["\u12Z4"]`, `["\x"]`, `["\U0041"]`, `["\a"]`, "[\"a\tb\"]", "[\"a\x00b\"]",
	"[\"a\x1fb\"]", "[\"a\x7fb\"]", "[\"\u2028\u2029\"]", `[{"a" 1}]`, `[{"a":}]`, `[{1:2}]`, `[{"a":1,}]`,
	`[{"a":1 "b":2}]`, `[{,}]`, `[{"a"`, `[{"a":`, `[{"a":1`, `[{`, `[1`, `[1,`, `[{"a":1}`, "\xef\xbb\xbf[1]",
	"[\ufeff1]", "[1]\x00", "[1] \n\t\r", "\v[1]", "\f[1]", "\u00a0[1]", "[1]\u00a0", `[1e400, x]`,
	`[1e400, 1e400]`, `[{"a":1e400}]`, `[{"~/":1e400}]`, `[{"":1e400}]`, `[[1e400]]`, `[1, 2, 1e400]`,
	`[{"a":[1e400]}]`, `[{"0":1e400}]`, `[{"a":{"b":1e400}}]`, `[1e400, "\ud800"]`, `[{"\u0061":1}]`,
	`[{"a":1}, {"a":1e400}]`, `[1e400]x`, `[1e400`, `[1e400,]`, `["a",1e400,{"b":-1e400}]`,
	`[{"a":1,"b":{"c":[1,2,{"d":1e999}]}}]`, `[1, {"x": tru}]`, `[{"a\u0000b":1e400}]`, `[{"é":1e400}]`,
	"[{\"\xff\":1e400}]", `["\u00"]`, `["\u"]`, `["\uD834\uDD1E"]`, `["\uDD1E\uD834"]`, `["\uD800\uDC00\uD800"]`,
	`[true false]`, `[null,]`, `[[]`, `[[]]]`, `[[],[]]`, `[[], []]`, `[{}, {}]`, `[{"a":[]}]`, `[""]`, `["", ""]`,
	`[1,"1",true,null,[],{}]`, `[-0, 0, 0.0, -0.0, 0e5]`, `[5e-324]`, `[1e-324]`, `[3e-324]`, `[1.000000000000000000001]`,
	`[{"x.5":1e400}]`, `[{"a/b~c":1e400}]`, `[{"a.b":1e400}]`, `[{"5.a":1e400}]`, `[{"a":{"5":[1e400]}}]`, `{"a":1e400}`,
	`1e400`, "\"\xff\"", `[1e400]]`, `[{"a":1e400,"a":1}]`, `[{"a":[1],"a":{"b":2}}]`, `[{"b":1,"a":2,"b":3}]`,
	`[{"\u0000":1,"":2}]`, `[{"\ud800":1,"\ufffd":2}]`, "[{\"\xff\":1,\"\ufffd\":2}]", `[1,]`, `[{"a":1}}`, `[}`, `[{]`,
	`[{"a":1]`, `[[}]`, `{"a":[}`, `[:]`, `[,]`, `:`, `,`, `[1:2]`, `[{"a"::1}]`, `[{"a":1,,}]`, `["\uD83D\uDE00\uD83D"]`,
}

// longNumbers stresses float parsing past 800 significant digits, where
// strconv keeps a truncated decimal.
func longNumbers() []string {
	// 2^-1075 is halfway between 0 and the smallest subnormal float64.
	half := new(big.Float).SetPrec(4000).SetMantExp(new(big.Float).SetPrec(4000).SetInt64(1), -1075)
	exact := half.Text('e', -1)
	mant, exp, _ := strings.Cut(exact, "e")
	pad := strings.Repeat("0", 300)
	return []string{
		"[" + exact + "]",
		"[" + mant + pad + "e" + exp + "]",
		"[" + mant + pad + "1e" + exp + "]",
		"[" + strings.Replace(mant, "2.4703", "2.4702", 1) + "e" + exp + "]",
		"[0." + strings.Repeat("0", 1000) + "1]",
		"[9007199254740993" + strings.Repeat("0", 900) + "1e-901]",
		"[9007199254740993" + strings.Repeat("0", 900) + "e-900]",
		"[" + strings.Repeat("1", 1000) + "]",
		"[-" + strings.Repeat("9", 309) + "]",
		"[" + strings.Repeat("9", 308) + "]",
		"[1" + strings.Repeat("0", 2000) + "e-2000]",
	}
}

func randomJSONValue(r *rand.Rand, depth int, sb *strings.Builder) {
	ws := func() {
		for i, n := 0, r.IntN(3); i < n; i++ {
			sb.WriteByte(" \t\n\r"[r.IntN(4)])
		}
	}
	k := r.IntN(10)
	if depth >= 3 && k >= 8 {
		k = r.IntN(8)
	}
	switch k {
	case 0:
		sb.WriteString("null")
	case 1:
		sb.WriteString([]string{"true", "false"}[r.IntN(2)])
	case 2, 3:
		randomNumber(r, sb)
	case 4, 5, 6, 7:
		randomString(r, sb)
	case 8:
		sb.WriteByte('[')
		for i, n := 0, r.IntN(4); i < n; i++ {
			if i > 0 {
				sb.WriteByte(',')
			}
			ws()
			randomJSONValue(r, depth+1, sb)
			ws()
		}
		sb.WriteByte(']')
	case 9:
		sb.WriteByte('{')
		for i, n := 0, r.IntN(4); i < n; i++ {
			if i > 0 {
				sb.WriteByte(',')
			}
			ws()
			randomString(r, sb)
			ws()
			sb.WriteByte(':')
			ws()
			randomJSONValue(r, depth+1, sb)
		}
		sb.WriteByte('}')
	}
}

func randomNumber(r *rand.Rand, sb *strings.Builder) {
	if r.IntN(2) == 0 {
		sb.WriteByte('-')
	}
	if r.IntN(4) == 0 {
		sb.WriteByte('0')
	} else {
		sb.WriteByte(byte('1' + r.IntN(9)))
		for i, n := 0, r.IntN(20); i < n; i++ {
			sb.WriteByte(byte('0' + r.IntN(10)))
		}
	}
	if r.IntN(3) == 0 {
		sb.WriteByte('.')
		for i, n := 0, 1+r.IntN(20); i < n; i++ {
			sb.WriteByte(byte('0' + r.IntN(10)))
		}
	}
	if r.IntN(3) == 0 {
		sb.WriteByte("eE"[r.IntN(2)])
		if j := r.IntN(3); j < 2 {
			sb.WriteByte("+-"[j])
		}
		for i, n := 0, 1+r.IntN(3); i < n; i++ {
			sb.WriteByte(byte('0' + r.IntN(10)))
		}
	}
}

func randomString(r *rand.Rand, sb *strings.Builder) {
	pieces := []string{"a", "b", "1", " ", "é", "日", "\U0001F600", `\"`, `\\`, `\/`, `\b`, `\f`, `\n`, `\r`,
		`\t`, `\u0041`, `\u00e9`, `\ud83d\ude00`, `\ud83d`, `\ude00`, `\u0000`, "\xff", "\xc3", "\x7f", "\u2028"}
	sb.WriteByte('"')
	for i, n := 0, r.IntN(5); i < n; i++ {
		sb.WriteString(pieces[r.IntN(len(pieces))])
	}
	sb.WriteByte('"')
}

func jsonCorpus() []string {
	r := rand.New(rand.NewPCG(14, 2))
	corpus := append(append([]string{}, jsonEdgeCases...), longNumbers()...)
	// Deep nesting around encoding/json's limits.
	for _, depth := range []int{100, 9999, 10000, 10001, 10002} {
		corpus = append(corpus, strings.Repeat("[", depth)+strings.Repeat("]", depth))
		corpus = append(corpus, "["+strings.Repeat(`{"a":`, depth-1)+"1"+strings.Repeat("}", depth-1)+"]")
	}
	var valid []string
	for i := 0; i < 800; i++ {
		var sb strings.Builder
		if r.IntN(10) == 0 {
			randomJSONValue(r, 0, &sb)
		} else {
			sb.WriteByte('[')
			for j, n := 0, r.IntN(6); j < n; j++ {
				if j > 0 {
					sb.WriteByte(',')
				}
				randomJSONValue(r, 1, &sb)
			}
			sb.WriteByte(']')
		}
		valid = append(valid, sb.String())
	}
	corpus = append(corpus, valid...)
	// Mutations: delete, insert or replace one byte, or truncate.
	alphabet := []byte("[]{}\",:0123456789-+.eEtruefalsn\\u \t\n\x00\x7f\xff\xc3")
	for i := 0; i < 1200; i++ {
		s := []byte(valid[r.IntN(len(valid))])
		if len(s) == 0 {
			continue
		}
		p := r.IntN(len(s))
		switch r.IntN(4) {
		case 0:
			s = append(s[:p], s[p+1:]...)
		case 1:
			s = append(s[:p], append([]byte{alphabet[r.IntN(len(alphabet))]}, s[p:]...)...)
		case 2:
			s[p] = alphabet[r.IntN(len(alphabet))]
		case 3:
			s = s[:p]
		}
		corpus = append(corpus, string(s))
	}
	// Token soup.
	for i := 0; i < 300; i++ {
		var sb strings.Builder
		for j, n := 0, 1+r.IntN(12); j < n; j++ {
			sb.WriteByte(alphabet[r.IntN(len(alphabet))])
		}
		corpus = append(corpus, sb.String())
	}
	return corpus
}

// jsonObjectEdgeCases are inputs for the map[string]any target, which viper's
// JSON codec decodes config files into.
var jsonObjectEdgeCases = []string{
	`{}`, ` {} `, `{"a":1}`, `{"a":1,"a":2}`, `{"Port":1,"port":2}`, `{"b":1,"a":2,"b":3}`, `{"a":{"b":1},"a":{"c":2}}`,
	`{"master":{"port":1e400}}`, `{"master":{"port":8086,"host":"0.0.0.0"}}`, `{"a":[1,{"b":1e400}]}`, "\xef\xbb\xbf{}",
	`{}{`, `{} {}`, `{"a":1}x`, `[]`, `[{}]`, `"x"`, `1`, `-1e400`, `1e400`, `true`, `false`, `null`, ` null `, `nul`,
	`{"a":null}`, `{"":1}`, `{"\u0000":1}`, `{"a":{"a":{"a":1e400}}}`, `{"a.b":{"c":1e400}}`, `{"a":1e400,"b":1e400}`,
	`{"0":1e400}`, `{"a":{"0":1e400}}`, `{"a":[[1e400]]}`, `{`, `{"a"`, `{"a":`, `{"a":1`, `{"a":1,`, `{"a":1,}`, `{,}`,
	`{"a" 1}`, `{1:1}`, `{"a":1 "b":2}`, `{"a":1}}`, `{"a":[}`, "{\"\xff\":1}", `{"\ud800":1}`, `{"a":"\ud800"}`,
	"{\"a\":\"\xff\"}", `{"a":1e400,"a":1}`, `{"a":1,"a":1e400}`, `{"a":-0}`, `{"a":9007199254740993}`,
	`{"database":{"data_store":"mysql://u:p@tcp(h:3306)/gorse","cache_store":"redis://h:6379/0"},"master":{"port":8086}}`,
}

// jsonObjectCorpus is the object edge cases, seeded random objects and
// mutations of them.
func jsonObjectCorpus() []string {
	r := rand.New(rand.NewPCG(14, 5))
	corpus := append([]string{}, jsonObjectEdgeCases...)
	var valid []string
	for i := 0; i < 300; i++ {
		var sb strings.Builder
		sb.WriteByte('{')
		for j, n := 0, r.IntN(6); j < n; j++ {
			if j > 0 {
				sb.WriteByte(',')
			}
			randomString(r, &sb)
			sb.WriteByte(':')
			randomJSONValue(r, 1, &sb)
		}
		sb.WriteByte('}')
		valid = append(valid, sb.String())
	}
	corpus = append(corpus, valid...)
	alphabet := []byte("[]{}\",:0123456789-+.eEtruefalsn\\u \t\n\x00\x7f\xff\xc3")
	for i := 0; i < 300; i++ {
		s := []byte(valid[r.IntN(len(valid))])
		p := r.IntN(len(s))
		switch r.IntN(4) {
		case 0:
			s = append(s[:p], s[p+1:]...)
		case 1:
			s = append(s[:p], append([]byte{alphabet[r.IntN(len(alphabet))]}, s[p:]...)...)
		case 2:
			s[p] = alphabet[r.IntN(len(alphabet))]
		case 3:
			s = s[:p]
		}
		corpus = append(corpus, string(s))
	}
	return corpus
}

func jsonDecodeGolden() []map[string]any {
	var out []map[string]any
	for _, in := range append(jsonCorpus(), jsonObjectCorpus()...) {
		// What the json_contains closure does for a string argument.
		var j []any
		err := json.Unmarshal([]byte(in), &j)
		c := map[string]any{"in": bs(in)}
		if err != nil {
			c["err"] = bs(err.Error())
		} else if j == nil {
			c["nil"] = true
		} else {
			c["value"] = canon(j, nil)
		}
		// json.Unmarshal into an any, as Gorse decodes labels.
		var a any
		if err := json.Unmarshal([]byte(in), &a); err != nil {
			c["any_err"] = bs(err.Error())
		} else {
			c["any_value"] = canon(a, nil)
		}
		// json.Unmarshal into a map[string]any, as viper's JSON codec
		// decodes a config file.
		var m map[string]any
		if err := json.Unmarshal([]byte(in), &m); err != nil {
			c["map_err"] = bs(err.Error())
		} else if m == nil {
			c["map_nil"] = true
		} else {
			c["map_value"] = canon(m, nil)
		}
		out = append(out, c)
	}
	return out
}

// ----------------------------------------------------------- json_contains

// jsonArg is one SQL argument: its SQLite type and value.
type jsonArg struct {
	Kind  string  `json:"kind"` // null, integer, real, text, blob
	Int   int64   `json:"int,omitempty"`
	Real  float64 `json:"real,omitempty"`
	Bytes []byte  `json:"bytes,omitempty"`
}

func (a jsonArg) value() any {
	switch a.Kind {
	case "null":
		return nil
	case "integer":
		return a.Int
	case "real":
		return a.Real
	case "text":
		return string(a.Bytes)
	case "blob":
		// Never nil: modernc binds a nil []byte as NULL, and an empty blob
		// is meant here.
		return append([]byte{}, a.Bytes...)
	}
	panic(a.Kind)
}

func (a jsonArg) golden() map[string]any {
	m := map[string]any{"kind": a.Kind}
	switch a.Kind {
	case "integer":
		m["int"] = a.Int
	case "real":
		m["bits"] = fmt.Sprintf("%016x", math.Float64bits(a.Real))
	case "text", "blob":
		m["bytes"] = bs(a.Bytes)
	}
	return m
}

// jsonContainsResult is what a child reports. The texts are bytes, so that
// invalid UTF-8 in them survives the trip.
type jsonContainsResult struct {
	Null  bool   `json:"null,omitempty"`
	Int   *int64 `json:"int,omitempty"`
	Err   []byte `json:"err,omitempty"`
	Panic []byte `json:"panic,omitempty"`
}

func runJSONContainsChild() {
	var args [2]jsonArg
	if err := json.NewDecoder(os.Stdin).Decode(&args); err != nil {
		log.Fatal(err)
	}
	db, err := sql.Open("sqlite", ":memory:")
	if err != nil {
		log.Fatal(err)
	}
	var res jsonContainsResult
	func() {
		defer func() {
			if p := recover(); p != nil {
				res.Panic = []byte(fmt.Sprint(p))
			}
		}()
		var v any
		if err := db.QueryRow("SELECT json_contains(?, ?)", args[0].value(), args[1].value()).Scan(&v); err != nil {
			res.Err = []byte(err.Error())
			return
		}
		switch x := v.(type) {
		case nil:
			res.Null = true
		case int64:
			res.Int = &x
		default:
			log.Fatalf("unexpected result %T", v)
		}
	}()
	if err := json.NewEncoder(os.Stdout).Encode(res); err != nil {
		log.Fatal(err)
	}
}

func text(s string) jsonArg { return jsonArg{Kind: "text", Bytes: []byte(s)} }
func blob(s string) jsonArg { return jsonArg{Kind: "blob", Bytes: []byte(s)} }

func jsonContainsCases() [][2]jsonArg {
	null := jsonArg{Kind: "null"}
	cases := [][2]jsonArg{
		{text(`[1,2]`), text(`[1]`)}, {text(`[1]`), text(`[1,2]`)}, {text(`[1]`), text(`[1]`)},
		{text(`[]`), text(`[]`)}, {text(`[1]`), text(`[]`)}, {text(`[]`), text(`[1]`)},
		{text(`["a","b"]`), text(`["b","a"]`)}, {text(`["a","b"]`), text(`["a","a"]`)},
		{text(`["a"]`), text(`["A"]`)}, {text(`[1]`), text(`[1.0]`)}, {text(`[1]`), text(`[1e0]`)},
		{text(`[100]`), text(`[1e2]`)}, {text(`[0]`), text(`[-0]`)}, {text(`[-0.0]`), text(`[0]`)},
		{text(`[1]`), text(`["1"]`)}, {text(`[true]`), text(`[1]`)}, {text(`[null]`), text(`[null]`)},
		{text(`[null]`), text(`[false]`)}, {text(`[true,false]`), text(`[false]`)},
		{text(`["é"]`), text(`["\u00e9"]`)}, {text(`["e\u0301"]`), text(`["é"]`)},
		{text("[\"\xff\"]"), text(`["\ufffd"]`)}, {text(`["\ud800"]`), text(`["\ufffd"]`)},
		{text(`[0.1]`), text(`[0.10000000000000001]`)}, {text(`[9007199254740993]`), text(`[9007199254740992]`)},
		{text(`[1e-400]`), text(`[0]`)}, {text(`[1e400]`), text(`[1]`)}, {text(`[1]`), text(`[1e400]`)},
		{text(`[1,[2]]`), text(`[1]`)}, {text(`[1]`), text(`[[1]]`)}, {text(`[1]`), text(`[2,[3]]`)},
		{text(`[1]`), text(`[1,[3]]`)}, {text(`[{"a":1}]`), text(`[]`)}, {text(`[1]`), text(`[{}]`)},
		{text(`[1]`), text(`[2,{}]`)}, {text(`[]`), text(`[[1]]`)}, {text(`[[]]`), text(`[]`)},
		{text(`[1]`), text(`{}`)}, {text(`{}`), text(`[1]`)}, {text(`"x"`), text(`[1]`)},
		{text(`null`), text(`[1]`)}, {text(`[1]`), text(`null`)}, {text(`null`), text(`null`)},
		{text(`x`), text(`[1]`)}, {text(`[1]`), text(`x`)}, {text(`x`), text(`y`)}, {text(``), text(`[1]`)},
		{text(`[1]`), text(``)}, {text(`[1]x`), text(`[1]`)}, {text(`[1e400, x]`), text(`[1]`)},
		{null, text(`[1]`)}, {text(`[1]`), null}, {null, null}, {null, text(`x`)}, {text(`x`), null},
		{jsonArg{Kind: "integer", Int: 1}, text(`[1]`)}, {text(`[1]`), jsonArg{Kind: "integer", Int: 1}},
		{jsonArg{Kind: "real", Real: 1.5}, text(`[1]`)}, {text(`[1]`), jsonArg{Kind: "real", Real: 1.5}},
		{jsonArg{Kind: "integer", Int: 1}, jsonArg{Kind: "real", Real: 1.5}}, {text(`x`), jsonArg{Kind: "integer", Int: 1}},
		{blob(`[1]`), text(`[1]`)}, {text(`[1]`), blob(`[1]`)}, {blob(`[1]`), blob(`[1]`)}, {blob(``), text(`[1]`)},
		{blob(`x`), text(`[1]`)}, {text("[1]\x00x"), text(`[1]`)}, {text("[1,2]\x00"), text("[2]\x00junk")},
		{blob("[1]\x00x"), text(`[1]`)}, {text("\x00"), text(`[1]`)}, {text(`[1]`), text("\x00[2]")},
		{text(`["a","b","c"]`), text(`["c","a"]`)}, {text(`["tag1","tag2"]`), text(`["tag2"]`)},
		{text(`["tag1","tag2"]`), text(`["tag3"]`)}, {text(`["tag1","tag2"]`), text(`["tag1","tag3"]`)},
		{text(`[]`), text(`["tag1"]`)}, {text(`["x"]`), text(`[]`)}, {text(` [ "a" ] `), text("\t[\"a\"]\n")},
		{text(`[{"a":1}]`), text(`[1]`)}, {text(`[[1]]`), text(`[]`)}, {text(`[1]`), text(`[2,[1]]`)},
		{text(`[1,1,1]`), text(`[1,1]`)}, {text(`[1e400]`), text(`[x]`)}, {text(`[x]`), text(`[1e400]`)},
		{text(`[{"a\u0000b":1e400}]`), text(`[1]`)}, {text(`[]`), text(`[{}]`)}, {text(`null`), text(`[[1]]`)},
		{text(`[1,{}]`), text(`[1]`)}, {text(`[[1],{}]`), text(`[]`)}, {text(`[{"x.5":1e400}]`), text(`[]`)},
		{text(`[1,2]`), text(`[2,1e400]`)}, {blob("[\"\xff\"]"), text(`["\ufffd"]`)}, {text(`[1]`), text(`[1,{}]`)},
		{text(`[1]`), text(`[2,{}]`)}, {blob(``), blob(``)}, {text(`[]`), text(`null`)}, {text(`null`), text(`[]`)},
		{text(`[-0]`), text(`[0, -0.0, 0e9]`)}, {text(`[true]`), text(`[true, true]`)}, {text(`[null, null]`), text(`[null]`)},
	}
	// Seeded pairs from a pool of scalars, so both answers come up.
	r := rand.New(rand.NewPCG(14, 3))
	pool := []string{`1`, `1.0`, `2`, `-0`, `0`, `1e2`, `100`, `"1"`, `"a"`, `"A"`, `"é"`, `"\u00e9"`,
		`true`, `false`, `null`, `"\ufffd"`, "\"\xff\"", `0.30000000000000004`, `[1]`, `{}`}
	arr := func(n int) string {
		var parts []string
		for i := 0; i < n; i++ {
			parts = append(parts, pool[r.IntN(len(pool))])
		}
		return "[" + strings.Join(parts, ",") + "]"
	}
	for i := 0; i < 160; i++ {
		a, b := text(arr(r.IntN(5))), text(arr(r.IntN(3)))
		if r.IntN(8) == 0 {
			a.Kind = "blob"
		}
		cases = append(cases, [2]jsonArg{a, b})
	}
	return cases
}

func jsonContainsGolden() []map[string]any {
	self, err := os.Executable()
	if err != nil {
		log.Fatal(err)
	}
	var out []map[string]any
	for _, c := range jsonContainsCases() {
		in, err := json.Marshal(c)
		if err != nil {
			log.Fatal(err)
		}
		cmd := exec.Command(self, "-json-contains-child")
		cmd.Stdin = bytes.NewReader(in)
		cmd.Stderr = os.Stderr
		stdout, err := cmd.Output()
		if err != nil {
			log.Fatalf("child for %s: %v", in, err)
		}
		var res jsonContainsResult
		if err := json.Unmarshal(stdout, &res); err != nil {
			log.Fatal(err)
		}
		g := map[string]any{"args": []any{c[0].golden(), c[1].golden()}}
		switch {
		case res.Panic != nil:
			g["panic"] = bs(res.Panic)
		case res.Err != nil:
			g["err"] = bs(res.Err)
		case res.Null:
			g["null"] = true
		default:
			g["int"] = *res.Int
		}
		out = append(out, g)
	}
	return out
}

// ----------------------------------------------------------- URL params

var rawURLs = []string{
	`c:\\sqlite.db`, `sqlite.db`, `/var/lib/gorse/cache.db`, `/var/lib/gorse/data.db`, `file:test.db?cache=shared&mode=memory`,
	`:memory:`, `file::memory:?cache=shared`, `test.db?_pragma=foo`, `test.db?a=1&a=2&b=`, `a b.db`, `%zz.db`,
	`C:/Users/x/gorse.db`, `C:\Users\x\gorse.db`, `relative/path/to.db`, `data.db#frag`, `data.db?x=1#frag`,
	`http://[::1]:80/x`, ``, `?`, `#`, `/`, `//host/path.db`, `///abs.db`, `file:///tmp/x.db`, `file:/tmp/x.db`,
	`mem.db?_pragma=busy_timeout(5000)`, `x.db?;=1`, `x.db?a;b=1`, `x.db?%zz=1`, `x.db?a=%zz`, `x.db?a=b&&c=d`,
	`日本.db`, "\xff.db", "x\x00.db", "x\x7f.db", `x y.db?a b=c d`, `1:x.db`, `a1+-.:x`, `-a:x`, `a:b:c`,
	`./x.db`, `../x.db`, `~/x.db`, `x.db?`, `x.db?&`, `x.db?=`, `x.db?a`, `x.db?a=b=c`, `x%20y.db`, `x%2Fy.db`,
	`http://host:port/x`, `http://[fe80::1%25en0]:8080/`, `http://user:pa%zzss@host/`, `scheme://user@host/x?q#f`,
	`gorse_test.db`, `/tmp/gorse/../gorse.db`, `C:`, `c:/`, `\\server\share\x.db`, `x.db?_pragma=journal_mode(wal)`,
}

var paramLists = [][]lo.Tuple2[string, string]{
	{{A: "_pragma", B: "busy_timeout(10000)"}, {A: "_pragma", B: "journal_mode(wal)"}},
	{{A: "a", B: "b"}},
	nil,
	{{A: "a b", B: "c&d=e"}, {A: "é", B: "日本"}},
	{{A: "", B: ""}, {A: "a", B: ""}},
	{{A: "b", B: "2"}, {A: "a", B: "1"}, {A: "b", B: "1"}},
}

func appendURLParamsGolden() []map[string]any {
	var out []map[string]any
	for _, u := range rawURLs {
		for pi, params := range paramLists {
			res, err := storage.AppendURLParams(u, params)
			c := map[string]any{"url": bs(u), "params": pi}
			if err != nil {
				c["err"] = bs(err.Error())
			} else {
				c["out"] = bs(res)
			}
			out = append(out, c)
		}
	}
	var lists []any
	for _, params := range paramLists {
		var l [][2]bs
		for _, p := range params {
			l = append(l, [2]bs{bs(p.A), bs(p.B)})
		}
		lists = append(lists, l)
	}
	return []map[string]any{{"param_lists": lists, "cases": out}}
}

// ----------------------------------------------------------- MySQL DSN

// dsn_test.go testDSNs and invalidDSNs, Gorse's DSNs, and edge cases.
var mysqlDSNs = []string{
	"username:password@protocol(address)/dbname?param=value",
	"username:password@protocol(address)/dbname?param=value&columnsWithAlias=true",
	"username:password@protocol(address)/dbname?param=value&columnsWithAlias=true&multiStatements=true",
	"user@unix(/path/to/socket)/dbname?charset=utf8",
	"user:password@tcp(localhost:5555)/dbname?charset=utf8&tls=true",
	"user:password@tcp(localhost:5555)/dbname?charset=utf8mb4,utf8&tls=skip-verify",
	"user:password@/dbname?loc=UTC&timeout=30s&readTimeout=1s&writeTimeout=1s&allowAllFiles=1&clientFoundRows=true&allowOldPasswords=TRUE&collation=utf8mb4_unicode_ci&maxAllowedPacket=16777216&tls=false&allowCleartextPasswords=true&parseTime=true&rejectReadOnly=true",
	"user:password@/dbname?allowNativePasswords=false&checkConnLiveness=false&maxAllowedPacket=0&allowFallbackToPlaintext=true",
	"user:p@ss(word)@tcp([de:ad:be:ef::ca:fe]:80)/dbname?loc=Local",
	"/dbname", "/dbname%2Fwithslash", "@/", "/", "", "user:p@/ssword@/", "unix/?arg=%2Fsome%2Fpath.ext",
	"tcp(127.0.0.1)/dbname", "tcp(de:ad:be:ef::ca:fe)/dbname",
	"user:password@/dbname?loc=UTC&timeout=30s&parseTime=true&timeTruncate=1h",
	"foo:bar@tcp(192.168.1.50:3307)/baz?timeout=10s&connectionAttributes=program_name:MySQLGoDriver%2FTest,program_version:1.2.3",
	"@net(addr/", "@tcp(/", "tcp(/", "(/", "net(addr)//", "User:pass@tcp(1.2.3.4:3306)", "net()/",
	"user:pass@tcp(127.0.0.1:3306)/db/name", "user:password@/dbname?allowFallbackToPlaintext=PREFERRED",
	"user:password@/dbname?connectionAttributes=attr1:/unescaped/value", "/dbname?arg=/some/unescaped/path",
	"tcp(example.com:1234)/?tls=true", "tcp(example.com)/?tls=true", "User:password@tcp(localhost:5555)/dbname?serverPubKey=testKey",
	"User:password@tcp(localhost:5555)/dbname?tls=invalid_tls", "/dbname?collation=gbk_chinese_ci&interpolateParams=true",
	"/dbname?collation=gbk_chinese_ci&interpolateParams=false", "/dbname?collation=gbk_chinese_ci",
	"/dbname?collation=ascii_bin&interpolateParams=true", "/dbname?collation=utf8mb4_general_ci&interpolateParams=true",
	// Gorse
	"gorse:gorse_pass@tcp(localhost:3306)/gorse", "root:password@tcp(127.0.0.1:3306)/", "root@tcp(db)/gorse",
	"x@y(z:123)/foo", "x@y(1.2.3.4:123)/foo", "localhost:3306/gorse", "a,b,c/bar", "root:password@tcp(mysql:3306)/gorse?parseTime=true",
	"root:password@tcp(mysql:3306)/gorse?sql_mode=ANSI", "root:password@tcp(mysql:3306)/gorse?transaction_isolation=%27READ-COMMITTED%27",
	"root:password@tcp(mysql:3306)/gorse?tx_isolation='SERIALIZABLE'&parseTime=false",
	// edge cases
	"u:p@tcp(h)/d?strict=true", "u:p@tcp(h)/d?strict", "/d?tls=SKIP-VERIFY", "/d?tls=Preferred", "/d?tls=PREFERRED",
	"/d?tls=skip-verİfy", "/d?tls=s\u212Aip-verify", "/d?tls=TRUE", "/d?tls=1", "/d?tls=0", "/d?tls=custom",
	"/d?tls=%zz", "/d?tls=a%20b", "tcp(h:1)/d?tls=true", "unix(/s)/d?tls=true", "tcp([::1]:3306)/d?tls=true",
	"tcp(::1)/d?tls=true", "tcp(h:)/d", "tcp(:1)/d", "tcp(h:1:2)/d", "tcp([h]:1)/d", "tcp([h])/d", "tcp(h]:1)/d",
	"/d?loc=Asia%2FShanghai", "/d?loc=Asia/Shanghai", "/d?loc=Local", "/d?loc=UTC", "/d?loc=", "/d?loc=Nowhere%2FZone",
	"/d?loc=%zz", "/d?loc=../etc", "/d?loc=Europe/Berlin&parseTime=1", "/d?readTimeout=x", "/d?readTimeout=-1s",
	"/d?readTimeout=0", "/d?timeout=1.5h", "/d?writeTimeout=1h2m3.5s", "/d?timeTruncate=x", "/d?timeTruncate=-1ms",
	"/d?timeTruncate=1us", "/d?maxAllowedPacket=x", "/d?maxAllowedPacket=-1", "/d?maxAllowedPacket=+5",
	"/d?maxAllowedPacket=99999999999999999999", "/d?maxAllowedPacket=67108864", "/d?maxAllowedPacket=", "/d?charset=",
	"/d?charset=a,,b", "/d?collation=", "/d?interpolateParams=1&collation=big5_bin", "/d?interpolateParams=yes",
	"/d?a=1&a=2", "/d?a", "/d?=v", "/d?a=%zz", "/d?a=%41%42", "/d?a=b+c", "/d?a=b%2Bc", "/d?&&a=1&&",
	"/d?serverPubKey=", "/d?serverPubKey=%zz", "/d?serverPubKey=k", "/d?connectionAttributes=%zz",
	"/d?connectionAttributes=a:b,c:d", "/d?compress=true", "/d?compress=x", "/d?compress=0", "/d?parseTime=True",
	"/d?parseTime=yes", "/d?columnsWithAlias=FALSE", "/d?rejectReadOnly=False", "/d?multiStatements=1",
	"/d?allowAllFiles=true&allowNativePasswords=false", "foo()/d", "foo/d", "unix/d", "unix()/d", "tcp/d", "tcp()/d",
	"tcp6(h)/d", "tcp6()/d", "u@/d", "u:@/d", ":p@/d", ":@/d", "@@/d", "u:p:q@/d", "u@h@/d", "u(x)@tcp(h)/d",
	"u:p@tcp(h)/d%zz", "u:p@tcp(h)/d%2f", "u:p@tcp(h)/d+e", "u:p@tcp(h)/d e", "u:p@tcp(h)/日本", "u:p@tcp(h)/\xff",
	"\xff:\xfe@tcp(\xfd)/d?\xfc=\xfb", "u:p@tcp(h)/d?", "u:p@tcp(h)/d??a=1", "u:p@tcp(h)/d?a=1?b=2", "u:p@tcp(h)//",
	"u:p@tcp(h)/d/", "u:p@tcp(h(i))/d", "u:p@tcp((h))/d", "u:p@tcp(h)x/d", "u:p@(h)/d", "u:p@tcp(h)/d#f",
	"user:pa/ss@tcp(h)/d", "user:pa@ss@tcp(h)/d", "user:pa:ss@tcp(h)/d", "user:pa)ss@tcp(h)/d", "(/d", ")/d",
	"tcp(h)", "tcp(h)d", "/?parseTime=true&loc=Asia%2FTokyo&timeout=5s",
}

type mysqlConfigOut struct {
	User                     bs             `json:"user"`
	Passwd                   bs             `json:"passwd"`
	Net                      bs             `json:"net"`
	Addr                     bs             `json:"addr"`
	DBName                   bs             `json:"db_name"`
	ParamKeys                []bs           `json:"param_keys"`
	ParamValues              []bs           `json:"param_values"`
	ConnectionAttributes     bs             `json:"connection_attributes"`
	Collation                bs             `json:"collation"`
	Loc                      bs             `json:"loc"`
	LocIs                    string         `json:"loc_is"`
	MaxAllowedPacket         int            `json:"max_allowed_packet"`
	ServerPubKey             bs             `json:"server_pub_key"`
	TLSConfig                bs             `json:"tls_config"`
	TLS                      map[string]any `json:"tls"`
	Timeout                  int64          `json:"timeout"`
	ReadTimeout              int64          `json:"read_timeout"`
	WriteTimeout             int64          `json:"write_timeout"`
	AllowAllFiles            bool           `json:"allow_all_files"`
	AllowCleartextPasswords  bool           `json:"allow_cleartext_passwords"`
	AllowFallbackToPlaintext bool           `json:"allow_fallback_to_plaintext"`
	AllowNativePasswords     bool           `json:"allow_native_passwords"`
	AllowOldPasswords        bool           `json:"allow_old_passwords"`
	CheckConnLiveness        bool           `json:"check_conn_liveness"`
	ClientFoundRows          bool           `json:"client_found_rows"`
	ColumnsWithAlias         bool           `json:"columns_with_alias"`
	InterpolateParams        bool           `json:"interpolate_params"`
	MultiStatements          bool           `json:"multi_statements"`
	ParseTime                bool           `json:"parse_time"`
	RejectReadOnly           bool           `json:"reject_read_only"`
	Compress                 bool           `json:"compress"`
	TimeTruncate             int64          `json:"time_truncate"`
	Charsets                 []bs           `json:"charsets"`
	HasPubKey                bool           `json:"has_pub_key"`
}

func dumpMySQLConfig(cfg *mysql.Config) mysqlConfigOut {
	o := mysqlConfigOut{
		User: bs(cfg.User), Passwd: bs(cfg.Passwd), Net: bs(cfg.Net), Addr: bs(cfg.Addr), DBName: bs(cfg.DBName),
		ConnectionAttributes: bs(cfg.ConnectionAttributes), Collation: bs(cfg.Collation),
		MaxAllowedPacket: cfg.MaxAllowedPacket, ServerPubKey: bs(cfg.ServerPubKey), TLSConfig: bs(cfg.TLSConfig),
		Timeout: int64(cfg.Timeout), ReadTimeout: int64(cfg.ReadTimeout), WriteTimeout: int64(cfg.WriteTimeout),
		AllowAllFiles: cfg.AllowAllFiles, AllowCleartextPasswords: cfg.AllowCleartextPasswords,
		AllowFallbackToPlaintext: cfg.AllowFallbackToPlaintext, AllowNativePasswords: cfg.AllowNativePasswords,
		AllowOldPasswords: cfg.AllowOldPasswords, CheckConnLiveness: cfg.CheckConnLiveness,
		ClientFoundRows: cfg.ClientFoundRows, ColumnsWithAlias: cfg.ColumnsWithAlias,
		InterpolateParams: cfg.InterpolateParams, MultiStatements: cfg.MultiStatements, ParseTime: cfg.ParseTime,
		RejectReadOnly: cfg.RejectReadOnly,
	}
	// Keys may not be UTF-8, so the map is written as sorted keys and their
	// values in the same order. A nil map has nil keys.
	if cfg.Params != nil {
		keys := lo.Keys(cfg.Params)
		sort.Strings(keys)
		o.ParamKeys, o.ParamValues = []bs{}, []bs{}
		for _, k := range keys {
			o.ParamKeys = append(o.ParamKeys, bs(k))
			o.ParamValues = append(o.ParamValues, bs(cfg.Params[k]))
		}
	}
	switch {
	case cfg.Loc == nil:
		o.LocIs = "nil"
	case cfg.Loc == time.UTC:
		o.LocIs = "UTC"
	case cfg.Loc == time.Local:
		o.LocIs = "Local"
	default:
		o.LocIs = "loaded"
	}
	if cfg.Loc != nil {
		o.Loc = bs(cfg.Loc.String())
	}
	if cfg.TLS != nil {
		o.TLS = map[string]any{"server_name": bs(cfg.TLS.ServerName), "insecure_skip_verify": cfg.TLS.InsecureSkipVerify}
	}
	v := reflect.ValueOf(cfg).Elem()
	o.Compress = v.FieldByName("compress").Bool()
	o.TimeTruncate = v.FieldByName("timeTruncate").Int()
	if cs := v.FieldByName("charsets"); !cs.IsNil() {
		o.Charsets = []bs{}
		for i := 0; i < cs.Len(); i++ {
			o.Charsets = append(o.Charsets, bs(cs.Index(i).String()))
		}
	}
	o.HasPubKey = !v.FieldByName("pubKey").IsNil()
	return o
}

func randomDSNs(r *rand.Rand, n int) []string {
	alphabet := []byte("@:/()?&=%,;[]. \x00\xffabcdtu0123456789")
	var out []string
	for i := 0; i < n; i++ {
		s := []byte(mysqlDSNs[r.IntN(len(mysqlDSNs))])
		for j, k := 0, 1+r.IntN(2); j < k; j++ {
			p := 0
			if len(s) > 0 {
				p = r.IntN(len(s) + 1)
			}
			switch r.IntN(3) {
			case 0:
				if p < len(s) {
					s = append(s[:p], s[p+1:]...)
				}
			case 1:
				s = append(s[:p], append([]byte{alphabet[r.IntN(len(alphabet))]}, s[p:]...)...)
			case 2:
				if p < len(s) {
					s[p] = alphabet[r.IntN(len(alphabet))]
				}
			}
		}
		out = append(out, string(s))
	}
	return out
}

// mysqlParse runs ParseDSN, recovering the panic of the removed strict mode.
func mysqlParse(dsn string) (cfg *mysql.Config, err error, p any) {
	defer func() { p = recover() }()
	cfg, err = mysql.ParseDSN(dsn)
	return
}

func appendMySQL(dsn string, params map[string]string) (out string, err error, p any) {
	defer func() { p = recover() }()
	out, err = storage.AppendMySQLParams(dsn, params)
	return
}

// mysqlRawByteDSNs put bytes that are not UTF-8 into each value that an
// error text embeds as it is (not quoted).
var mysqlRawByteDSNs = []string{
	"/d?loc=%FF", "/d?loc=A%FF/B", "/d?loc=%FF..x", "/d?loc=%2F%FF", "/d?loc=%5C%FF", "/d?loc=x%00y", "/d?loc=\xff",
	"/d?tls=%FF", "/d?tls=\xff", "/d?serverPubKey=%FF", "/d?serverPubKey=\xfe", "\xff()/d", "t\xffp()/d", "\xff(h)/d",
	"/d?timeTruncate=\xff", "/d?timeTruncate=1\xffs", "/d?compress=\xff", "/d?parseTime=\xff", "/d?readTimeout=\xff",
	"/d?maxAllowedPacket=\xff", "/d?charset=\xff", "/d?collation=\xff", "/d?\xff=\xfe", "\xff@/d", "u:\xff@/d",
}

func mysqlDSNGolden() map[string]any {
	r := rand.New(rand.NewPCG(14, 4))
	dsns := append(append([]string{}, mysqlDSNs...), randomDSNs(r, 700)...)
	dsns = append(dsns, mysqlRawByteDSNs...)
	var parsed []map[string]any
	for _, dsn := range dsns {
		c := map[string]any{"dsn": bs(dsn)}
		cfg, err, p := mysqlParse(dsn)
		switch {
		case p != nil:
			c["panic"] = bs(fmt.Sprint(p))
		case err != nil:
			c["err"] = bs(err.Error())
		default:
			c["config"] = dumpMySQLConfig(cfg)
			c["format"] = bs(cfg.FormatDSN())
		}
		parsed = append(parsed, c)
	}

	// AppendMySQLParams as storage/data and storage/cache call it, for each
	// variable name ProbeMySQLIsolationVariableName can return.
	var appended []map[string]any
	for _, name := range []string{"transaction_isolation", "tx_isolation", ""} {
		params := map[string]string{
			"sql_mode":  "'ONLY_FULL_GROUP_BY,STRICT_TRANS_TABLES,ERROR_FOR_DIVISION_BY_ZERO,NO_ENGINE_SUBSTITUTION'",
			name:        fmt.Sprintf("'%s'", storage.NewOptions().IsolationLevel),
			"parseTime": "true",
		}
		for _, dsn := range dsns {
			c := map[string]any{"isolation_var": name, "dsn": bs(dsn)}
			out, err, p := appendMySQL(dsn, params)
			switch {
			case p != nil:
				c["panic"] = bs(fmt.Sprint(p))
			case err != nil:
				c["err"] = bs(err.Error())
			default:
				c["out"] = bs(out)
			}
			appended = append(appended, c)
		}
	}
	return map[string]any{"parse": parsed, "append": appended}
}
