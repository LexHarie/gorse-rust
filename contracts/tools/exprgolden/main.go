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

// Command exprgolden writes what common/expression does with a corpus of
// inputs, for the Rust port's tests:
//
//   - parse: FromString of every corpus string, applied to a zero value and to
//     a value already holding an expression (FromString leaves Value alone
//     when the input has none, and sets fields before it fails);
//   - format: String and MarshalJSON of hand-picked and random values;
//   - to_pb, from_pb: the protocol conversions, with the Go protobuf bytes;
//   - match, match_any: Match and MatchFeedbackTypeExpressions;
//   - unmarshal_json: UnmarshalJSON of raw JSON values;
//   - decode_slice, encode_slice: encoding/json of []FeedbackTypeExpression,
//     the shape every Gorse caller uses;
//   - must_parse: whether MustParseFeedbackTypeExpression panics, and with
//     what.
//
// The corpus is the expressions found in the Go tests and config files of the
// pinned tree, edge cases of the pattern, and seeded random strings over the
// pattern's alphabet. Floats are written as their IEEE bits in hex so both
// sides read them exactly; raw JSON inputs are hex so they may hold any bytes.
package main

import (
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"math"
	"math/rand/v2"
	"os"
	"strings"

	"github.com/gorse-io/gorse/common/expression"
	"github.com/gorse-io/gorse/protocol"
	"google.golang.org/protobuf/proto"
)

// State is a FeedbackTypeExpression; Value is the hex of its IEEE bits.
type State struct {
	FeedbackType string `json:"feedback_type"`
	ExprType     int    `json:"expr_type"`
	Value        string `json:"value"`
}

// PB is a protocol.FeedbackTypeExpression.
type PB struct {
	FeedbackType   string `json:"feedback_type"`
	ExpressionType int32  `json:"expression_type"`
	Value          string `json:"value"`
}

type ParseCase struct {
	Source string  `json:"source"`
	Start  State   `json:"start"`
	Input  string  `json:"input"`
	Error  *string `json:"error"`
	State  State   `json:"state"`
}

type FormatCase struct {
	State       State  `json:"state"`
	String      string `json:"string"`
	MarshalJSON string `json:"marshal_json"`
}

type ToPBCase struct {
	State State  `json:"state"`
	PB    PB     `json:"pb"`
	Hex   string `json:"hex"`
}

type FromPBCase struct {
	Start State  `json:"start"`
	PB    PB     `json:"pb"`
	Hex   string `json:"hex"`
	State State  `json:"state"`
}

type MatchCase struct {
	State        State  `json:"state"`
	FeedbackType string `json:"feedback_type"`
	Value        string `json:"value"`
	Match        bool   `json:"match"`
}

type MatchAnyCase struct {
	Exprs        []State `json:"exprs"`
	FeedbackType string  `json:"feedback_type"`
	Value        string  `json:"value"`
	Match        bool    `json:"match"`
}

type UnmarshalCase struct {
	Start   State   `json:"start"`
	JSONHex string  `json:"json_hex"`
	Error   *string `json:"error"`
	State   State   `json:"state"`
}

type DecodeSliceCase struct {
	JSON   string  `json:"json"`
	Error  *string `json:"error"`
	Result []State `json:"result"`
}

type EncodeSliceCase struct {
	Exprs []State `json:"exprs"`
	JSON  string  `json:"json"`
}

type MustParseCase struct {
	Input string  `json:"input"`
	Panic *string `json:"panic"`
	State *State  `json:"state"`
}

type Golden struct {
	Parse         []ParseCase       `json:"parse"`
	Format        []FormatCase      `json:"format"`
	ToPB          []ToPBCase        `json:"to_pb"`
	FromPB        []FromPBCase      `json:"from_pb"`
	Match         []MatchCase       `json:"match"`
	MatchAny      []MatchAnyCase    `json:"match_any"`
	UnmarshalJSON []UnmarshalCase   `json:"unmarshal_json"`
	DecodeSlice   []DecodeSliceCase `json:"decode_slice"`
	EncodeSlice   []EncodeSliceCase `json:"encode_slice"`
	MustParse     []MustParseCase   `json:"must_parse"`
}

func bits(f float64) string {
	return fmt.Sprintf("%016x", math.Float64bits(f))
}

func state(e expression.FeedbackTypeExpression) State {
	return State{FeedbackType: e.FeedbackType, ExprType: int(e.ExprType), Value: bits(e.Value)}
}

func pbOf(pb *protocol.FeedbackTypeExpression) PB {
	return PB{FeedbackType: pb.FeedbackType, ExpressionType: int32(pb.ExpressionType), Value: bits(pb.Value)}
}

func errText(err error) *string {
	if err == nil {
		return nil
	}
	s := err.Error()
	return &s
}

func marshalPB(pb *protocol.FeedbackTypeExpression) string {
	data, err := proto.MarshalOptions{Deterministic: true}.Marshal(pb)
	if err != nil {
		panic(err)
	}
	return hex.EncodeToString(data)
}

// corpus is every expression string the Go tree uses, then edge cases.
var corpus = []struct{ source, input string }{
	// common/expression/expression_test.go
	{"expression_test.go", "test"},
	{"expression_test.go", "1a"},
	{"expression_test.go", "test<16"},
	{"expression_test.go", "test<=16"},
	{"expression_test.go", "test>16"},
	{"expression_test.go", "test>=16"},
	{"expression_test.go", "test+"},
	// config/config.toml, contracts/config/config.toml, client/config.toml
	{"config.toml", "star"},
	{"config.toml", "like"},
	{"config.toml", "read>=3"},
	{"config.toml", "read"},
	{"client/config.toml", "rating>=4"},
	{"client/config.toml", "rating"},
	// MustParseFeedbackTypeExpression calls in the Go tests
	{"go tests", "FeedbackType"},
	{"go tests", "a"},
	{"go tests", "b"},
	{"go tests", "click"},
	{"go tests", "dislike"},
	{"go tests", "n"},
	{"go tests", "negative"},
	{"go tests", "p"},
	{"go tests", "positive"},
	{"go tests", "read>=100"},
	{"go tests", "type"},
	// server/rest_test.go feedback-type path parameters
	{"server/rest_test.go", "type1"},
	{"server/rest_test.go", "type2"},
	// edge cases of the pattern
	{"edge", ""},
	{"edge", "A"},
	{"edge", "Z9_"},
	{"edge", "a_b_c"},
	{"edge", "_a"},
	{"edge", "9"},
	{"edge", "a=5"},
	{"edge", "a=5.5"},
	{"edge", "a="},
	{"edge", "a=="},
	{"edge", "a<"},
	{"edge", "a<="},
	{"edge", "a>"},
	{"edge", "a>="},
	{"edge", "a<>"},
	{"edge", "a=<1"},
	{"edge", "a=>1"},
	{"edge", "a<<1"},
	{"edge", "a."},
	{"edge", "a<."},
	{"edge", "a>=."},
	{"edge", "a<1."},
	{"edge", "a<.5"},
	{"edge", "a<0"},
	{"edge", "a<00"},
	{"edge", "a<007.500"},
	{"edge", "a.5"},
	{"edge", "a1.5"},
	{"edge", "a1"},
	{"edge", "a12.34"},
	{"edge", "a1."},
	{"edge", "a<1.2.3"},
	{"edge", "a<-1"},
	{"edge", "a<+1"},
	{"edge", "a<1e5"},
	{"edge", "a<1E5"},
	{"edge", "a<0x10"},
	{"edge", "a<inf"},
	{"edge", "a<NaN"},
	{"edge", "a<1_000"},
	{"edge", "a <1"},
	{"edge", "a< 1"},
	{"edge", " a"},
	{"edge", "a "},
	{"edge", "a\n"},
	{"edge", "\na"},
	{"edge", "a<1\n"},
	{"edge", "a\x00"},
	{"edge", "a-b"},
	{"edge", "a.b"},
	{"edge", "\u00e9"},
	{"edge", "a\u00e9"},
	{"edge", "a<\u0661"},
	{"edge", "a<\uff11"},
	{"edge", "a<0.1"},
	{"edge", "a<0.30000000000000004"},
	{"edge", "a<1000000"},
	{"edge", "a<999999"},
	{"edge", "a<0.0001"},
	{"edge", "a<0.00001"},
	{"edge", "a<123456.7"},
	{"edge", "a<0.000000029802322387695312"}, // 2^-25: its two shortest forms tie
	{"edge", "a<9007199254740993"},
	{"edge", "a<179769313486231570000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"},
	{"edge", "a<" + strings.Repeat("9", 309)},
	{"edge", "a<" + strings.Repeat("9", 400)},
	{"edge", "a<" + strings.Repeat("9", 400) + "."},
	{"edge", "a<." + strings.Repeat("0", 400) + "1"},
	{"edge", "a<0." + strings.Repeat("0", 323) + "5"},
	{"edge", "a<0." + strings.Repeat("0", 323) + "2"},
	{"edge", "a<0." + strings.Repeat("0", 323) + "247032822920623272088284396434110686182529901307162382212792841250337753635104375932649918180817996189898282347722858865463328355177969898199387398005390939063150356595155702263922908583924491051844359318028499365361525003193704576782492193656236698636584807570015857692699037063119282795585513329278343384093519780155312465972635795746227664652728272200563740064854999770965994704540208281662262378573934507363390079677619305775067401763246736009689513405355374585166611342237666786041621596804619144672918403005300575308490487653917113865916462395249126236538818796362393732804238910186723484976682350898633885879256283027559956575244555072551893136908362547791869486679949683240497058210285131854513962138377228261454376934125320985913276672363281255"},
	{"edge", "a<" + "1" + strings.Repeat("0", 22)},
	{"edge", "a<9007199254740993." + strings.Repeat("0", 800) + "1"},
	{"edge", "a<9007199254740993." + strings.Repeat("0", 900)},
	{"edge", "a<9007199254740995"},
	{"edge", "a" + strings.Repeat("b", 1000) + "<1"},
}

func parseCases(rng *rand.Rand) []ParseCase {
	inputs := corpus
	// Random strings over the pattern's alphabet plus characters it rejects.
	alphabet := []rune("aZ09_<>=.-+e \u00e9")
	for i := 0; i < 400; i++ {
		n := rng.IntN(9)
		var b strings.Builder
		for j := 0; j < n; j++ {
			b.WriteRune(alphabet[rng.IntN(len(alphabet))])
		}
		inputs = append(inputs, struct{ source, input string }{"random", b.String()})
	}
	// Random well-formed expressions with random decimal values.
	ops := []string{"", "<", "<=", ">", ">=", "="}
	for i := 0; i < 300; i++ {
		var b strings.Builder
		b.WriteString("t")
		b.WriteString(ops[rng.IntN(len(ops))])
		for j := rng.IntN(20); j > 0; j-- {
			b.WriteByte(byte('0' + rng.IntN(10)))
		}
		if rng.IntN(2) == 0 {
			b.WriteByte('.')
			for j := rng.IntN(20); j > 0; j-- {
				b.WriteByte(byte('0' + rng.IntN(10)))
			}
		}
		inputs = append(inputs, struct{ source, input string }{"random value", b.String()})
	}
	starts := []expression.FeedbackTypeExpression{
		{},
		{FeedbackType: "previous", ExprType: expression.Greater, Value: 5},
	}
	var cases []ParseCase
	for i, in := range inputs {
		for j, start := range starts {
			if j > 0 && i >= len(corpus) {
				break // random inputs start from the zero value only
			}
			e := start
			err := e.FromString(in.input)
			cases = append(cases, ParseCase{
				Source: in.source,
				Start:  state(start),
				Input:  in.input,
				Error:  errText(err),
				State:  state(e),
			})
		}
	}
	return cases
}

var exprTypes = []expression.ExprType{
	expression.None, expression.Less, expression.LessOrEqual, expression.Greater, expression.GreaterOrEqual,
}

func formatCases(rng *rand.Rand) []FormatCase {
	values := []float64{
		0, math.Copysign(0, -1), 16, 6, 1, -1, 1.5, -1.5, 0.1, 0.1 + 0.2, 1.0 / 3,
		3, 100, 4, 50,
		999999, 1e6, 1e6 + 1, 123456.7, 1234567, 12345678, 99999.95, 999999.9,
		1e20, 1e21, 1e22, 1e100, 1e300, math.MaxFloat64, -math.MaxFloat64,
		1e-4, 1e-5, 0.00012345, 0.000099999, 1e-7, 1e-100, 1e-300,
		math.SmallestNonzeroFloat64, -math.SmallestNonzeroFloat64, 2.2250738585072014e-308,
		math.Pi, math.E, 9007199254740993, 9007199254740992, 1 << 53, 1<<63 - 1, 5e-324,
		math.Inf(1), math.Inf(-1), math.NaN(),
		math.Ldexp(1, -25), // 2.9802322387695312e-08: its two shortest forms tie
	}
	for e := -30; e <= 30; e++ {
		values = append(values, math.Pow(10, float64(e)), 1.5*math.Pow(10, float64(e)))
	}
	for i := 0; i < 400; i++ {
		values = append(values, math.Float64frombits(rng.Uint64()))
	}
	for i := 0; i < 200; i++ {
		// Short decimals, the shape config values have.
		values = append(values, float64(rng.IntN(2000000))/math.Pow(10, float64(rng.IntN(8))))
	}
	var cases []FormatCase
	add := func(e expression.FeedbackTypeExpression) {
		buf, err := e.MarshalJSON()
		if err != nil {
			panic(err)
		}
		cases = append(cases, FormatCase{State: state(e), String: e.String(), MarshalJSON: string(buf)})
	}
	for _, typ := range exprTypes {
		for _, v := range values[:12] {
			add(expression.FeedbackTypeExpression{FeedbackType: "test", ExprType: typ, Value: v})
		}
	}
	for _, v := range values {
		add(expression.FeedbackTypeExpression{FeedbackType: "v", ExprType: expression.GreaterOrEqual, Value: v})
	}
	// Feedback types only FromPB or Go code can produce, for JSON escaping.
	var control strings.Builder
	for c := 0; c < 0x20; c++ {
		control.WriteByte(byte(c))
	}
	for _, ft := range []string{
		"", "<>&", `"quote"`, `back\slash`, "slash/", control.String(), "\x7f", "\u2028\u2029", "\u00e9", "\u65e5\u672c", "\U0001F600",
		"\ufffd", "mixed<a&b>\"c\"\n",
	} {
		add(expression.FeedbackTypeExpression{FeedbackType: ft, ExprType: expression.None, Value: 1})
		add(expression.FeedbackTypeExpression{FeedbackType: ft, ExprType: expression.Less, Value: 1})
	}
	return cases
}

func toPBCases() []ToPBCase {
	var cases []ToPBCase
	for _, typ := range exprTypes {
		for _, v := range []float64{6, 0, math.Copysign(0, -1), math.Inf(1), math.NaN(), -2.5} {
			for _, ft := range []string{"test", "", "\u00e9<"} {
				e := expression.FeedbackTypeExpression{FeedbackType: ft, ExprType: typ, Value: v}
				pb := e.ToPB()
				cases = append(cases, ToPBCase{State: state(e), PB: pbOf(pb), Hex: marshalPB(pb)})
			}
		}
	}
	return cases
}

func fromPBCases() []FromPBCase {
	starts := []expression.FeedbackTypeExpression{
		{},
		{FeedbackType: "previous", ExprType: expression.Greater, Value: 5},
	}
	var cases []FromPBCase
	for _, start := range starts {
		for _, et := range []int32{0, 1, 2, 3, 4, 5, -1, 99, math.MaxInt32, math.MinInt32} {
			for _, v := range []float64{6, 0, math.Copysign(0, -1), math.NaN()} {
				pb := &protocol.FeedbackTypeExpression{
					FeedbackType:   "test",
					ExpressionType: protocol.ExpressionType(et),
					Value:          v,
				}
				// Decode the Go bytes, as a receiving node would.
				data, err := proto.Marshal(pb)
				if err != nil {
					panic(err)
				}
				var decoded protocol.FeedbackTypeExpression
				if err = proto.Unmarshal(data, &decoded); err != nil {
					panic(err)
				}
				e := start
				e.FromPB(&decoded)
				cases = append(cases, FromPBCase{Start: state(start), PB: pbOf(&decoded), Hex: hex.EncodeToString(data), State: state(e)})
			}
		}
	}
	return cases
}

func matchCases() []MatchCase {
	thresholds := []float64{6, 0, math.Copysign(0, -1), math.NaN(), math.Inf(1), math.Inf(-1)}
	values := []float64{5, 6, 7, 0, math.Copysign(0, -1), math.NaN(), math.Inf(1), math.Inf(-1),
		math.Nextafter(6, 7), math.Nextafter(6, 5)}
	var cases []MatchCase
	for _, typ := range exprTypes {
		for _, th := range thresholds {
			e := expression.FeedbackTypeExpression{FeedbackType: "test", ExprType: typ, Value: th}
			for _, ft := range []string{"test", "Test"} {
				for _, v := range values {
					cases = append(cases, MatchCase{State: state(e), FeedbackType: ft, Value: bits(v), Match: e.Match(ft, v)})
				}
			}
		}
	}
	return cases
}

func matchAnyCases() []MatchAnyCase {
	lists := [][]expression.FeedbackTypeExpression{
		nil,
		{{FeedbackType: "a"}, {FeedbackType: "b"}, {FeedbackType: "c"}},
		{expression.MustParseFeedbackTypeExpression("a<1"), expression.MustParseFeedbackTypeExpression("a>5")},
		{expression.MustParseFeedbackTypeExpression("star"), expression.MustParseFeedbackTypeExpression("like"),
			expression.MustParseFeedbackTypeExpression("read>=3")},
		{expression.MustParseFeedbackTypeExpression("a>=1"), expression.MustParseFeedbackTypeExpression("a")},
	}
	var cases []MatchAnyCase
	for _, list := range lists {
		var exprs []State
		for _, e := range list {
			exprs = append(exprs, state(e))
		}
		for _, ft := range []string{"a", "b", "d", "star", "like", "read"} {
			for _, v := range []float64{0, 1, 3, 5, 6, math.NaN()} {
				cases = append(cases, MatchAnyCase{
					Exprs: exprs, FeedbackType: ft, Value: bits(v),
					Match: expression.MatchFeedbackTypeExpressions(list, ft, v),
				})
			}
		}
	}
	return cases
}

func unmarshalCases() []UnmarshalCase {
	inputs := []string{
		`"test"`, `"1a"`, `"test<16"`, `"test<=16"`, `"test>16"`, `"test>=16"`,
		`"test\u003c16"`, `"\u0074est"`, `"a\/b"`, `"a<1e6"`, `""`, `" a"`, `"a<1."`, `"a<."`,
		`"a\ud800"`, `"a\udc00"`, `"\ud83d\ude00"`, `"\ud83dx"`, `"a\u0000"`, `"\u00e9"`, "\"\U0001F600\"", "\"\u00e9\"",
		"\"a\xff\"", "\"\xc3\xa9\"",
		` "spaced" `, "\t\n\"ws\"\r\n",
		`16`, `-1.5`, `0`, `1e400`, `true`, `false`, `null`, `{}`, `{"a":1}`, `[]`, `["a"]`,
	}
	starts := []expression.FeedbackTypeExpression{
		{},
		{FeedbackType: "previous", ExprType: expression.Greater, Value: 5},
	}
	var cases []UnmarshalCase
	for _, in := range inputs {
		if !json.Valid([]byte(in)) {
			panic("not valid JSON: " + in)
		}
		for _, start := range starts {
			e := start
			err := e.UnmarshalJSON([]byte(in))
			cases = append(cases, UnmarshalCase{
				Start: state(start), JSONHex: hex.EncodeToString([]byte(in)), Error: errText(err), State: state(e),
			})
		}
	}
	return cases
}

func decodeSliceCases() []DecodeSliceCase {
	inputs := []string{
		`[]`, `["star","like","read>=3"]`, `["read"]`, `["rating>=4"]`,
		`["a\u003c1","b\u003e=2.5"]`, `["a","1a"]`, `["a",16]`, `["a",null]`, `["a",true]`,
		`["a",{}]`, `["a",[]]`, `["a<."]`, `["a=5"]`, `["t<1","u",  "v>=0.5"]`,
	}
	var cases []DecodeSliceCase
	for _, in := range inputs {
		var exprs []expression.FeedbackTypeExpression
		err := json.Unmarshal([]byte(in), &exprs)
		c := DecodeSliceCase{JSON: in, Error: errText(err)}
		if err == nil {
			c.Result = []State{}
			for _, e := range exprs {
				c.Result = append(c.Result, state(e))
			}
		}
		cases = append(cases, c)
	}
	return cases
}

func encodeSliceCases() []EncodeSliceCase {
	lists := [][]expression.FeedbackTypeExpression{
		{},
		{expression.MustParseFeedbackTypeExpression("star"), expression.MustParseFeedbackTypeExpression("like"),
			expression.MustParseFeedbackTypeExpression("read>=3")},
		{expression.MustParseFeedbackTypeExpression("a<1"), expression.MustParseFeedbackTypeExpression("b>=0.5"),
			expression.MustParseFeedbackTypeExpression("c<=1000000"), expression.MustParseFeedbackTypeExpression("d=7")},
		{{FeedbackType: "x&y", ExprType: expression.Greater, Value: 1e-7}},
	}
	var cases []EncodeSliceCase
	for _, list := range lists {
		data, err := json.Marshal(list)
		if err != nil {
			panic(err)
		}
		exprs := []State{}
		for _, e := range list {
			exprs = append(exprs, state(e))
		}
		cases = append(cases, EncodeSliceCase{Exprs: exprs, JSON: string(data)})
	}
	return cases
}

func mustParseCases() []MustParseCase {
	var cases []MustParseCase
	for _, in := range []string{"test+", "test", "read>=3", "", "a<.", "a<" + strings.Repeat("9", 400)} {
		c := MustParseCase{Input: in}
		func() {
			defer func() {
				if r := recover(); r != nil {
					s := fmt.Sprint(r)
					c.Panic = &s
				}
			}()
			s := state(expression.MustParseFeedbackTypeExpression(in))
			c.State = &s
		}()
		cases = append(cases, c)
	}
	return cases
}

func main() {
	out := flag.String("out", "", "output JSON file")
	flag.Parse()
	rng := rand.New(rand.NewPCG(20261017, 7))
	g := Golden{
		Parse:         parseCases(rng),
		Format:        formatCases(rng),
		ToPB:          toPBCases(),
		FromPB:        fromPBCases(),
		Match:         matchCases(),
		MatchAny:      matchAnyCases(),
		UnmarshalJSON: unmarshalCases(),
		DecodeSlice:   decodeSliceCases(),
		EncodeSlice:   encodeSliceCases(),
		MustParse:     mustParseCases(),
	}
	if err := os.WriteFile(*out, write(g), 0o644); err != nil {
		panic(err)
	}
}

// write lays the golden file out one case per line, so diffs stay readable.
func write(g Golden) []byte {
	var raw map[string]json.RawMessage
	data, err := json.Marshal(g)
	if err != nil {
		panic(err)
	}
	if err = json.Unmarshal(data, &raw); err != nil {
		panic(err)
	}
	keys := []string{"parse", "format", "to_pb", "from_pb", "match", "match_any",
		"unmarshal_json", "decode_slice", "encode_slice", "must_parse"}
	if len(keys) != len(raw) {
		panic("golden section list is out of date")
	}
	var b strings.Builder
	b.WriteString("{\n")
	for i, key := range keys {
		var cases []json.RawMessage
		if err = json.Unmarshal(raw[key], &cases); err != nil {
			panic(err)
		}
		fmt.Fprintf(&b, "%q: [\n", key)
		for j, c := range cases {
			b.Write(c)
			if j+1 < len(cases) {
				b.WriteByte(',')
			}
			b.WriteByte('\n')
		}
		b.WriteString("]")
		if i+1 < len(keys) {
			b.WriteByte(',')
		}
		b.WriteByte('\n')
	}
	b.WriteString("}\n")
	return []byte(b.String())
}
