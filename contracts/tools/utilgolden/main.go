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

// Command utilgolden writes, with -out, the golden JSON that the Rust port of
// common/util/util.go, common/util/strconv.go and the parts of Go's strconv
// they rely on is checked against: util.ParseFloat, util.ParseInt,
// util.ParseUInt, util.FormatInt, strconv.Atoi, strconv.ParseInt/ParseUint
// with bit size 0, strconv.Quote (which the parse errors use),
// strconv.IsPrint at the edges of every range, util.ValidateId and util.MD5.
//
// Parse results are written as "<hex of the 64-bit pattern>|<error text>",
// with an empty error text on success. Floats are math.Float64bits of the
// float64 or math.Float32bits of the float32.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"math"
	"math/big"
	"os"
	"runtime"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/gorse-io/gorse/common/util"
)

// Golden is the whole output file.
type Golden struct {
	GoVersion  string        `json:"go_version"`
	Floats     []FloatCase   `json:"floats"`
	Ints       []IntCase     `json:"ints"`
	FormatInts []FormatCase  `json:"format_ints"`
	Quotes     []QuoteCase   `json:"quotes"`
	IsPrint    []IsPrintCase `json:"is_print"`
	ValidateID []ErrCase     `json:"validate_id"`
	MD5        []MD5Case     `json:"md5"`
}

// FloatCase is util.ParseFloat of one string for float32 and float64.
type FloatCase struct {
	In  string `json:"in"`
	F32 string `json:"f32"`
	F64 string `json:"f64"`
}

// IntCase is every integer parser on one string.
type IntCase struct {
	In    string `json:"in"`
	I8    string `json:"i8"`
	I16   string `json:"i16"`
	I32   string `json:"i32"`
	I64   string `json:"i64"`
	U8    string `json:"u8"`
	U16   string `json:"u16"`
	U32   string `json:"u32"`
	U64   string `json:"u64"`
	Atoi  string `json:"atoi"`
	Int0  string `json:"int0"`
	Uint0 string `json:"uint0"`
}

// FormatCase is util.FormatInt of one int64.
type FormatCase struct {
	In  int64  `json:"in"`
	Out string `json:"out"`
}

// QuoteCase is strconv.Quote of one string.
type QuoteCase struct {
	In  string `json:"in"`
	Out string `json:"out"`
}

// IsPrintCase is strconv.IsPrint of one code point.
type IsPrintCase struct {
	Rune  int32 `json:"rune"`
	Print bool  `json:"print"`
}

// ErrCase is the error text of a call on one string, empty on success.
type ErrCase struct {
	In  string `json:"in"`
	Err string `json:"err"`
}

// MD5Case is util.MD5 of a list of strings.
type MD5Case struct {
	In  []string `json:"in"`
	Out string   `json:"out"`
}

func result(bits uint64, err error) string {
	if err != nil {
		return fmt.Sprintf("%016x|%s", bits, err.Error())
	}
	return fmt.Sprintf("%016x|", bits)
}

type splitmix64 uint64

func (s *splitmix64) next() uint64 {
	*s += 0x9e3779b97f4a7c15
	z := uint64(*s)
	z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9
	z = (z ^ (z >> 27)) * 0x94d049bb133111eb
	return z ^ (z >> 31)
}

func (s *splitmix64) intn(n int) int { return int(s.next() % uint64(n)) }

func floatInputs() []string {
	in := []string{
		"", "0", "-0", "+0", "00", "0.0", "-0.0", ".0", "0.", ".", "-", "+", "1", "-1", "+1",
		"1.5", "-1.5", "1e5", "1E5", "1e+5", "1e-5", "1.e5", ".5", "5.", "+.5", "-.5", ".e5", "e5",
		"1e", "1e+", "1e-", "1.5e", " 1", "1 ", "1,5", "1.2.3", "1e5e5", "--1", "+-1", "0x", "0x1",
		"0x1p0", "0x1p-2", "0X1P+3", "0x1.8p1", "0x.8p1", "0x1.8", "0xp1", "0x1p", "0x1p+", "-0x1p-1074",
		"0x1p-1075", "0x1p-1076", "0x1.0000000000001p-1075", "0x1.fffffffffffffp1023", "0x1.fffffffffffff8p1023",
		"0x1p1024", "0x1p-149", "0x1p-150", "0x1.000002p-150", "0x1.fffffep127", "0x1.ffffffp127", "0x1p128",
		"0x123456789abcdef0123p0", "0x1.23456789abcdef0123456789p0", "0xAbCdEfp-4", "0x_1p0", "0x1_0p0",
		"0x1__0p0", "0x1p1_0", "0x0p0", "-0x0p0", "0x0.000000000000000000001p0",
		"inf", "Inf", "INF", "+inf", "-inf", "infinity", "Infinity", "+Infinity", "-INFINITY", "infin",
		"infinit", "infinityx", "infx", "nan", "NaN", "NAN", "+nan", "-nan", "nanx", "na", "i", "n",
		"1_000", "1_000.5", "1__0", "_1", "1_", "1._5", "1_.5", "0.000_1", "1e1_0", "1e_10", "1_0e1_0",
		"0_1", "0x_", "1.5_", "_",
		"1e308", "1.7976931348623157e308", "1.7976931348623158e308", "1.7976931348623159e308",
		"1.797693134862315807e308", "1.7976931348623157e309", "1e309", "-1e309", "1e400", "2e308",
		"4.9406564584124654e-324", "4.9406564584124653e-324", "2.4703282292062327e-324",
		"2.4703282292062328e-324", "2.47032822920623272e-324", "5e-324", "3e-324", "2e-324", "1e-324",
		"1e-400", "2.2250738585072011e-308", "2.2250738585072012e-308", "2.2250738585072014e-308",
		"3.4028234663852886e38", "3.4028235e38", "3.4028236e38", "3.40282356779733661637539395458142568448e38",
		"3.40282356779733661637539395458142568447e38", "3.5e38", "1e39", "1.401298464324817e-45",
		"7.006492321624085e-46", "7.006492321624086e-46", "1e-46", "1.1754943508222875e-38",
		"1e22", "1e23", "9007199254740993", "9007199254740992", "9007199254740991", "18446744073709551615",
		"18446744073709551616", "123456789012345678901234567890", "0.1", "0.2", "0.3", "1e-7", "100",
		"1e99999999999999999999", "1e-99999999999999999999", "0e99999999", "1e010000", "1e0010000",
		"0.1e10000", "1e+0000000000000000000000000000000001", "1e-0000000000000000000000000000000001",
		"8.589973e9", "8.589974e9", "8.5899735e9", "16777217", "16777219", "33554435",
		"6.103515625e-05", "0.3333333333333333", "3.141592653589793238462643383279502884197",
	}
	// A 0 followed by many zeros and a large exponent, which Go caps.
	in = append(in, "0."+strings.Repeat("0", 20000)+"1e12345", "1"+strings.Repeat("0", 400), "1e-"+strings.Repeat("9", 30))
	// Long mantissas: halfway points between neighbouring doubles and floats,
	// written exactly, and nudged by a tail far beyond 800 digits.
	g := splitmix64(11)
	for i := 0; i < 60; i++ {
		var lo, hi *big.Float
		if i%2 == 0 {
			x := math.Float64frombits(g.next() & 0x7fefffffffffffff)
			lo = new(big.Float).SetFloat64(x)
			hi = new(big.Float).SetFloat64(math.Nextafter(x, math.Inf(1)))
		} else {
			x := math.Float32frombits(uint32(g.next()) & 0x7f7fffff)
			lo = new(big.Float).SetFloat64(float64(x))
			hi = new(big.Float).SetFloat64(float64(math.Nextafter32(x, float32(math.Inf(1)))))
		}
		mid := new(big.Float).SetPrec(2000).Add(lo, hi)
		mid.Quo(mid, big.NewFloat(2))
		exact := mid.Text('e', 1200)
		mant, exp, _ := strings.Cut(exact, "e")
		mant = strings.TrimRight(strings.TrimRight(mant, "0"), ".")
		in = append(in, mant+"e"+exp)
		switch i % 3 {
		case 0:
			in = append(in, mant+strings.Repeat("0", 900)+"1e"+exp)
		case 1:
			in = append(in, "-"+mant+"e"+exp)
		}
	}
	// Shortest, %e and %x forms of random doubles and floats, and digit
	// strings that are cut short.
	for i := 0; i < 300; i++ {
		x := math.Float64frombits(g.next())
		y := math.Float32frombits(uint32(g.next()))
		in = append(in,
			strconv.FormatFloat(x, 'g', -1, 64),
			strconv.FormatFloat(x, 'e', 20, 64),
			strconv.FormatFloat(x, 'x', -1, 64),
			strconv.FormatFloat(float64(y), 'g', -1, 32),
			strconv.FormatFloat(float64(y), 'x', 3, 32),
		)
	}
	// Random strings over the characters the grammar cares about.
	const alphabet = "0123456789000111+-._eEpPxXabcdfinINFtyAN #"
	for i := 0; i < 1000; i++ {
		var b strings.Builder
		n := g.intn(14)
		for j := 0; j < n; j++ {
			b.WriteByte(alphabet[g.intn(len(alphabet))])
		}
		in = append(in, b.String())
	}
	return in
}

func intInputs() []string {
	in := []string{
		"", "0", "-0", "+0", "00", "007", "-007", "+", "-", "++1", "+-1", "-+1", "1-", " 1", "1 ", "1_000",
		"0x10", "0b1", "0o7", "1e3", "1.0", "a", "\u0661", "\uff11", "9", "10", "99", "-99",
		"127", "128", "-128", "-129", "255", "256", "32767", "32768", "-32768", "-32769", "65535", "65536",
		"2147483647", "2147483648", "-2147483648", "-2147483649", "4294967295", "4294967296",
		"9223372036854775807", "9223372036854775808", "-9223372036854775808", "-9223372036854775809",
		"18446744073709551615", "18446744073709551616", "18446744073709551617", "184467440737095516150",
		"99999999999999999999", "-99999999999999999999", "0000000000000000000000000000127",
		"-0000000000000000000000000000128", "123456789012345678", "1234567890123456789",
		"-123456789012345678", "-1234567890123456789", "12345678901234567890", "999999999999999999",
		"-999999999999999999", "+999999999999999999", "+123456789012345678", "18446744073709551615x",
		"1x", "x1", "--", "+0x", "1\x00", "1\u00e9",
	}
	g := splitmix64(12)
	for i := 0; i < 400; i++ {
		var b strings.Builder
		n := g.intn(22)
		const alphabet = "01234567890123456789+-_ x"
		for j := 0; j < n; j++ {
			b.WriteByte(alphabet[g.intn(len(alphabet))])
		}
		in = append(in, b.String())
	}
	for i := 0; i < 150; i++ {
		v := int64(g.next())
		bits := uint(1 + g.intn(64))
		if bits < 64 {
			v >>= 64 - bits
		}
		in = append(in, strconv.FormatInt(v, 10), strconv.FormatUint(uint64(v), 10))
	}
	return in
}

func intCase(s string) IntCase {
	c := IntCase{In: s}
	v8, err := util.ParseInt[int8](s)
	c.I8 = result(uint64(int64(v8)), err)
	v16, err := util.ParseInt[int16](s)
	c.I16 = result(uint64(int64(v16)), err)
	v32, err := util.ParseInt[int32](s)
	c.I32 = result(uint64(int64(v32)), err)
	v64, err := util.ParseInt[int64](s)
	c.I64 = result(uint64(v64), err)
	u8, err := util.ParseUInt[uint8](s)
	c.U8 = result(uint64(u8), err)
	u16, err := util.ParseUInt[uint16](s)
	c.U16 = result(uint64(u16), err)
	u32, err := util.ParseUInt[uint32](s)
	c.U32 = result(uint64(u32), err)
	u64, err := util.ParseUInt[uint64](s)
	c.U64 = result(u64, err)
	a, err := strconv.Atoi(s)
	c.Atoi = result(uint64(a), err)
	i0, err := strconv.ParseInt(s, 10, 0)
	c.Int0 = result(uint64(i0), err)
	u0, err := strconv.ParseUint(s, 10, 0)
	c.Uint0 = result(u0, err)
	return c
}

func quoteInputs() []string {
	in := []string{
		"", "abc", `"`, `\`, `a"b\c`, "'", "`", "\a\b\f\n\r\t\v", "\x00\x01\x1f\x7f", " ", "~",
		"\u0080\u0085\u009f\u00a0\u00a1\u00ad\u00ff", "\u0100", "\u00e9", "\u65e5\u672c\u8a9e", "\u200b\u200c\u200d\u200e",
		"\u2028\u2029", "\ufeff", "\ufffd", "\uffff", "\U0001f600", "\U0001f1fa\U0001f1f8", "a\u0301",
		"\U000e0001", "\U000e0100", "\U000f0000", "\U0010ffff", "\U0001d173", "\u061c", "\u3000",
		"\u1680", "\u180e", "tab\there", "line\nbreak",
	}
	// Every Latin-1 code point, and the code points at both edges of every
	// IsPrint range, 64 to a string.
	var runes []rune
	for r := rune(0); r < 0x100; r++ {
		runes = append(runes, r)
	}
	for _, r := range isPrintRanges() {
		for _, x := range []rune{r[0] - 1, r[0], r[1], r[1] + 1} {
			if x >= 0x100 && x <= utf8.MaxRune && (x < 0xd800 || x > 0xdfff) {
				runes = append(runes, x)
			}
		}
	}
	for len(runes) > 0 {
		n := min(64, len(runes))
		in = append(in, string(runes[:n]))
		runes = runes[n:]
	}
	return in
}

// isPrintRanges returns the inclusive ranges of code points IsPrint accepts.
func isPrintRanges() [][2]rune {
	var ranges [][2]rune
	for r := rune(0); r <= utf8.MaxRune; r++ {
		if !strconv.IsPrint(r) {
			continue
		}
		if len(ranges) > 0 && ranges[len(ranges)-1][1] == r-1 {
			ranges[len(ranges)-1][1] = r
		} else {
			ranges = append(ranges, [2]rune{r, r})
		}
	}
	return ranges
}

func main() {
	out := flag.String("out", "", "write the golden JSON to this file")
	flag.Parse()
	if *out == "" {
		log.Fatal("-out is required")
	}
	g := Golden{GoVersion: runtime.Version()}
	for _, s := range floatInputs() {
		f32, err32 := util.ParseFloat[float32](s)
		f64, err64 := util.ParseFloat[float64](s)
		g.Floats = append(g.Floats, FloatCase{s, result(uint64(math.Float32bits(f32)), err32), result(math.Float64bits(f64), err64)})
	}
	for _, s := range intInputs() {
		g.Ints = append(g.Ints, intCase(s))
	}
	for _, v := range []int64{0, 1, -1, 7, -42, 127, -128, 32767, -32768, 2147483647, -2147483648, math.MaxInt64, math.MinInt64} {
		g.FormatInts = append(g.FormatInts, FormatCase{v, util.FormatInt(v)})
	}
	for _, s := range quoteInputs() {
		g.Quotes = append(g.Quotes, QuoteCase{s, strconv.Quote(s)})
	}
	for _, r := range isPrintRanges() {
		for _, x := range []rune{r[0] - 1, r[0], r[1], r[1] + 1} {
			if x >= 0 && x <= utf8.MaxRune && (x < 0xd800 || x > 0xdfff) {
				g.IsPrint = append(g.IsPrint, IsPrintCase{x, strconv.IsPrint(x)})
			}
		}
	}
	for _, s := range []string{"", " ", "\t\n\v\f\r ", "\u0085", "\u00a0", "\u1680", "\u2000\u200a", "\u2028",
		"\u2029", "\u202f", "\u205f", "\u3000", "\u180e", "\u200b", "\ufeff", "/", " / ", "a/b", "abc", " abc ",
		"a,b", "\u00a0a\u00a0", "\u65e5\u672c", "\x00", "\u0085/"} {
		e := ""
		if err := util.ValidateId(s); err != nil {
			e = err.Error()
		}
		g.ValidateID = append(g.ValidateID, ErrCase{s, e})
	}
	for _, in := range [][]string{{}, {""}, {"a"}, {"latest"}, {"b", "a", "c"}, {"c", "b", "a"}, {"ab", "a", "b"},
		{"a", "ab", "b"}, {"B", "a", "A", "b"}, {"\u00e9", "e", "z"}, {"x", "x"}, {"\u65e5\u672c", "\u8a9e"}} {
		args := append([]string(nil), in...)
		g.MD5 = append(g.MD5, MD5Case{in, util.MD5(args...)})
	}
	data, err := json.MarshalIndent(g, "", " ")
	if err != nil {
		log.Fatal(err)
	}
	if err = os.WriteFile(*out, append(data, '\n'), 0o644); err != nil {
		log.Fatal(err)
	}
}
