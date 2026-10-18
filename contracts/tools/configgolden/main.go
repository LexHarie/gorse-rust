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

// Command configgolden writes what the Go config package does with a corpus
// of configuration files and environment variables, for the Rust port's tests
// (crates/gorse-config):
//
//	go run ./contracts/tools/configgolden -out crates/gorse-config/tests/data/config_golden.json
//
// The file holds:
//
//   - load: config.LoadConfig on a file (TOML, YAML, JSON or dotenv, chosen by
//     the extension as viper does; or no file, a missing one, a directory) with
//     environment variables set. A loaded config is recorded as json.Marshal
//     writes it, the Meta.config the master sends (contracts/grpc.md), with the
//     raw fields of every feedback type expression (Marshal prints only the
//     string form), the recommender names and the recommender hashes. A
//     failure is recorded as its error text; parse_error marks viper's
//     ConfigParseError, whose text comes from the format's parser.
//   - validate: a JSON document decoded by json.Unmarshal over a base config
//     (as workers and servers decode Meta.config), then Config.Validate.
//   - hash: a JSON RecommendConfig, its recommender names and hashes.
//
// HOME is set to a temporary directory, written "${HOME}" in the output, and
// the case files live in another, written "${DIR}".
package main

import (
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"strings"
	"unicode/utf8"

	"github.com/gorse-io/gorse/common/expression"
	"github.com/gorse-io/gorse/common/log"
	"github.com/gorse-io/gorse/config"
	"github.com/spf13/viper"
	"gopkg.in/yaml.v3"
)

// Expr is a feedback type expression's fields; the value is its float64 bits
// in hex.
type Expr struct {
	FeedbackType string `json:"feedback_type"`
	ExprType     int    `json:"expr_type"`
	Value        string `json:"value"`
}

// Recommender is a recommender's full name and hash.
type Recommender struct {
	Name string `json:"name"`
	Hash string `json:"hash"`
}

// Hashes are what RecommendConfig says about its recommenders.
type Hashes struct {
	List         []string      `json:"list"`
	Hash         string        `json:"hash"`
	Recommenders []Recommender `json:"recommenders"`
}

// LoadCase is one call of config.LoadConfig.
type LoadCase struct {
	Name string `json:"name"`
	// File is the file name in the case directory; empty means LoadConfig("").
	File string `json:"file,omitempty"`
	// Text is the file's contents, written unless Missing or Dir; TextHex
	// holds them instead when they are not UTF-8.
	Text    string            `json:"text,omitempty"`
	TextHex string            `json:"text_hex,omitempty"`
	Missing bool              `json:"missing,omitempty"`
	Dir     bool              `json:"dir,omitempty"`
	Env     map[string]string `json:"env,omitempty"`

	Meta       string            `json:"meta,omitempty"`
	MetaError  string            `json:"meta_error,omitempty"`
	Exprs      map[string][]Expr `json:"exprs,omitempty"`
	Hashes     *Hashes           `json:"hashes,omitempty"`
	Error      string            `json:"error,omitempty"`
	ParseError bool              `json:"parse_error,omitempty"`
	Panic      string            `json:"panic,omitempty"`
}

// ValidateCase is one call of Config.Validate.
type ValidateCase struct {
	Name string `json:"name"`
	// Base is "default" (GetDefaultConfig) or "suite" (the default with the
	// cache and data stores ValidateTestSuite sets).
	Base  string `json:"base"`
	Patch string `json:"patch"`
	Error string `json:"error,omitempty"`
}

// HashCase is one RecommendConfig.
type HashCase struct {
	Name      string `json:"name"`
	Recommend string `json:"recommend"`
	Hashes
}

// Golden is the output file.
type Golden struct {
	Bindings []Binding      `json:"bindings"`
	Default  string         `json:"default"`
	Load     []LoadCase     `json:"load"`
	Validate []ValidateCase `json:"validate"`
	Hash     []HashCase     `json:"hash"`
}

var home, dir string

// clean replaces the temporary directories in s.
func clean(s string) string {
	return strings.ReplaceAll(strings.ReplaceAll(s, dir, "${DIR}"), home, "${HOME}")
}

// Binding is a viper key and the environment variable bound to it.
type Binding struct {
	Key string `json:"key"`
	Env string `json:"env"`
}

// bindings are what LoadConfig binds, read from its unexported table.
func bindings() []Binding {
	text, err := os.ReadFile("config/config.go")
	if err != nil {
		panic(err)
	}
	var out []Binding
	start := strings.Index(string(text), "var bindings = []configBinding{")
	for _, line := range strings.Split(string(text)[start:], "\n")[1:] {
		line = strings.TrimSpace(line)
		if line == "}" {
			break
		}
		fields := strings.Split(strings.Trim(line, "{},"), ", ")
		out = append(out, Binding{strings.Trim(fields[0], `"`), strings.Trim(fields[1], `"`)})
	}
	return out
}

func exprs(list []expression.FeedbackTypeExpression) []Expr {
	out := make([]Expr, 0, len(list))
	for _, e := range list {
		out = append(out, Expr{e.FeedbackType, int(e.ExprType), fmt.Sprintf("%016x", math.Float64bits(e.Value))})
	}
	return out
}

func hashes(r *config.RecommendConfig) *Hashes {
	h := &Hashes{List: r.ListRecommenders(), Hash: r.Hash(), Recommenders: []Recommender{}}
	for _, c := range r.NonPersonalized {
		h.Recommenders = append(h.Recommenders, Recommender{c.FullName(), c.Hash()})
	}
	for _, c := range r.ItemToItem {
		h.Recommenders = append(h.Recommenders, Recommender{c.FullName(), c.Hash(r)})
	}
	for _, c := range r.UserToUser {
		h.Recommenders = append(h.Recommenders, Recommender{c.FullName(), c.Hash(r)})
	}
	for _, c := range r.External {
		h.Recommenders = append(h.Recommenders, Recommender{c.FullName(), c.Hash()})
	}
	h.Recommenders = append(h.Recommenders, Recommender{r.Collaborative.FullName(), r.Collaborative.Hash(r)})
	return h
}

func load(c LoadCase, envs []Binding) LoadCase {
	for _, b := range envs {
		_ = os.Unsetenv(b.Env)
	}
	for k, v := range c.Env {
		_ = os.Setenv(k, v)
	}
	defer func() {
		for k := range c.Env {
			_ = os.Unsetenv(k)
		}
	}()
	caseDir, err := os.MkdirTemp(dir, "case")
	if err != nil {
		panic(err)
	}
	defer func() { _ = os.RemoveAll(caseDir) }()
	path := ""
	if c.File != "" {
		path = filepath.Join(caseDir, c.File)
		switch {
		case c.Dir:
			if err := os.Mkdir(path, 0o755); err != nil {
				panic(err)
			}
		case !c.Missing:
			if err := os.WriteFile(path, []byte(c.Text), 0o644); err != nil {
				panic(err)
			}
		}
	}
	defer func() {
		if p := recover(); p != nil {
			c.Panic = clean(fmt.Sprint(p))
		}
	}()
	conf, err := config.LoadConfig(path)
	if err != nil {
		c.Error = clean(strings.ReplaceAll(err.Error(), caseDir, "${DIR}"))
		var parseErr viper.ConfigParseError
		c.ParseError = errors.As(err, &parseErr)
		return c
	}
	if meta, err := json.Marshal(conf); err != nil {
		c.MetaError = err.Error()
	} else {
		c.Meta = clean(string(meta))
	}
	ds := conf.Recommend.DataSource
	c.Exprs = map[string][]Expr{
		"positive_feedback_types": exprs(ds.PositiveFeedbackTypes),
		"negative_feedback_types": exprs(ds.NegativeFeedbackTypes),
		"read_feedback_types":     exprs(ds.ReadFeedbackTypes),
	}
	c.Hashes = hashes(&conf.Recommend)
	return c
}

func validate(c ValidateCase) ValidateCase {
	conf := config.GetDefaultConfig()
	if c.Base == "suite" {
		conf.Database.CacheStore = "redis://localhost:6379/0"
		conf.Database.DataStore = "mysql://gorse:gorse_pass@tcp(localhost:3306)/gorse"
	}
	if err := json.Unmarshal([]byte(c.Patch), conf); err != nil {
		panic(fmt.Sprintf("%s: %v", c.Name, err))
	}
	if err := conf.Validate(); err != nil {
		c.Error = err.Error()
	}
	return c
}

func hash(c HashCase) HashCase {
	var r config.RecommendConfig
	if err := json.Unmarshal([]byte(c.Recommend), &r); err != nil {
		panic(fmt.Sprintf("%s: %v", c.Name, err))
	}
	c.Hashes = *hashes(&r)
	return c
}

// sample is config/config.toml.
var sample = config.ConfigTOML

// unmarshalText is the sample as TestUnmarshal changes it.
func unmarshalText() string {
	text := sample
	for _, r := range [][2]string{
		{"ssl_mode = false", "ssl_mode = true"},
		{"ssl_ca = \"\"", "ssl_ca = \"ca.pem\""},
		{"ssl_cert = \"\"", "ssl_cert = \"cert.pem\""},
		{"ssl_key = \"\"", "ssl_key = \"key.pem\""},
		{"dashboard_user_name = \"\"", "dashboard_user_name = \"admin\""},
		{"dashboard_password = \"\"", "dashboard_password = \"password\""},
		{"admin_api_key = \"\"", "admin_api_key = \"super_api_key\""},
		{"api_key = \"\"", "api_key = \"19260817\""},
		{"table_prefix = \"\"", "table_prefix = \"gorse_\""},
		{"cache_table_prefix = \"gorse_\"", "cache_table_prefix = \"gorse_cache_\""},
		{"data_table_prefix = \"gorse_\"", "data_table_prefix = \"gorse_data_\""},
		{"http_cors_domains = []", "http_cors_domains = [\".*\"]"},
		{"http_cors_methods = []", "http_cors_methods = [\"GET\",\"PATCH\",\"POST\"]"},
		{"issuer = \"\"", "issuer = \"https://accounts.google.com\""},
		{"client_id = \"\"", "client_id = \"client_id\""},
		{"client_secret = \"\"", "client_secret = \"client_secret\""},
		{"redirect_url = \"\"", "redirect_url = \"http://localhost:8088/callback/oauth2\""},
		{"auth_token = \"\"", "auth_token = \"<reranker_auth_token>\""},
		{"url = \"https://dashscope.aliyuncs.com/compatible-api/v1/reranks\"", "url = \"<reranker_url>\""},
	} {
		text = strings.ReplaceAll(text, r[0], r[1])
	}
	return text
}

// convert reads TOML text with viper and writes it as YAML or JSON, as
// TestUnmarshal does with yj.
func convert(text, format string) string {
	v := viper.New()
	v.SetConfigType("toml")
	if err := v.ReadConfig(strings.NewReader(text)); err != nil {
		panic(err)
	}
	var out []byte
	var err error
	if format == "yaml" {
		out, err = yaml.Marshal(v.AllSettings())
	} else {
		out, err = json.MarshalIndent(v.AllSettings(), "", "  ")
	}
	if err != nil {
		panic(err)
	}
	return string(out)
}

// field makes a case setting one key of one TOML table to a TOML value.
func field(table, key, value string) LoadCase {
	return LoadCase{
		Name: fmt.Sprintf("%s.%s = %s", table, key, value),
		File: "config.toml",
		Text: fmt.Sprintf("[%s]\n%s = %s\n", table, key, value),
	}
}

func toml(name, text string) LoadCase {
	return LoadCase{Name: name, File: "config.toml", Text: text}
}

func env(name string, vars map[string]string) LoadCase {
	return LoadCase{Name: name, Env: vars}
}

func loadCases(envs []Binding) []LoadCase {
	full := unmarshalText()
	cases := []LoadCase{
		{Name: "no file"},
		{Name: "missing file", File: "missing.toml", Missing: true},
		{Name: "directory", File: "config.toml", Dir: true},
		toml("sample", sample),
		toml("TestUnmarshal toml", full),
		{Name: "TestUnmarshal yaml", File: "config.yaml", Text: convert(full, "yaml")},
		{Name: "TestUnmarshal json", File: "config.json", Text: convert(full, "json")},
		{Name: "sample yml", File: "config.yml", Text: convert(sample, "yaml")},
		toml("empty", ""),
		toml("TestReloadConfig", "[server]\ndefault_n = 42\n"),
		{Name: "unsupported extension", File: "config.txt", Text: "[server]\ndefault_n = 42\n"},
		{Name: "upper-case extension", File: "config.TOML", Text: "[server]\ndefault_n = 42\n"},
		{Name: "no extension", File: "config", Text: "[server]\ndefault_n = 42\n"},
		{Name: "dotenv", File: "config.env", Text: "database.data_store=mysql://root@tcp(db)/gorse\nMASTER.PORT=1234\nserver.auto_insert_user=false\n"},
		{Name: "dotenv extension", File: "config.dotenv", Text: "master.n_jobs=3\n"},
		{Name: "properties extension", File: "config.properties", Text: "master.n_jobs=3\n"},
		{Name: "hcl extension", File: "config.hcl", Text: "master { n_jobs = 3 }\n"},
		{Name: "ini extension", File: "config.ini", Text: "[master]\nn_jobs = 3\n"},
		toml("toml syntax error", "[master\nport = 1\n"),
		toml("toml duplicate key", "[master]\nport = 1\nport = 2\n"),
		toml("toml bad value", "[master]\nport = \n"),
		{Name: "yaml syntax error", File: "config.yaml", Text: "master: [\n"},
		{Name: "yaml tab indent", File: "config.yaml", Text: "master:\n\tport: 1\n"},
		{Name: "json syntax error", File: "config.json", Text: "{\"master\": {\"port\": 1}\n"},
		{Name: "json top-level array", File: "config.json", Text: "[1, 2]\n"},
		{Name: "yaml top-level scalar", File: "config.yaml", Text: "5\n"},
		toml("upper-case keys", "[MASTER]\nPORT = 1234\nHttp_Port = 4321\n[Server]\nDefault_N = 7\n"),
		toml("quoted dotted key", "\"master.port\" = 5555\n"),
		toml("dotted key", "master.port = 5555\nmaster.host = \"h\"\n"),
		toml("unknown keys", "[master]\nport = 1\nunknown = \"x\"\n[unknown]\na = 1\n"),
		toml("table where a value is expected", "[master.port]\na = 1\n"),
		toml("value where a table is expected", "master = 5\n"),
		toml("array where a table is expected", "master = [1, 2]\n"),
		{Name: "yaml null value", File: "config.yaml", Text: "master:\n  host: null\n  port: ~\n"},
		{Name: "yaml empty table", File: "config.yaml", Text: "master:\n"},
		{Name: "yaml hex int", File: "config.yaml", Text: "master:\n  port: 0x10\n"},
		{Name: "yaml yes", File: "config.yaml", Text: "master:\n  ssl_mode: yes\n"},
		{Name: "yaml bool", File: "config.yaml", Text: "master:\n  ssl_mode: true\n  host: 1.5\n"},
		{Name: "yaml duration int", File: "config.yaml", Text: "master:\n  meta_timeout: 10\n"},
		{Name: "yaml anchor", File: "config.yaml", Text: "a: &x\n  port: 77\nmaster: *x\n"},
		{Name: "json float int", File: "config.json", Text: "{\"master\": {\"port\": 8080.5, \"n_jobs\": 2}}"},
		{Name: "json string int", File: "config.json", Text: "{\"master\": {\"port\": \"8080\"}}"},
		{Name: "json null", File: "config.json", Text: "{\"master\": {\"host\": null}}"},
		{Name: "json big int", File: "config.json", Text: "{\"master\": {\"port\": 1e300}}"},
		{Name: "json recommenders", File: "config.json", Text: "{\"recommend\": {\"non-personalized\": [{\"name\": \"a\", \"score\": \"1\"}], \"ranker\": {\"recommenders\": [\"non-personalized/a\"]}}}"},
	}

	for _, v := range []string{`8080`, `"8080"`, `8080.9`, `-8080.9`, `1e3`, `true`, `false`, `"0x1F90"`, `"0o17"`,
		`"010"`, `"0b11"`, `"1_000"`, `"+5"`, `" 5"`, `"5 "`, `""`, `"abc"`, `"1e3"`, `"8080.5"`, `[1]`, `[]`, `{a = 1}`, `-1`,
		`9223372036854775807`, `-9223372036854775808`, `"9223372036854775808"`, `"-9223372036854775809"`, `1.0e19`, `-1.0e19`,
		`nan`, `inf`, `-inf`, `0x10`, `0o10`, `0b10`, `1979-05-27`, `1979-05-27T07:32:00Z`, `07:32:00`} {
		cases = append(cases, field("master", "port", v))
	}
	for _, v := range []string{`0`, `"0"`, `0.5`, `-0.5`} {
		cases = append(cases, field("master", "n_jobs", v))
	}
	for _, v := range []string{`true`, `"true"`, `"1"`, `"t"`, `"T"`, `"TRUE"`, `"True"`, `"tRUE"`, `"yes"`, `"0"`, `"f"`,
		`"FALSE"`, `""`, `" true"`, `1`, `0`, `2`, `-1`, `1.5`, `0.0`, `-0.0`, `nan`, `[true]`, `{a = true}`} {
		cases = append(cases, field("master", "ssl_mode", v))
	}
	for _, v := range []string{`"x"`, `123`, `-1.5`, `1e20`, `1e21`, `1e-7`, `0.1`, `100.0`, `-0.0`, `nan`, `inf`, `true`,
		`false`, `["a"]`, `[]`, `{a = "b"}`, `1979-05-27T07:32:00Z`, `1979-05-27T07:32:00.123+08:00`, `1979-05-27`,
		`07:32:00`, `"\u00e9\u0000"`, `9223372036854775807`} {
		cases = append(cases, field("master", "host", v))
	}
	for _, v := range []string{`"1h"`, `"90"`, `90`, `"1.5h"`, `"-1s"`, `"0s"`, `"0"`, `"bad"`, `1.5`, `-1`, `0`, `""`,
		`"1h1h"`, `"1µs"`, `"1μs"`, `"1us"`, `".5s"`, `"5.s"`, `"."`, `"9999999h"`, `"2562047h47m16.854775807s"`,
		`"2562047h47m16.854775808s"`, `"1h "`, `" 1h"`, `true`, `["1h"]`, `"+1m"`, `"-0s"`, `"1ms2ns"`, `"1H"`,
		`"0.000000001s"`, `"0.0000000001s"`, `"1.00000000000000000001h"`, `9223372036854775807`, `1e19`, `nan`} {
		cases = append(cases, field("master", "meta_timeout", v))
	}
	for _, v := range []string{`"-1s"`, `-1`, `"0s"`} {
		cases = append(cases, field("database.mysql", "conn_max_lifetime", v))
	}
	for _, v := range []string{`5`, `-1`, `"-1"`, `"5"`, `5.7`, `-5.7`, `"18446744073709551615"`, `"18446744073709551616"`,
		`"0x10"`, `true`, `1e19`, `1e20`, `-1e19`, `nan`, `"abc"`} {
		cases = append(cases, field("recommend.data_source", "positive_feedback_ttl", v))
	}
	for _, v := range []string{`1`, `"0.5"`, `"abc"`, `nan`, `inf`, `-inf`, `-0.0`, `0`, `true`, `"1e400"`, `"0x1p-2"`,
		`"Inf"`, `"NaN"`, `"+inf"`, `"1_0"`, `" 1"`, `""`, `1e-320`, `"4.9e-324"`, `[1.0]`} {
		cases = append(cases, field("recommend.replacement", "positive_replacement_decay", v))
	}
	for _, v := range []string{`nan`, `inf`, `-0.0`, `0.1`, `1e300`} {
		cases = append(cases, field("tracing", "ratio", v))
	}
	for _, v := range []string{`[]`, `["a"]`, `"a,b"`, `[1, 2]`, `[true]`, `[["a"]]`, `1`, `""`, `["x", 1.5]`, `[{a = 1}]`,
		`{a = "b"}`} {
		cases = append(cases, field("master", "http_cors_domains", v))
	}
	for _, v := range []string{`["latest"]`, `"latest"`, `[]`, `["latest", "collaborative"]`, `["collaborative"]`,
		`["not_exist"]`, `[""]`, `""`, `[1]`, `"latest,collaborative"`} {
		cases = append(cases, field("recommend.ranker", "recommenders", v))
	}
	for _, v := range []string{`["star", "read>=3"]`, `"star"`, `["bad expr!"]`, `[1]`, `["a=5"]`, `[]`, `["like", "like"]`,
		`["read>1e6"]`, `["x<=-0.5"]`, `[""]`, `""`, `["a.5"]`, `["a<1e-5"]`, `[["a"]]`, `[{FeedbackType = "a"}]`,
		`{FeedbackType = "a"}`, `"star,like"`, `[" star"]`, `["star>"]`, `["star>=99999999999999999999999"]`} {
		cases = append(cases, field("recommend.data_source", "positive_feedback_types", v))
	}
	cases = append(cases,
		toml("negative and read feedback types", "[recommend.data_source]\nnegative_feedback_types = [\"dislike\", \"rate<2\"]\nread_feedback_types = [\"read\"]\n"),
		toml("non-personalized", "[[recommend.non-personalized]]\nname = \"popular\"\nscore = \"count(feedback)\"\nfilter = \"item.Timestamp > now()\"\n"),
		toml("non-personalized without score", "[[recommend.non-personalized]]\nname = \"popular\"\n"),
		toml("non-personalized bad score", "[[recommend.non-personalized]]\nname = \"popular\"\nscore = \"count(\"\n"),
		toml("non-personalized bad filter", "[[recommend.non-personalized]]\nname = \"popular\"\nscore = \"1\"\nfilter = \"a ==\"\n"),
		toml("non-personalized duplicate", "[[recommend.non-personalized]]\nname = \"a\"\nscore = \"1\"\n[[recommend.non-personalized]]\nname = \"a\"\nscore = \"2\"\n"),
		toml("non-personalized without name", "[[recommend.non-personalized]]\nscore = \"1\"\n"),
		toml("non-personalized as a table", "[recommend.non-personalized]\nname = \"a\"\nscore = \"1\"\n"),
		toml("non-personalized as a string", "[recommend]\nnon-personalized = \"a\"\n"),
		toml("non-personalized unknown field", "[[recommend.non-personalized]]\nname = \"a\"\nscore = \"1\"\nweight = 3\n"),
		toml("non-personalized upper-case fields", "[[recommend.non-personalized]]\nNAME = \"a\"\nScore = \"1\"\n"),
		toml("non-personalized number score", "[[recommend.non-personalized]]\nname = \"a\"\nscore = 1\n"),
		toml("item-to-item types", "[[recommend.item-to-item]]\nname = \"e\"\ntype = \"embedding\"\ncolumn = \"item.Labels.embedding\"\n[[recommend.item-to-item]]\nname = \"t\"\ntype = \"tags\"\ncolumn = \"item.Labels.tags\"\n[[recommend.item-to-item]]\nname = \"u\"\ntype = \"users\"\n[[recommend.item-to-item]]\nname = \"a\"\ntype = \"auto\"\n"),
		toml("item-to-item bad type", "[[recommend.item-to-item]]\nname = \"x\"\ntype = \"items\"\n"),
		toml("item-to-item without type", "[[recommend.item-to-item]]\nname = \"x\"\n"),
		toml("item-to-item bad column", "[[recommend.item-to-item]]\nname = \"x\"\ntype = \"tags\"\ncolumn = \"item.Labels.\"\n"),
		toml("item-to-item duplicate", "[[recommend.item-to-item]]\nname = \"x\"\ntype = \"users\"\n[[recommend.item-to-item]]\nname = \"x\"\ntype = \"auto\"\n"),
		toml("user-to-user types", "[[recommend.user-to-user]]\nname = \"e\"\ntype = \"embedding\"\n[[recommend.user-to-user]]\nname = \"t\"\ntype = \"tags\"\n[[recommend.user-to-user]]\nname = \"i\"\ntype = \"items\"\n[[recommend.user-to-user]]\nname = \"a\"\ntype = \"auto\"\n"),
		toml("user-to-user bad type", "[[recommend.user-to-user]]\nname = \"x\"\ntype = \"users\"\n"),
		toml("user-to-user duplicate", "[[recommend.user-to-user]]\nname = \"x\"\ntype = \"items\"\n[[recommend.user-to-user]]\nname = \"x\"\ntype = \"auto\"\n"),
		toml("external", "[[recommend.external]]\nname = \"e\"\nscript = \"return []\"\n"),
		toml("ranker with every recommender", "[recommend.collaborative]\ntype = \"mf\"\n[[recommend.non-personalized]]\nname = \"p\"\nscore = \"1\"\n[[recommend.item-to-item]]\nname = \"i\"\ntype = \"users\"\n[[recommend.user-to-user]]\nname = \"u\"\ntype = \"items\"\n[[recommend.external]]\nname = \"e\"\nscript = \"x\"\n[recommend.data_source]\npositive_feedback_types = [\"like\"]\nnegative_feedback_types = [\"hate\"]\n[recommend.ranker]\ntype = \"fm\"\nrecommenders = [\"latest\", \"collaborative\", \"non-personalized/p\", \"item-to-item/i\", \"user-to-user/u\", \"external/e\"]\n[recommend.fallback]\nrecommenders = [\"item-to-item/i\", \"latest\"]\n"),
		toml("ranker none with two recommenders", "[recommend.ranker]\ntype = \"none\"\nrecommenders = [\"latest\", \"latest\"]\n"),
		toml("ranker collaborative disabled", "[recommend.ranker]\ntype = \"fm\"\nrecommenders = [\"collaborative\"]\n"),
		toml("fallback unknown", "[recommend.fallback]\nrecommenders = [\"nope\"]\n"),
		toml("fallback collaborative disabled", "[recommend.fallback]\nrecommenders = [\"collaborative\"]\n"),
		toml("fallback empty", "[recommend.fallback]\nrecommenders = []\n"),
		toml("fallback as a value", "[recommend]\nfallback = 5\n"),
		toml("decision without url", "[recommend.ranker]\ntype = \"decision\"\n"),
		toml("decision blank url", "[recommend.ranker]\ntype = \"decision\"\n[recommend.ranker.decision_api]\nurl = \" \\t\"\nmodel = \"m\"\n"),
		toml("decision without model", "[recommend.ranker]\ntype = \"decision\"\n[recommend.ranker.decision_api]\nurl = \"http://d\"\n"),
		toml("decision", "[recommend.ranker]\ntype = \"decision\"\n[recommend.ranker.decision_api]\nurl = \"http://d\"\nmodel = \"m\"\n"),
		toml("decision before validator", "[master]\nport = -1\n[recommend.ranker]\ntype = \"decision\"\n"),
		toml("duplicate before validator", "[master]\nport = -1\n[[recommend.non-personalized]]\nname = \"a\"\nscore = \"1\"\n[[recommend.non-personalized]]\nname = \"a\"\nscore = \"1\"\n"),
		toml("validator before recommenders", "[master]\nport = -1\n[recommend.ranker]\nrecommenders = []\n"),
		toml("first validator error wins", "[master]\nport = -1\nn_jobs = 0\n[database]\ndata_store = \"\"\n[blob]\nuri = \"\"\n"),
		toml("two struct errors", "[[recommend.non-personalized]]\nname = \"a\"\n[[recommend.item-to-item]]\nname = \"b\"\n"),
		toml("table prefix only", "[database]\ntable_prefix = \"gorse_\"\n"),
		toml("table prefix and cache prefix", "[database]\ntable_prefix = \"gorse_\"\ncache_table_prefix = \"c_\"\n"),
		toml("table prefix compat", strings.ReplaceAll(strings.ReplaceAll(strings.ReplaceAll(sample,
			"cache_table_prefix = \"\"", ""), "data_table_prefix = \"\"", ""), "table_prefix = \"\"", "table_prefix = \"gorse_\"")),
		toml("strings with markup", "[recommend.ranker.reranker_api]\nquery_template = \"<a href=\\\"x\\\">&amp;</a> \\u2028\"\n"),
		toml("integer limits", "[openai]\nchat_completion_rpm = 9223372036854775807\nembedding_dimensions = -9223372036854775808\n"),
		toml("tracing", "[tracing]\nenable_tracing = true\nexporter = \"zipkin\"\ncollector_endpoint = \"http://z\"\nsampler = \"ratio\"\nratio = 0.5\n"),
		toml("tracing bad exporter", "[tracing]\nexporter = \"jaeger\"\n"),
		toml("oidc", "[oidc]\nenable = true\nredirect_url = \"http://x/callback/oauth2\"\n"),
		toml("oidc bad redirect", "[oidc]\nredirect_url = \"http://x/callback\"\n"),
		toml("blob", "[blob]\nuri = \"s3://bucket/dir\"\n[blob.s3]\nendpoint = \"e\"\naccess_key_id = \"a\"\nsecret_access_key = \"s\"\n[blob.gcs]\ncredentials_file = \"c\"\n[blob.azure]\nendpoint = \"e\"\naccount_name = \"n\"\naccount_key = \"k\"\nconnection_string = \"cs\"\n"),
		toml("collaborative early stopping", "[recommend.collaborative]\ntype = \"mf\"\n[recommend.collaborative.early_stopping]\npatience = -3\n[recommend.ranker.early_stopping]\npatience = 4\n"),
		toml("search columns", "[recommend.search]\ncolumns = [\"item.Labels.description\", \"\"]\n"),
		toml("search bad column", "[recommend.search]\ncolumns = [\"item.Labels.\"]\n"),
		toml("data store prefixes", "[database]\ndata_store = \"mongodb+srv://h/db\"\ncache_store = \"rediss+cluster://h\"\nvector_store = \"weaviates://h\"\n"),
		toml("three decode errors", "[master]\nport = \"a\"\nhttp_port = \"b\"\n[server]\ndefault_n = \"c\"\n"),
		toml("decode errors in a slice", "[master]\nhttp_cors_domains = [[1], {a = 1}, \"ok\", [2]]\n"),
		toml("decode and hook errors", "[master]\nmeta_timeout = \"x\"\nport = \"y\"\n[recommend.data_source]\npositive_feedback_types = [\"!\", \"ok\", 1]\n"),
		toml("toml 1.1 inline table newline", "master = {\n  port = 1\n}\n"),
		toml("toml 1.1 inline table trailing comma", "master = { port = 1, }\n"),
		toml("toml 1.1 escape e", "[master]\nhost = \"\\e\"\n"),
		toml("toml 1.1 escape x", "[master]\nhost = \"\\x41\"\n"),
		toml("toml 1.1 time without seconds", "[master]\nhost = 07:32\n"),
		toml("toml literal and multi-line strings", "[master]\nhost = '''\nline\\n'''\nhttp_host = \"\"\"\n  a \\\n  b\"\"\"\nssl_ca = 'C:\\x'\n"),
		toml("toml local date-time", "[master]\nhost = 1979-05-27T07:32:00\n"),
		toml("toml array of tables then table", "[[recommend.external]]\nname = \"a\"\nscript = \"s\"\n[[recommend.external]]\nname = \"b\"\n[recommend.ranker]\ntype = \"fm\"\nrecommenders = [\"external/a\", \"external/b\"]\n"),
		toml("toml comments and whitespace", "# c\n[ master ] # c\n  port=1 # c\n\n\t[ \"server\" ]\ndefault_n=2\n"),
		toml("toml integer with underscores and sign", "[master]\nport = +1_0\nn_jobs = 0x_1\n"),
		toml("toml bad underscore", "[master]\nport = 1__0\n"),
		toml("toml leading zero", "[master]\nport = 010\n"),
		toml("toml float forms", "[recommend.replacement]\npositive_replacement_decay = 5e-1\nread_replacement_decay = 1_0.0_1\n[tracing]\nratio = +0.1\n"),
		toml("toml utf-8 bom", "\ufeff[master]\nport = 1\n"),
		toml("toml crlf", "[master]\r\nport = 1\r\n"),
		toml("toml invalid utf-8", "[master]\nhost = \"\xff\"\n"),
		toml("toml empty key", "[master]\n\"\" = 1\n"),
		toml("toml nested inline tables", "recommend = { ranker = { type = \"fm\", recommenders = [\"latest\"] }, cache_size = 5 }\n"),
		toml("toml dotted table header", "[recommend.ranker.reranker_api]\nmodel = \"m\"\n[recommend.ranker]\ntype = \"reranker\"\n"),
		toml("toml table redefined", "[master]\nport = 1\n[master]\nhost = \"h\"\n"),
		toml("toml super-table after sub-table", "[recommend.ranker]\ntype = \"fm\"\n[recommend]\ncache_size = 3\n"),
		LoadCase{Name: "json duplicate keys", File: "config.json", Text: "{\"master\": {\"port\": 1, \"port\": 2}}"},
		LoadCase{Name: "json top-level null", File: "config.json", Text: "null"},
		LoadCase{Name: "json empty file", File: "config.json", Text: ""},
		LoadCase{Name: "json huge number", File: "config.json", Text: "{\"master\": {\"port\": 1e400}}"},
		LoadCase{Name: "json negative zero", File: "config.json", Text: "{\"tracing\": {\"ratio\": -0}, \"master\": {\"port\": -0.0}}"},
		LoadCase{Name: "json lone surrogate", File: "config.json", Text: "{\"master\": {\"host\": \"\\ud800x\"}}"},
		LoadCase{Name: "json invalid utf-8", File: "config.json", Text: "{\"master\": {\"host\": \"\xff\"}}"},
		LoadCase{Name: "json bom", File: "config.json", Text: "\ufeff{}"},
		LoadCase{Name: "json trailing data", File: "config.json", Text: "{} {}"},
		LoadCase{Name: "json bool and array", File: "config.json", Text: "{\"master\": {\"ssl_mode\": \"true\", \"http_cors_methods\": \"GET\"}, \"server\": {\"auto_insert_user\": 0}}"},
		LoadCase{Name: "json duration number", File: "config.json", Text: "{\"master\": {\"meta_timeout\": 1500000000.7}}"},
		LoadCase{Name: "yaml multi-document", File: "config.yaml", Text: "master:\n  port: 1\n---\nmaster:\n  port: 2\n"},
		LoadCase{Name: "yaml merge key", File: "config.yaml", Text: "base: &b\n  port: 5\n  host: b\nmaster:\n  <<: *b\n  host: m\n"},
		LoadCase{Name: "yaml flow mapping", File: "config.yaml", Text: "master: {port: 7, http_cors_domains: [a, b]}\n"},
		LoadCase{Name: "yaml block scalars", File: "config.yaml", Text: "master:\n  host: |\n    a\n    b\n  http_host: >-\n    c\n    d\n"},
		LoadCase{Name: "yaml duplicate key", File: "config.yaml", Text: "master:\n  port: 1\n  port: 2\n"},
		LoadCase{Name: "yaml non-string key", File: "config.yaml", Text: "1: a\nmaster:\n  port: 3\n"},
		LoadCase{Name: "yaml bool key", File: "config.yaml", Text: "true: a\nmaster:\n  port: 3\n"},
		LoadCase{Name: "yaml null key", File: "config.yaml", Text: "~: a\nmaster:\n  port: 3\n"},
		LoadCase{Name: "yaml empty file", File: "config.yaml", Text: ""},
		LoadCase{Name: "yaml comments only", File: "config.yaml", Text: "# nothing\n"},
		LoadCase{Name: "yaml bom", File: "config.yaml", Text: "\ufeffmaster:\n  port: 4\n"},
		LoadCase{Name: "yaml sequence of maps", File: "config.yaml", Text: "recommend:\n  non-personalized:\n    - name: a\n      score: \"1\"\n    - name: b\n      score: \"2\"\n"},
		LoadCase{Name: "yaml explicit document", File: "config.yaml", Text: "%YAML 1.1\n---\nmaster:\n  port: 9\n...\n"},
		LoadCase{Name: "dotenv forms", File: "config.env", Text: "export master.host=h1\nmaster.http_host = \"h 2\"\nserver.api_key='k#1'\nmaster.ssl_ca=a # comment\nmaster.ssl_cert=\"a\\nb\"\nmaster.ssl_key=$HOME/k\nmaster.dashboard_user_name=${master.host}x\nmaster.dashboard_password=\n"},
		LoadCase{Name: "dotenv bad line", File: "config.env", Text: "master.port\n"},
		LoadCase{Name: "dotenv colon", File: "config.env", Text: "master.port: 3\n"},
		LoadCase{Name: "dotenv upper-case and underscores", File: "config.env", Text: "MASTER_PORT=3\nMASTER.N_JOBS=4\n"},
		LoadCase{Name: "dotenv crlf", File: "config.env", Text: "master.port=5\r\n"},
		LoadCase{Name: "dotenv single-quoted no expansion", File: "config.env", Text: "master.host='$HOME'\n"},
		LoadCase{Name: "dotenv unterminated quote", File: "config.env", Text: "master.host=\"abc\n"},
	)
	for _, v := range []string{`~`, `null`, `Null`, `NULL`, `nULL`, `true`, `True`, `TRUE`, `tRUE`, `false`, `yes`, `no`, `on`,
		`off`, `y`, `n`, `0x1F`, `0X1F`, `0o17`, `017`, `09`, `0b101`, `+12`, `-12`, `1_000`, `.inf`, `-.Inf`, `+.INF`, `.nan`,
		`.NaN`, `1e3`, `1.5e+3`, `1.`, `.5`, `-0`, `-0.0`, `0.1`, `1e400`, `18446744073709551615`, `18446744073709551616`,
		`9223372036854775808`, `-9223372036854775809`, `2001-12-14t21:59:43.10-05:00`, `2002-12-14`, `'123'`, `"1\t2"`,
		`!!str 123`, `!!int "5"`, `!!float 1`, `!!bool "true"`, `!!binary aGVsbG8=`, `!!timestamp 2002-12-14`, `!!null x`,
		`[1, 2]`, `{a: 1}`, `""`, `''`, `"\u00e9"`, `a: b`, `- x`, `&a 5`, `!custom 5`, `0.0.1`, `1:20`, `=`, `<<`} {
		cases = append(cases,
			LoadCase{Name: "yaml master.host: " + v, File: "config.yaml", Text: "master:\n  host: " + v + "\n"},
			LoadCase{Name: "yaml master.port: " + v, File: "config.yaml", Text: "master:\n  port: " + v + "\n"})
	}
	for _, c := range []struct{ table, key, value string }{
		{"database", "data_store", `""`}, {"database", "data_store", `"redis://h"`}, {"database", "data_store", `"MYSQL://h"`},
		{"database", "data_store", `"postgresql://h"`}, {"database", "data_store", `"mongodb://h"`},
		{"database", "cache_store", `""`}, {"database", "cache_store", `"xvec://h"`}, {"database", "cache_store", `"redis+cluster://h"`},
		{"database", "vector_store", `""`}, {"database", "vector_store", `"sqlite://h"`}, {"database", "vector_store", `"qdrant://h"`},
		{"database", "vector_store", `"milvus://h"`},
		{"database.mysql", "isolation_level", `"BAD"`}, {"database.mysql", "isolation_level", `"SERIALIZABLE"`},
		{"database.mysql", "isolation_level", `""`}, {"database.mysql", "isolation_level", `"read-committed"`},
		{"database.mysql", "max_open_conns", `-1`}, {"database.mysql", "max_idle_conns", `-1`},
		{"database.postgres", "max_open_conns", `-1`}, {"database.postgres", "max_idle_conns", `-1`},
		{"database.postgres", "conn_max_lifetime", `"-1s"`},
		{"database.redis", "max_search_results", `0`},
		{"database.vector", "quantization_type", `"xx"`}, {"database.vector", "quantization_type", `"pq"`},
		{"database.vector", "quantization_bits", `-1`},
		{"master", "http_port", `-1`}, {"master", "n_jobs", `-1`}, {"master", "meta_timeout", `"0s"`},
		{"server", "default_n", `0`}, {"server", "clock_error", `"-1s"`}, {"server", "cache_expire", `"0s"`},
		{"recommend", "cache_size", `0`}, {"recommend", "cache_expire", `"0s"`}, {"recommend", "context_size", `0`},
		{"recommend", "active_user_ttl", `-1`},
		{"recommend.collaborative", "type", `"x"`}, {"recommend.collaborative", "type", `""`},
		{"recommend.collaborative", "fit_period", `"0s"`}, {"recommend.collaborative", "fit_epoch", `0`},
		{"recommend.collaborative", "optimize_period", `"-1s"`}, {"recommend.collaborative", "optimize_trials", `0`},
		{"recommend.replacement", "read_replacement_decay", `0`}, {"recommend.replacement", "read_replacement_decay", `-0.5`},
		{"recommend.ranker", "type", `"x"`}, {"recommend.ranker", "type", `"reranker"`}, {"recommend.ranker", "cache_expire", `"0s"`},
		{"recommend.ranker", "fit_period", `"0s"`}, {"recommend.ranker", "fit_epoch", `0`},
		{"recommend.ranker", "optimize_period", `"-1s"`}, {"recommend.ranker", "optimize_trials", `0`},
		{"blob", "uri", `""`},
		{"quota", "max_users_count", `-1`}, {"quota", "max_items_count", `-1`}, {"quota", "max_labels_size", `-1`},
		{"quota", "max_comment_size", `-1`}, {"quota", "max_categories_count", `-1`}, {"quota", "max_categories_size", `-1`},
		{"quota", "max_users_count", `5`},
	} {
		cases = append(cases, field(c.table, c.key, c.value))
	}

	bindEnv := map[string]string{
		"GORSE_CACHE_STORE": "redis://<cache_store>", "GORSE_DATA_STORE": "mysql://<data_store>",
		"GORSE_VECTOR_STORE": "qdrant://<vector_store>", "GORSE_TABLE_PREFIX": "gorse_",
		"GORSE_DATA_TABLE_PREFIX": "gorse_data_", "GORSE_CACHE_TABLE_PREFIX": "gorse_cache_",
		"GORSE_CACHE_CLIENT_NAME": "gorse_cache_client_from_env", "GORSE_VECTOR_TABLE_PREFIX": "gorse_vector_",
		"GORSE_VECTOR_QUANTIZATION_TYPE": "rq", "GORSE_VECTOR_QUANTIZATION_BITS": "8", "GORSE_MASTER_PORT": "123",
		"GORSE_MASTER_HOST": "<master_host>", "GORSE_MASTER_SSL_MODE": "true", "GORSE_MASTER_SSL_CA": "ca.pem",
		"GORSE_MASTER_SSL_CERT": "cert.pem", "GORSE_MASTER_SSL_KEY": "key.pem", "GORSE_MASTER_HTTP_PORT": "456",
		"GORSE_MASTER_HTTP_HOST": "<master_http_host>", "GORSE_MASTER_JOBS": "789", "GORSE_DASHBOARD_USER_NAME": "user_name",
		"GORSE_DASHBOARD_PASSWORD": "password", "GORSE_DASHBOARD_AUTH_SERVER": "http://127.0.0.1:8888",
		"GORSE_DASHBOARD_REDACTED": "true", "GORSE_ADMIN_API_KEY": "<admin_api_key>", "GORSE_SERVER_API_KEY": "<server_api_key>",
		"GORSE_OIDC_ENABLE": "true", "GORSE_OIDC_ISSUER": "https://accounts.google.com", "GORSE_OIDC_CLIENT_ID": "client_id",
		"GORSE_OIDC_CLIENT_SECRET": "client_secret", "GORSE_OIDC_REDIRECT_URL": "http://localhost:8088/callback/oauth2",
		"GORSE_BLOB_URI": "s3://<bucket>/path", "S3_ENDPOINT": "https://s3.example.com", "S3_ACCESS_KEY_ID": "<access_key_id>",
		"S3_SECRET_ACCESS_KEY": "<secret_access_key>", "GCS_CREDENTIALS_FILE": "/path/to/credentials.json",
		"AZURE_STORAGE_ENDPOINT": "https://<account>.blob.core.windows.net", "AZURE_STORAGE_ACCOUNT_NAME": "<account_name>",
		"AZURE_STORAGE_ACCOUNT_KEY":       "<account_key>",
		"AZURE_STORAGE_CONNECTION_STRING": "DefaultEndpointsProtocol=https;AccountName=<account>;AccountKey=<key>",
		"OPENAI_BASE_URL":                 "https://api.openai.com/v1", "OPENAI_AUTH_TOKEN": "<auth_token>", "OPENAI_CHAT_COMPLETION_MODEL": "gpt-4",
		"RERANKER_AUTH_TOKEN": "<reranker_auth_token>", "RERANKER_URL": "<reranker_url>", "RERANKER_MODEL": "<reranker_model>",
		"DECISION_AUTH_TOKEN": "<decision_auth_token>", "DECISION_URL": "<decision_url>", "DECISION_MODEL": "<decision_model>",
		"GORSE_QUOTA_MAX_USERS_COUNT": "9", "GORSE_QUOTA_MAX_ITEMS_COUNT": "10", "GORSE_QUOTA_MAX_LABELS_SIZE": "11",
		"GORSE_QUOTA_MAX_COMMENT_SIZE": "12", "GORSE_QUOTA_MAX_CATEGORIES_COUNT": "13", "GORSE_QUOTA_MAX_CATEGORIES_SIZE": "14",
	}
	cases = append(cases, LoadCase{Name: "TestBindEnv", File: "config.toml", Text: sample, Env: bindEnv})
	empty := map[string]string{}
	for _, b := range envs {
		empty[b.Env] = ""
	}
	cases = append(cases, env("TestSetDefault: empty variables", empty))
	cases = append(cases, LoadCase{Name: "empty variable keeps the file value", File: "config.toml",
		Text: "[database]\ndata_store = \"mysql://file\"\n", Env: map[string]string{"GORSE_DATA_STORE": ""}})
	cases = append(cases, LoadCase{Name: "variable over the file", File: "config.toml",
		Text: "[master]\nport = 1\nhost = \"file\"\n", Env: map[string]string{"GORSE_MASTER_PORT": "2"}})
	cases = append(cases, LoadCase{Name: "variable over a missing file", File: "missing.toml", Missing: true,
		Env: map[string]string{"GORSE_MASTER_PORT": "3"}})
	cases = append(cases, LoadCase{Name: "variable and file table prefixes", File: "config.toml",
		Text: "[database]\ndata_table_prefix = \"d_\"\n", Env: map[string]string{"GORSE_TABLE_PREFIX": "t_"}})
	cases = append(cases, LoadCase{Name: "variable under a file value of another type", File: "config.toml",
		Text: "master = 5\n", Env: map[string]string{"GORSE_MASTER_PORT": "4"}})
	for _, v := range []string{"0x1F90", "010", "0b11", "0o17", "1_000", "+5", " 5", "5 ", "abc", "-1", "-0",
		"99999999999999999999", "1e3", "8080.5", "٣", "0x", "_1", "1__0", "0_1"} {
		cases = append(cases, env("GORSE_MASTER_PORT="+v, map[string]string{"GORSE_MASTER_PORT": v}))
	}
	for _, v := range []string{"1", "t", "T", "TRUE", "True", "tRuE", "yes", "0", "f", "F", "FALSE", "False", "2", " true"} {
		cases = append(cases, env("GORSE_MASTER_SSL_MODE="+v, map[string]string{"GORSE_MASTER_SSL_MODE": v}))
	}
	for _, v := range []string{"-1", "x", "0x8"} {
		cases = append(cases, env("GORSE_QUOTA_MAX_USERS_COUNT="+v, map[string]string{"GORSE_QUOTA_MAX_USERS_COUNT": v}))
		cases = append(cases, env("GORSE_VECTOR_QUANTIZATION_BITS="+v, map[string]string{"GORSE_VECTOR_QUANTIZATION_BITS": v}))
	}
	cases = append(cases,
		env("GORSE_DATA_STORE=redis://x", map[string]string{"GORSE_DATA_STORE": "redis://x"}),
		env("GORSE_VECTOR_QUANTIZATION_TYPE=xx", map[string]string{"GORSE_VECTOR_QUANTIZATION_TYPE": "xx"}),
		env("GORSE_OIDC_REDIRECT_URL=http://x", map[string]string{"GORSE_OIDC_REDIRECT_URL": "http://x"}),
		env("GORSE_OIDC_ENABLE=on", map[string]string{"GORSE_OIDC_ENABLE": "on"}),
		env("GORSE_MASTER_HOST with spaces and markup", map[string]string{"GORSE_MASTER_HOST": " <a&b> "}),
		env("DECISION_URL without a decision ranker", map[string]string{"DECISION_URL": "http://d"}),
		LoadCase{Name: "DECISION_URL and DECISION_MODEL for a decision ranker", File: "config.toml",
			Text: "[recommend.ranker]\ntype = \"decision\"\n",
			Env:  map[string]string{"DECISION_URL": "http://d", "DECISION_MODEL": "m"}},
		LoadCase{Name: "variable with a yaml file", File: "config.yaml", Text: "master:\n  port: 1\n",
			Env: map[string]string{"GORSE_MASTER_PORT": "0x10"}},
	)
	return cases
}

func validateCases() []ValidateCase {
	var cases []ValidateCase
	add := func(base, name, patch string) {
		cases = append(cases, ValidateCase{Name: name, Base: base, Patch: patch})
	}
	add("suite", "suite base", `{}`)
	add("default", "default base", `{}`)
	add("suite", "TestDuplicateNonPersonalized", `{"Recommend": {"NonPersonalized": [{"name": "most_starred_weekly", "score": "count(feedback, .FeedbackType == 'star')"}, {"name": "most_starred_weekly", "score": "count(feedback, .FeedbackType == 'star')"}]}}`)
	add("suite", "TestDuplicateItemToItem", `{"Recommend": {"ItemToItem": [{"name": "item_to_item", "type": "users"}, {"name": "item_to_item", "type": "users"}]}}`)
	add("suite", "TestRecommendersExistence not_exist", `{"Recommend": {"Ranker": {"Recommenders": ["not_exist"]}}}`)
	add("suite", "TestRecommendersExistence collaborative", `{"Recommend": {"Collaborative": {"Type": "none"}, "Ranker": {"Recommenders": ["collaborative"]}}}`)
	add("suite", "TestSearchColumns good", `{"Recommend": {"Search": {"Columns": ["item.Labels.description"]}}}`)
	add("suite", "TestSearchColumns bad", `{"Recommend": {"Search": {"Columns": ["item.Labels."]}}}`)
	add("default", "TestDecisionAPIRequiredConfig url", `{"Recommend": {"Ranker": {"Type": "decision"}}}`)
	add("default", "TestDecisionAPIRequiredConfig model", `{"Recommend": {"Ranker": {"Type": "decision", "DecisionAPI": {"URL": "http://localhost/decisions"}}}}`)
	add("default", "TestDecisionAPIRequiredConfig ok", `{"Recommend": {"Ranker": {"Type": "decision", "DecisionAPI": {"URL": "http://localhost/decisions", "Model": "test-model"}}}}`)
	add("suite", "TestQuota", `{"Quota": {"MaxLabelsSize": -1}}`)
	add("suite", "TestCacheStore redis+cluster", `{"Database": {"CacheStore": "redis+cluster://:password@192.168.1.11:6379?addr=192.168.0.5:6379&addr=192.168.0.7:6379"}}`)
	add("suite", "TestCacheStore rediss+cluster", `{"Database": {"CacheStore": "rediss+cluster://:password@192.168.1.11:6379?addr=192.168.0.5:6379"}}`)
	add("suite", "TestVectorStore empty", `{"Database": {"VectorStore": ""}}`)
	for _, v := range []string{"xvec://vectors", "qdrant://localhost:6334", "weaviate://localhost:8080",
		"weaviates://localhost:8080", "milvus://localhost:19530", "sqlite://:memory:", "mysql://localhost:3306/gorse"} {
		add("suite", "TestVectorStore "+v, fmt.Sprintf(`{"Database": {"VectorStore": %q}}`, v))
	}
	add("suite", "negative duration", `{"Master": {"MetaTimeout": -1}}`)
	add("suite", "zero uint", `{"Recommend": {"DataSource": {"PositiveFeedbackTTL": 0, "ItemTTL": 18446744073709551615}}}`)
	add("suite", "nil recommenders", `{"Recommend": {"Ranker": {"Recommenders": null}}}`)
	add("suite", "user-to-user duplicate", `{"Recommend": {"UserToUser": [{"name": "a", "type": "auto"}, {"name": "a", "type": "auto"}]}}`)
	add("suite", "item-to-item and user-to-user share a name", `{"Recommend": {"ItemToItem": [{"name": "a", "type": "auto"}], "UserToUser": [{"name": "a", "type": "auto"}], "Ranker": {"Type": "fm", "Recommenders": ["item-to-item/a", "user-to-user/a"]}}}`)
	add("suite", "external without script", `{"Recommend": {"External": [{"name": "e"}], "Ranker": {"Recommenders": ["external/e"]}}}`)
	add("suite", "fallback", `{"Recommend": {"Fallback": {"Recommenders": ["latest", "nope"]}}}`)
	add("suite", "decision whitespace model", `{"Recommend": {"Ranker": {"Type": "decision", "DecisionAPI": {"URL": "u", "Model": "\n"}}}}`)
	add("suite", "collaborative named in ranker when enabled", `{"Recommend": {"Collaborative": {"Type": "mf"}, "Ranker": {"Type": "fm", "Recommenders": ["collaborative", "latest"]}}}`)
	add("suite", "unicode name", `{"Recommend": {"NonPersonalized": [{"name": "\u00e9", "score": "1"}], "Ranker": {"Recommenders": ["non-personalized/\u00e9"]}}}`)
	add("suite", "nested error field names", `{"Recommend": {"NonPersonalized": [{"name": "a", "score": "1"}, {"name": "b"}]}}`)
	add("suite", "oidc endswith exact", `{"OIDC": {"RedirectURL": "/callback/oauth2"}}`)
	add("suite", "oidc endswith case", `{"OIDC": {"RedirectURL": "http://x/Callback/OAuth2"}}`)
	add("suite", "item_expr with whitespace", `{"Recommend": {"Search": {"Columns": [" "]}}}`)
	return cases
}

func hashCases() []HashCase {
	var cases []HashCase
	add := func(name, recommend string) {
		cases = append(cases, HashCase{Name: name, Recommend: recommend})
	}
	add("empty", `{}`)
	add("non-personalized a", `{"NonPersonalized": [{"name": "a"}]}`)
	add("non-personalized b", `{"NonPersonalized": [{"name": "b"}]}`)
	add("non-personalized fields", `{"NonPersonalized": [{"name": "n", "score": "s", "filter": "f"}, {"name": "ns", "score": "f"}]}`)
	add("item-to-item", `{"ItemToItem": [{"name": "a", "type": "users", "column": "c"}, {"name": "b", "type": "tags", "column": "c"}]}`)
	add("item-to-item users with feedback", `{"ItemToItem": [{"name": "a", "type": "users"}], "DataSource": {"PositiveFeedbackTypes": ["like", "read>=3"], "NegativeFeedbackTypes": ["dislike"]}}`)
	add("user-to-user items with feedback", `{"UserToUser": [{"name": "a", "type": "items"}, {"name": "b", "type": "auto"}], "DataSource": {"PositiveFeedbackTypes": ["like"], "NegativeFeedbackTypes": ["x<0.5"]}}`)
	add("external", `{"External": [{"name": "a", "script": "s"}]}`)
	add("collaborative like", `{"DataSource": {"PositiveFeedbackTypes": ["like"]}}`)
	add("collaborative star", `{"DataSource": {"PositiveFeedbackTypes": ["star"]}}`)
	add("ranker latest", `{"DataSource": {"PositiveFeedbackTypes": ["like"]}, "Ranker": {"Recommenders": ["latest"]}}`)
	add("ranker collaborative", `{"DataSource": {"PositiveFeedbackTypes": ["star"]}, "Ranker": {"Recommenders": ["collaborative"]}}`)
	add("user-to-user order a b", `{"UserToUser": [{"name": "a"}, {"name": "b"}], "Ranker": {"Recommenders": ["user-to-user/a", "user-to-user/b"]}}`)
	add("user-to-user order b a", `{"UserToUser": [{"name": "b"}, {"name": "a"}], "Ranker": {"Recommenders": ["user-to-user/b", "user-to-user/a"]}}`)
	add("ranker names nothing configured", `{"Ranker": {"Recommenders": ["external/x"]}}`)
	add("every kind", `{"NonPersonalized": [{"name": "p", "score": "1"}], "ItemToItem": [{"name": "i", "type": "users"}], "UserToUser": [{"name": "u", "type": "items"}], "External": [{"name": "e", "script": "x"}], "DataSource": {"PositiveFeedbackTypes": ["like"], "NegativeFeedbackTypes": ["hate"]}, "Ranker": {"Recommenders": ["latest", "collaborative", "non-personalized/p", "item-to-item/i", "user-to-user/u", "external/e"]}}`)
	add("every kind, ranker subset", `{"NonPersonalized": [{"name": "p", "score": "1"}], "ItemToItem": [{"name": "i", "type": "users"}], "UserToUser": [{"name": "u", "type": "items"}], "External": [{"name": "e", "script": "x"}], "Ranker": {"Recommenders": ["external/e", "item-to-item/i"]}}`)
	add("expression with a large value", `{"ItemToItem": [{"name": "a", "type": "users"}], "DataSource": {"PositiveFeedbackTypes": ["read>=1000000", "x<0.00001", "y=3"]}}`)
	add("unicode", `{"NonPersonalized": [{"name": "\u00e9\u4e2d", "score": "\u00ff"}]}`)
	return cases
}

func main() {
	out := flag.String("out", "", "output JSON file")
	flag.Parse()
	var err error
	if home, err = os.MkdirTemp("", "configgolden-home"); err != nil {
		panic(err)
	}
	defer func() { _ = os.RemoveAll(home) }()
	if dir, err = os.MkdirTemp("", "configgolden-cases"); err != nil {
		panic(err)
	}
	defer func() { _ = os.RemoveAll(dir) }()
	if err = os.Setenv("HOME", home); err != nil {
		panic(err)
	}
	// LoadConfig warns about missing files.
	log.CloseLogger()

	envs := bindings()
	defaults, err := json.Marshal(config.GetDefaultConfig())
	if err != nil {
		panic(err)
	}
	golden := Golden{Bindings: envs, Default: clean(string(defaults))}
	for _, c := range loadCases(envs) {
		c = load(c, envs)
		if !utf8.ValidString(c.Text) {
			c.TextHex, c.Text = hex.EncodeToString([]byte(c.Text)), ""
		}
		golden.Load = append(golden.Load, c)
	}
	for _, c := range validateCases() {
		golden.Validate = append(golden.Validate, validate(c))
	}
	for _, c := range hashCases() {
		golden.Hash = append(golden.Hash, hash(c))
	}
	// Every case must have done what it records: a load either loaded or failed.
	for _, c := range golden.Load {
		if c.Meta == "" && c.MetaError == "" && c.Error == "" && c.Panic == "" {
			panic(c.Name + ": nothing recorded")
		}
	}
	data, err := json.MarshalIndent(golden, "", " ")
	if err != nil {
		panic(err)
	}
	if err = os.WriteFile(*out, append(data, '\n'), 0o644); err != nil {
		panic(err)
	}
	fmt.Printf("%d load, %d validate, %d hash cases\n", len(golden.Load), len(golden.Validate), len(golden.Hash))
}
