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

// Command urlgolden writes what Go's net/url (and the net/netip.ParseAddr
// it calls) do, for the Rust port in crates/gorse-common/src/gourl.rs
// (WP-1-14), to gourl_golden.json, with the Go version that wrote it:
//
//   - parse: Parse and ParseRequestURI of each input, with every field of
//     the URL and the results of String, Redacted, RequestURI, EscapedPath,
//     EscapedFragment, Hostname, Port, IsAbs, Query, Query().Encode and
//     User.String, or the error text.
//   - escape: QueryEscape, PathEscape, QueryUnescape and PathUnescape of
//     each input.
//   - query: ParseQuery of each query input, with Encode of the result.
//   - methods: the same URL methods on URLs built field by field, which
//     reach states parsing does not (a RawPath that does not match Path, a
//     host with bytes Parse rejects, OmitHost with a "//" path).
//   - parse_addr: netip.ParseAddr, which parseHost calls for a bracketed
//     host.
//   - godebug_parse and query_limits: Parse under GODEBUG=urlstrictcolons
//     values and ParseQuery under GODEBUG=urlmaxqueryparams values, set with
//     os.Setenv, which Go's internal/godebug picks up.
//
// The inputs are every string literal of $GOROOT/src/net/url/url_test.go
// and the examples of example_test.go, some URLs of the kinds Gorse parses,
// and a corpus mutated from them with a fixed seed over an alphabet of the
// bytes the parser treats specially (%, [, ], @, :, ?, #, ;, +, space,
// control and non-ASCII bytes), so the output is the same on every run.
//
// A string that is valid UTF-8 is written as a JSON string; any other is
// written as {"hex": "..."}, so that every byte is kept. Empty URL fields
// and method results, false flags and empty query lists are left out.
//
// Run it with GODEBUG unset. Regenerate (from the repository root):
//
//	go run ./contracts/tools/urlgolden \
//	  -out crates/gorse-common/tests/data/gourl_golden.json
package main

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"math/rand/v2"
	"net/netip"
	"net/url"
	"os"
	"reflect"
	"runtime"
	"sort"
	"strings"
	"unicode/utf8"
)

// testSeeds are the string literals of $GOROOT/src/net/url/url_test.go
// (go1.27.0), in the order they appear, without duplicates and without the
// format strings of test messages.
var testSeeds = []string{
	"in",
	"http://www.google.com",
	"http",
	"www.google.com",
	"",
	"http://www.google.com/",
	"/",
	"http://www.google.com/file%20one%26two",
	"/file one&two",
	"/file%20one%26two",
	"http://www.google.com/#file%20one%26two",
	"file one&two",
	"file%20one%26two",
	"ftp://webmaster@www.google.com/",
	"ftp",
	"webmaster",
	"ftp://john%20doe@www.google.com/",
	"john doe",
	"http://www.google.com/?",
	"http://www.google.com/?foo=bar?",
	"foo=bar?",
	"http://www.google.com/?q=go+language",
	"q=go+language",
	"http://www.google.com/?q=go%20language",
	"q=go%20language",
	"http://www.google.com/a%20b?q=c+d",
	"/a b",
	"q=c+d",
	"http:www.google.com/?q=go+language",
	"www.google.com/",
	"http:%2f%2fwww.google.com/?q=go+language",
	"%2f%2fwww.google.com/",
	"mailto:/webmaster@golang.org",
	"mailto",
	"/webmaster@golang.org",
	"mailto:webmaster@golang.org",
	"webmaster@golang.org",
	"/foo?query=http://bad",
	"/foo",
	"query=http://bad",
	"//foo",
	"foo",
	"//user@foo/path?a=b",
	"user",
	"/path",
	"a=b",
	"///threeslashes",
	"http://user:password@google.com",
	"password",
	"google.com",
	"http://j@ne:password@google.com",
	"j@ne",
	"http://j%40ne:password@google.com",
	"http://jane:p@ssword@google.com",
	"jane",
	"p@ssword",
	"http://jane:p%40ssword@google.com",
	"http://j@ne:password@google.com/p@th?q=@go",
	"/p@th",
	"q=@go",
	"http://j%40ne:password@google.com/p@th?q=@go",
	"http://www.google.com/?q=go+language#foo",
	"http://www.google.com/?q=go+language#foo&bar",
	"foo&bar",
	"http://www.google.com/?q=go+language#foo%26bar",
	"foo%26bar",
	"file:///home/adg/rabbits",
	"file",
	"/home/adg/rabbits",
	"Windows",
	"file:///C:/FooBar/Baz.txt",
	"/C:/FooBar/Baz.txt",
	"MaIlTo:webmaster@golang.org",
	"a/b/c",
	"http://%3Fam:pa%3Fsword@google.com",
	"?am",
	"pa?sword",
	"http://192.168.0.1/",
	"192.168.0.1",
	"http://192.168.0.1:8080/",
	"192.168.0.1:8080",
	"http://[fe80::1]/",
	"[fe80::1]",
	"http://[fe80::1]:8080/",
	"[fe80::1]:8080",
	"https://[2001:db8::1]:8443/test/path",
	"https",
	"[2001:db8::1]:8443",
	"/test/path",
	"http://[fe80::1%25en0]/",
	"[fe80::1%en0]",
	"http://[fe80::1%25en0]:8080/",
	"[fe80::1%en0]:8080",
	"http://[fe80::1%25%65%6e%301-._~]/",
	"[fe80::1%en01-._~]",
	"http://[fe80::1%25en01-._~]/",
	"http://[fe80::1%25%65%6e%301-._~]:8080/",
	"[fe80::1%en01-._~]:8080",
	"http://[fe80::1%25en01-._~]:8080/",
	"http://rest.rsc.io/foo%2fbar/baz%2Fquux?alt=media",
	"rest.rsc.io",
	"/foo/bar/baz/quux",
	"/foo%2fbar/baz%2Fquux",
	"alt=media",
	"mysql://a,b,c/bar",
	"mysql",
	"a,b,c",
	"/bar",
	"scheme://!$&'()*+,;=hello!:1/path",
	"scheme",
	"!$&'()*+,;=hello!:1",
	"http://host/!$&'()*+,;=:@[hello]",
	"host",
	"/!$&'()*+,;=:@[hello]",
	"http://example.com/oid/[order_id]",
	"example.com",
	"/oid/[order_id]",
	"http://192.168.0.2:8080/foo",
	"192.168.0.2:8080",
	"http://192.168.0.2:/foo",
	"192.168.0.2:",
	"http://[2b01:e34:ef40:7730:8e70:5aff:fefe:edac]:8080/foo",
	"[2b01:e34:ef40:7730:8e70:5aff:fefe:edac]:8080",
	"http://[2b01:e34:ef40:7730:8e70:5aff:fefe:edac]:/foo",
	"[2b01:e34:ef40:7730:8e70:5aff:fefe:edac]:",
	"http://hello.世界.com/foo",
	"hello.世界.com",
	"http://hello.%E4%B8%96%E7%95%8C.com/foo",
	"http://hello.%e4%b8%96%e7%95%8c.com/foo",
	"http://example.com//foo",
	"myscheme://authority<\"hi\">/foo",
	"myscheme",
	"authority<\"hi\">",
	"tcp://[2020::2020:20:2020:2020%25Windows%20Loves%20Spaces]:2020",
	"tcp",
	"[2020::2020:20:2020:2020%Windows Loves Spaces]:2020",
	"magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn",
	"magnet",
	"xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn",
	"mailto:?subject=hi",
	"subject=hi",
	"postgres://host1:1,host2:2,host3:3",
	"postgres",
	"host1:1,host2:2,host3:3",
	"postgresql://host1:1,host2:2,host3:3",
	"postgresql",
	"mongodb://user:password@host1:1,host2:2,host3:3",
	"mongodb",
	"mongodb+srv://user:password@host1:1,host2:2,host3:3",
	"mongodb+srv",
	"//host/path",
	"http:%2F/host/path",
	"//not.a.user@not.a.host/just/a/path",
	"http://foo.com",
	"http://foo.com/",
	"http://foo.com/path",
	"//not.a.user@%66%6f%6f.com/just/a/path/also",
	"*",
	"foo.html",
	"../dir/",
	" http://foo.com",
	"http://192.168.0.%31/",
	"http://192.168.0.%31:8080/",
	"http://[fe80::%31]/",
	"http://[fe80::%31]:8080/",
	"http://[fe80::%31%25en0]/",
	"http://[fe80::%31%25en0]:8080/",
	"http://[fe80::1%en0]/",
	"http://[fe80::1%en0]:8080/",
	"https://[1:2:3:4:5:6:7:8]",
	"https://[2001:db8::a:b:c:d]",
	"https://[fe80::1%25eth0]",
	"https://[fe80::abc:def%254]",
	"https://[2001:db8::1]/path",
	"https://[fe80::1%25eth0]/path?query=1",
	"https://[::ffff:192.0.2.1]",
	"https://[:1] ",
	"https://[1:2:3:4:5:6:7:8:9]",
	"https://[1::1::1]",
	"https://[1:2:3:]",
	"https://[ffff::127.0.0.4000]",
	"https://[0:0::test.com]:80",
	"https://[2001:db8::test.com]",
	"https://[test.com]",
	"https://1:2:3:4:5:6:7:8",
	"https://1:2:3:4:5:6:7:8:80",
	"https://example.com:80:",
	"search",
	"http://www.google.com/search",
	":",
	"./",
	"this:that",
	"./this:that",
	"here/this:that",
	"http://www.google.com/this:that",
	"non-blank Password",
	"host.tld",
	"http://user:xxxxx@host.tld/this:that",
	"blank Password",
	"http://user@host.tld/this:that",
	"nil User",
	"http://:xxxxx@host.tld/this:that",
	"blank Username, blank Password",
	"http://host.tld/this:that",
	"empty URL",
	"nil URL",
	"abc",
	"1%41",
	"1A",
	"1%41%42%43",
	"1ABC",
	"%4a",
	"J",
	"%6F",
	"o",
	"%",
	"%a",
	"%1",
	"123%45%6",
	"%6",
	"%zzzzz",
	"%zz",
	"a+b",
	"a b",
	"a%20b",
	"+",
	"%20",
	"XXX",
	"one two",
	"one+two",
	"10%",
	"10%25",
	" ?&=#+%!<>#\"{}|\\^[]`☺\t:/@$'()*,;",
	"+%3F%26%3D%23%2B%25%21%3C%3E%23%22%7B%7D%7C%5C%5E%5B%5D%60%E2%98%BA%09%3A%2F%40%24%27%28%29%2A%2C%3B",
	"abc+def",
	"a/b",
	"a%2Fb",
	"one%20two",
	"%20%3F&=%23+%25%21%3C%3E%23%22%7B%7D%7C%5C%5E%5B%5D%60%E2%98%BA%09:%2F@$%27%28%29%2A%2C%3B",
	"user:password",
	"foo:bar",
	"~!@#$%^&*()_+{}|[]\\-=`:;'\"<>?,./",
	"foo%3Abar:~!%40%23$%25%5E&*()_+%7B%7D%7C%5B%5D%5C-=%60%3A;'%22%3C%3E?,.%2F",
	"q",
	"puppies",
	"oe",
	"utf8",
	"oe=utf8&q=puppies",
	"dogs",
	"&",
	"7",
	"q=dogs&q=%26&q=7",
	"a",
	"a1",
	"a2",
	"a3",
	"b",
	"b1",
	"b2",
	"b3",
	"c",
	"c1",
	"c2",
	"c3",
	"a=a1&a=a2&a=a3&b=b1&b=b2&b=b3&c=c1&c=c2&c=c3",
	"d",
	"e",
	"f",
	"g",
	"h",
	"i",
	"a=a&b=b&c=c&d=d&e=e&f=f&g=g&h=h&i=i",
	".",
	"/a/",
	"/a/c",
	"..",
	"a/",
	"../..",
	"../d",
	"/a/d",
	".././d",
	"./..",
	"a/./b",
	"a/../",
	"a/.././b",
	"/c",
	"../",
	"http://foo.com?a=b",
	"https://bar.com/",
	"https://bar.com/?a=b",
	"https://bar.com/?",
	"http://foo.com/bar",
	"mailto:foo@example.com",
	"/baz",
	"http://foo.com/baz",
	"http://foo.com/bar?a=b#f",
	"http://foo.com/bar?a=b",
	"/baz?",
	"http://foo.com/baz?",
	"/baz?c=d",
	"http://foo.com/baz?c=d",
	"http://foo.com//baz",
	"http://foo.com///baz/quux",
	"https://foo.com/bar?a=b",
	"//bar.com/quux",
	"https://bar.com/quux",
	"http://foo.com/bar/",
	"bar",
	"http://foo.com/bar/baz",
	"quux",
	"http://foo.com/bar/quux",
	"http://foo.com/bar/baz/",
	"http://foo.com/bar/baz/quux",
	"../quux",
	"http://foo.com/quux",
	"../../../../../quux",
	"quux/dotdot/../tail",
	"http://foo.com/bar/quux/tail",
	"quux/./dotdot/../tail",
	"quux/./dotdot/.././tail",
	"quux/./dotdot/./../tail",
	"quux/./dotdot/dotdot/././../../tail",
	"quux/./dotdot/dotdot/./.././../tail",
	"quux/./dotdot/dotdot/dotdot/./../../.././././tail",
	"quux/./dotdot/../dotdot/../dot/./tail/..",
	"http://foo.com/bar/quux/dot/",
	"http://foo.com/dot/./dotdot/../foo/bar",
	"../baz",
	"http://foo.com/dot/baz",
	"...",
	"http://foo.com/...",
	".#frag",
	"http://foo.com/#frag",
	"http://example.org/",
	"#!$&%27()*+,;=",
	"http://example.org/#!$&%27()*+,;=",
	"http://foo.com/foo%2fbar/",
	"http://foo.com/1/2%2f/3%2f4/5",
	"../../a/b/c",
	"http://foo.com/1/a/b/c",
	"http://foo.com/1/2/3",
	"./a%2f../../b/..%2fc",
	"http://foo.com/1/2/b/..%2fc",
	"./a%2f../b/../c",
	"http://foo.com/1/2%2f/3%2f4/a%2f../c",
	"http://foo.com/foo%20bar/",
	"http://foo.com/foo",
	"../bar%2fbaz",
	"http://foo.com/bar%2fbaz",
	"http://foo.com/foo%2dbar/",
	"./baz-quux",
	"http://foo.com/foo%2dbar/baz-quux",
	"http://a/b/c/d;p?q",
	"g:h",
	"http://a/b/c/g",
	"./g",
	"g/",
	"http://a/b/c/g/",
	"/g",
	"http://a/g",
	"//g",
	"http://g",
	"?y",
	"http://a/b/c/d;p?y",
	"g?y",
	"http://a/b/c/g?y",
	"#s",
	"http://a/b/c/d;p?q#s",
	"g#s",
	"http://a/b/c/g#s",
	"g?y#s",
	"http://a/b/c/g?y#s",
	";x",
	"http://a/b/c/;x",
	"g;x",
	"http://a/b/c/g;x",
	"g;x?y#s",
	"http://a/b/c/g;x?y#s",
	"http://a/b/c/",
	"http://a/b/",
	"../g",
	"http://a/b/g",
	"http://a/",
	"../../",
	"../../g",
	"../../../g",
	"../../../../g",
	"/./g",
	"/../g",
	"g.",
	"http://a/b/c/g.",
	".g",
	"http://a/b/c/.g",
	"g..",
	"http://a/b/c/g..",
	"..g",
	"http://a/b/c/..g",
	"./../g",
	"./g/.",
	"g/./h",
	"http://a/b/c/g/h",
	"g/../h",
	"http://a/b/c/h",
	"g;x=1/./y",
	"http://a/b/c/g;x=1/y",
	"g;x=1/../y",
	"http://a/b/c/y",
	"g?y/./x",
	"http://a/b/c/g?y/./x",
	"g?y/../x",
	"http://a/b/c/g?y/../x",
	"g#s/./x",
	"http://a/b/c/g#s/./x",
	"g#s/../x",
	"http://a/b/c/g#s/../x",
	"https://a/b/c/d;p?q",
	"//g?q",
	"https://g?q",
	"//g#s",
	"https://g#s",
	"//g/d/e/f?y#s",
	"https://g/d/e/f?y#s",
	"https://a/b/c/d;p#s",
	"https://a/b/c/d;p?y",
	"https://a/b/c/d;p?q#s",
	"?",
	"https://a/b/c/d;p?",
	"http:opaque",
	"http:opaque?x=y#zzz",
	"https:/foo?a=b#frag",
	"https:foo:bar",
	"https:bar/baz?a=b#frag",
	"https://user@host:1234?a=b#frag",
	"?a=b#frag",
	"http:opaque?a=b#frag",
	"opaque",
	"scheme:opaque",
	"http://x.com?foo=bar&bar=1&bar=2&baz",
	"Foo",
	"1",
	"baz",
	"noexist",
	"a=1",
	"a=1&b=2",
	"2",
	"a=1&a=2&a=banana",
	"banana",
	"ascii=%3Ckey%3A+0x90%3E",
	"ascii",
	"<key: 0x90>",
	"a=1;b=2",
	"a;b=1",
	"a=%3B",
	";",
	"a%3Bb=1",
	"a;b",
	"a=1&a=2;a=banana",
	"a;b&c=1",
	"a=1&b=2;a=3&c=4",
	"4",
	"a=1&b=2;c=3",
	"a=1;",
	"a=1&;",
	";a=1&b=2",
	"a=1&b=2;",
	"<error>",
	"<nil>",
	"urlmaxqueryparams=9",
	"urlmaxqueryparams=0",
	"GODEBUG",
	"/a%20b",
	"/%2F/%2F/",
	"//other.example.com/%2F/%2F/",
	"http://other.example.com/%2F/%2F/",
	"/////",
	"/WRONG/",
	"/a%20b?q=go+language",
	"/a?b",
	"/a%3Fb?q=go+language",
	"myschema",
	"opaque?q=go+language",
	"/foo?",
	"%gh&%ij",
	"%gh",
	"http://[::1]",
	"http://[::1]:80",
	"http://[::1]:namedport",
	"http://x:namedport",
	"http://[::1]/",
	"http://[::1]a",
	"http://[::1]%23",
	"http://[::1%25en0]",
	"http://[::1]:",
	"http://x:",
	"http://[::1]:%38%30",
	"http://[::1%25%41]",
	"http://[%10::1]",
	"http://[::1]/%48",
	"http://%41:8080/",
	"mysql://x@y(z:123)/foo",
	"mysql://x@y(1.2.3.4:123)/foo",
	"ht tp://foo.com",
	"ahttp://foo.com",
	"1http://foo.com",
	"http://[]%20%48%54%54%50%2f%31%2e%31%0a%4d%79%48%65%61%64%65%72%3a%20%31%32%33%0a%0a/",
	"http://a b.com/",
	"cache_object://foo",
	"cache_object:foo",
	"cache_object:foo/bar",
	"cache_object/:foo/bar",
	"http://[192.168.0.1]/",
	"http://[192.168.0.1]:8080/",
	"http://[::ffff:192.168.0.1]/",
	"http://[::ffff:192.168.0.1000]/",
	"http://[::ffff:192.168.0.1]:8080/",
	"http://[::ffff:c0a8:1]/",
	"http://[not-an-ip]/",
	"http://[fe80::1%foo]/",
	"http://[fe80::1",
	"http://fe80::1]/",
	"http://[test.com]/",
	"http://example.com[::1]",
	"http://example.com[::1",
	"http://[::1",
	"http://.[::1]",
	"http:// [::1]",
	"hxxp://mathepqo[.]serveftp(.)com:9059",
	"timeout error",
	"temporary error",
	"timeout/temporary error",
	"Get",
	"http://google.com/",
	"foo.com:80",
	"foo.com",
	"80",
	"foo.com:",
	"FOO.COM",
	"1.2.3.4",
	"1.2.3.4:80",
	"[1:2:3:4]",
	"1:2:3:4",
	"[1:2:3:4]:80",
	"[::1]:80",
	"::1",
	"[::1]",
	"[::1]:",
	"localhost",
	"localhost:443",
	"443",
	"some.super.long.domain.example.org:8080",
	"some.super.long.domain.example.org",
	"8080",
	"[2001:0db8:85a3:0000:0000:8a2e:0370:7334]:17000",
	"2001:0db8:85a3:0000:0000:8a2e:0370:7334",
	"17000",
	"[2001:0db8:85a3:0000:0000:8a2e:0370:7334]",
	"Hostname",
	"Hostname:Port",
	"[Hostname]",
	"[Hostname]:Port",
	"[google.com]:80",
	"google.com]:80",
	"google.com]",
	"google.com:80_invalid_port",
	"[::1]extra]:80",
	"::1]extra",
	"google.com]extra:extra",
	"https://www.google.com/x?y=z",
	"http://user^:passwo^rd@foo.com/",
	"http://foo.com/?foo\nbar",
	"http\r://foo.com/",
	"http://foo\x7f.com/",
	"net/url: invalid control character in URL",
	"http://foo.com/ctl\x80",
	"Фотки собак",
	"%D0%A4%D0%BE%D1%82%D0%BA%D0%B8+%D1%81%D0%BE%D0%B1%D0%B0%D0%BA",
	"%D0%A4%D0%BE%D1%82%D0%BA%D0%B8%20%D1%81%D0%BE%D0%B1%D0%B0%D0%BA",
	"shortrun(break)shortrun",
	"shortrun%28break%29shortrun",
	"longerrunofcharacters(break)anotherlongerrunofcharacters",
	"longerrunofcharacters%28break%29anotherlongerrunofcharacters",
	"padded/with+various%characters?that=need$some@escaping+paddedsowebreak/256bytes",
	"padded%2Fwith%2Bvarious%25characters%3Fthat%3Dneed%24some%40escaping%2Bpaddedsowebreak%2F256bytes",
	"padded%2Fwith+various%25characters%3Fthat=need$some@escaping+paddedsowebreak%2F256bytes",
	"https://go.googlesource.com",
	"go",
	"https://go.googlesource.com/go",
	"https://go.googlesource.com/a/b/c",
	"../../../go",
	"https://go.googlesource.com/",
	"../go",
	"../../go",
	"https://go.googlesource.com/../go",
	"./go",
	"https://go.googlesource.com//",
	"/go",
	"https://go.googlesource.com/go/a/b/c",
	"go/",
	"https://go.googlesource.com/go/",
	"go//",
	"https://go.googlesource.com/a%2fb",
	"https://go.googlesource.com/a%2fb/c",
	"c%2fd",
	"https://go.googlesource.com/a%2fb/c%2fd",
	"https://go.googlesource.com/a/b",
	"https://go.googlesource.com/a/b/go",
	"100%",
	"../b",
	"../../b",
	"../a",
	"nil",
	"non-nil error",
	"urlstrictcolons=0",
	"zero value",
	"Populated but nil .User",
	"non-nil .User",
	"non-nil .User: user and password set",
	"original",
	"cloned",
	"empty",
	"1 key, nil values",
	"1 key, no values",
	"1 key, some values",
	"multiple keys, diverse values",
	"X",
	"B",
	"abcdefghi",
	"XXXXXXXXXXX",
	"A",
	"golang",
	"directly modified",
	"good night",
}

// exampleSeeds are the inputs of $GOROOT/src/net/url/example_test.go.
var exampleSeeds = []string{
	"my/cool+blog&about,stuff",
	"my%2Fcool+blog&about%2Cstuff",
	"my%2Fcool%2Bblog%26about%2Cstuff",
	"http://bing.com/search?q=dotnet",
	"https://example.com/foo%2fbar",
	"../../..//search?q=dotnet",
	"http://example.com/directory/",
	"x=1&y=2&y=3",
	"http://example.com/x/y%2Fz",
	"http://example.com/#x/y%2Fz",
	"https://example.org:8000/path",
	"https://[2001:0db8:85a3:0000:0000:8a2e:0370:7334]:17000",
	"https://example.com/foo/bar",
	"https://example.org",
	"https://example.org:8080",
	"https://example.org/?a=1&a=2&b=&=3&&&&",
	"https://example.org/path?foo=bar",
	"cat sounds",
	"mew/",
	"mau$",
}

// gorseSeeds are URLs of the kinds Gorse's configuration and storage layer
// parse.
var gorseSeeds = []string{
	"redis://localhost:6379/0",
	"redis://:password@127.0.0.1:6379/1?pool_size=10",
	"rediss://user:p%40ss@redis.example.com:6380/0",
	"redis+cluster://host1:6379,host2:6379/0",
	"mysql://gorse:gorse_pass@tcp(localhost:3306)/gorse?parseTime=true",
	"mysql://root@unix(/var/run/mysqld/mysqld.sock)/gorse",
	"postgres://gorse:gorse_pass@localhost:5432/gorse?sslmode=disable",
	"postgresql://user@host1:5432,host2:5433/db?target_session_attrs=read-write",
	"mongodb://root:password@127.0.0.1:27017/",
	"mongodb+srv://user:pw@cluster0.example.net/gorse?retryWrites=true&w=majority",
	"clickhouse://default:@localhost:9000/default?dial_timeout=10s",
	"chhttp://localhost:8123/default",
	"sqlite:///var/lib/gorse/data.db",
	"sqlite://gorse.db?_pragma=busy_timeout(5000)",
	"sqlite://:memory:",
	"file:///tmp/gorse/cache",
	"qdrant://localhost:6334/collection",
	"milvus://user:pass@[::1]:19530/db",
	"s3://bucket/prefix/model.bin?region=us-east-1",
	"gs://bucket/path%2Fwith%2Fslashes",
	"azblob://container/blob?account=acct",
	"http://[fe80::1%25eth0]:8088/api/v1/items?n=10&offset=20",
	"https://gorse.example.com/api/dashboard/stats?begin=2026-01-01T00%3A00%3A00Z",
	"http://127.0.0.1:8087/api/recommend/user%201?category=a%2Fb&n=10",
}

// alphabet are the bytes and byte sequences mutations insert.
var alphabet = []string{
	"%", "[", "]", "@", ":", "?", "#", ";", "+", " ", "/", ".", "&", "=",
	"-", "_", "~", "!", "$", "'", "(", ")", "*", ",", "<", ">", "\"", "\\",
	"{", "}", "|", "^", "`",
	"\x00", "\t", "\n", "\r", "\x1f", "\x7f", "\x80", "\xff", "\xc3", "\xa9",
	"é", "世", "☺",
	"a", "z", "A", "Z", "0", "1", "9", "f", "F", "g", "x",
	"%25", "%2F", "%2f", "%20", "%3A", "%40", "%5B", "%5D", "%zz", "%4", "%e4%b8%96",
	"%00", "%7f", "%80", "%ff", "%41", "%3B",
	"::", "[::1]", "[fe80::1%25en0]", "%25en0", "1.2.3.4", ":80", ":", "//", "///",
	"http://", "https://", "mailto:", "user:pass@", "@host", "?q=1", "#frag",
}

func main() {
	out := flag.String("out", "", "write the net/url golden JSON here")
	flag.Parse()
	if *out == "" {
		fail(fmt.Errorf("-out is required"))
	}
	if v, ok := os.LookupEnv("GODEBUG"); ok {
		fail(fmt.Errorf("run with GODEBUG unset, not %q", v))
	}
	must(writeJSON(*out, build()))
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

// bs is a Go string that may not be valid UTF-8.
type bs string

// MarshalJSON writes s as a JSON string if it is valid UTF-8, else as
// {"hex": "..."}.
func (s bs) MarshalJSON() ([]byte, error) {
	if utf8.ValidString(string(s)) {
		return marshal(string(s))
	}
	return marshal(struct {
		Hex string `json:"hex"`
	}{hex.EncodeToString([]byte(s))})
}

// marshal is json.Marshal without HTML escaping.
func marshal(v any) ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return nil, err
	}
	return bytes.TrimSuffix(buf.Bytes(), []byte("\n")), nil
}

type golden struct {
	GoVersion    string        `json:"go_version"`
	Parse        []parseCase   `json:"parse"`
	Escape       []escapeCase  `json:"escape"`
	Query        []queryCase   `json:"query"`
	Methods      []methodsCase `json:"methods"`
	ParseAddr    []addrCase    `json:"parse_addr"`
	GodebugParse []parseCase   `json:"godebug_parse"`
	QueryLimits  []limitCase   `json:"query_limits"`
}

type userinfo struct {
	Username    bs   `json:"username"`
	Password    bs   `json:"password"`
	PasswordSet bool `json:"password_set"`
}

type urlFields struct {
	Scheme      bs        `json:"scheme,omitempty"`
	Opaque      bs        `json:"opaque,omitempty"`
	User        *userinfo `json:"user,omitempty"`
	Host        bs        `json:"host,omitempty"`
	Path        bs        `json:"path,omitempty"`
	RawPath     bs        `json:"raw_path,omitempty"`
	ForceQuery  bool      `json:"force_query,omitempty"`
	RawQuery    bs        `json:"raw_query,omitempty"`
	Fragment    bs        `json:"fragment,omitempty"`
	RawFragment bs        `json:"raw_fragment,omitempty"`
	OmitHost    bool      `json:"omit_host,omitempty"`
}

// keyValues is one key of a url.Values with its values.
type keyValues struct {
	Key    bs   `json:"key"`
	Values []bs `json:"values"`
}

// methods are the results of the URL methods.
type methods struct {
	String          bs          `json:"string,omitempty"`
	Redacted        bs          `json:"redacted,omitempty"`
	RequestURI      bs          `json:"request_uri,omitempty"`
	EscapedPath     bs          `json:"escaped_path,omitempty"`
	EscapedFragment bs          `json:"escaped_fragment,omitempty"`
	Hostname        bs          `json:"hostname,omitempty"`
	Port            bs          `json:"port,omitempty"`
	IsAbs           bool        `json:"is_abs,omitempty"`
	Query           []keyValues `json:"query,omitempty"`
	QueryEncode     bs          `json:"query_encode,omitempty"`
	UserString      bs          `json:"user_string,omitempty"`
}

// parsed is a Parse or ParseRequestURI result: Err, or the URL and its
// methods.
type parsed struct {
	Err     string     `json:"err,omitempty"`
	URL     *urlFields `json:"url,omitempty"`
	Methods *methods   `json:"methods,omitempty"`
}

type parseCase struct {
	Godebug string `json:"godebug,omitempty"`
	In      bs     `json:"in"`
	Parse   parsed `json:"parse"`
	Request parsed `json:"request"`
}

type escapeCase struct {
	In              bs     `json:"in"`
	QueryEscape     bs     `json:"query_escape"`
	PathEscape      bs     `json:"path_escape"`
	QueryUnescape   bs     `json:"query_unescape"`
	QueryUnescapeEr string `json:"query_unescape_err"`
	PathUnescape    bs     `json:"path_unescape"`
	PathUnescapeErr string `json:"path_unescape_err"`
}

type queryCase struct {
	In     bs          `json:"in"`
	Values []keyValues `json:"values"`
	Err    string      `json:"err"`
	Encode bs          `json:"encode"`
}

type methodsCase struct {
	URL     urlFields `json:"url"`
	Methods methods   `json:"methods"`
}

type addrCase struct {
	In    bs     `json:"in"`
	Err   string `json:"err"`
	Is4   bool   `json:"is4"`
	Zone  bs     `json:"zone"`
	Bytes string `json:"bytes"`
}

// limitCase is ParseQuery of params keys "p0&p1&...", as
// TestParseQueryLimits builds them, under GODEBUG=Godebug.
type limitCase struct {
	Godebug string `json:"godebug"`
	Params  int    `json:"params"`
	Err     string `json:"err"`
	Len     int    `json:"len"`
}

func errText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

func toFields(u *url.URL) urlFields {
	f := urlFields{
		Scheme:      bs(u.Scheme),
		Opaque:      bs(u.Opaque),
		Host:        bs(u.Host),
		Path:        bs(u.Path),
		RawPath:     bs(u.RawPath),
		ForceQuery:  u.ForceQuery,
		RawQuery:    bs(u.RawQuery),
		Fragment:    bs(u.Fragment),
		RawFragment: bs(u.RawFragment),
		OmitHost:    u.OmitHost,
	}
	if u.User != nil {
		p, ok := u.User.Password()
		f.User = &userinfo{Username: bs(u.User.Username()), Password: bs(p), PasswordSet: ok}
	}
	return f
}

func fromFields(f urlFields) *url.URL {
	u := &url.URL{
		Scheme:      string(f.Scheme),
		Opaque:      string(f.Opaque),
		Host:        string(f.Host),
		Path:        string(f.Path),
		RawPath:     string(f.RawPath),
		ForceQuery:  f.ForceQuery,
		RawQuery:    string(f.RawQuery),
		Fragment:    string(f.Fragment),
		RawFragment: string(f.RawFragment),
		OmitHost:    f.OmitHost,
	}
	if f.User != nil {
		if f.User.PasswordSet {
			u.User = url.UserPassword(string(f.User.Username), string(f.User.Password))
		} else {
			u.User = url.User(string(f.User.Username))
		}
	}
	return u
}

// sortedValues lists v by key, in the byte order Values.Encode sorts in.
func sortedValues(v url.Values) []keyValues {
	keys := make([]string, 0, len(v))
	for k := range v {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	out := []keyValues{}
	for _, k := range keys {
		vs := []bs{}
		for _, x := range v[k] {
			vs = append(vs, bs(x))
		}
		out = append(out, keyValues{Key: bs(k), Values: vs})
	}
	return out
}

func urlMethods(u *url.URL) methods {
	q := u.Query()
	return methods{
		String:          bs(u.String()),
		Redacted:        bs(u.Redacted()),
		RequestURI:      bs(u.RequestURI()),
		EscapedPath:     bs(u.EscapedPath()),
		EscapedFragment: bs(u.EscapedFragment()),
		Hostname:        bs(u.Hostname()),
		Port:            bs(u.Port()),
		IsAbs:           u.IsAbs(),
		Query:           sortedValues(q),
		QueryEncode:     bs(q.Encode()),
		UserString:      bs(u.User.String()),
	}
}

func parseResult(u *url.URL, err error) parsed {
	if err != nil {
		return parsed{Err: err.Error()}
	}
	f := toFields(u)
	m := urlMethods(u)
	return parsed{URL: &f, Methods: &m}
}

func parseCaseOf(in, godebug string) parseCase {
	c := parseCase{Godebug: godebug, In: bs(in)}
	c.Parse = parseResult(url.Parse(in))
	c.Request = parseResult(url.ParseRequestURI(in))
	return c
}

// mutator derives inputs from seeds with a fixed-seed generator.
type mutator struct {
	r *rand.Rand
}

func (m *mutator) token() string {
	return alphabet[m.r.IntN(len(alphabet))]
}

// mutate applies one to four random edits to s, splicing in other from
// time to time.
func (m *mutator) mutate(s, other string) string {
	b := []byte(s)
	for range 1 + m.r.IntN(4) {
		pos := 0
		if len(b) > 0 {
			pos = m.r.IntN(len(b) + 1)
		}
		switch m.r.IntN(6) {
		case 0, 1: // insert a token
			b = append(b[:pos:pos], append([]byte(m.token()), b[pos:]...)...)
		case 2: // delete a range
			if len(b) > 0 {
				end := min(len(b), pos+1+m.r.IntN(4))
				b = append(b[:pos:pos], b[end:]...)
			}
		case 3: // replace a byte with a token
			if pos < len(b) {
				b = append(b[:pos:pos], append([]byte(m.token()), b[pos+1:]...)...)
			}
		case 4: // splice: a prefix of b and a suffix of other
			cut := 0
			if len(other) > 0 {
				cut = m.r.IntN(len(other) + 1)
			}
			b = append(b[:pos:pos], other[cut:]...)
		case 5: // duplicate a range
			if len(b) > 0 {
				end := min(len(b), pos+1+m.r.IntN(6))
				b = append(b[:end:end], b[pos:]...)
			}
		}
	}
	return string(b)
}

// corpus returns seeds followed by n mutations of them, without duplicates.
func corpus(m *mutator, seeds []string, n int) []string {
	seen := map[string]bool{}
	var out []string
	add := func(s string) {
		if !seen[s] {
			seen[s] = true
			out = append(out, s)
		}
	}
	for _, s := range seeds {
		add(s)
	}
	for len(out) < len(seeds)+n {
		s := seeds[m.r.IntN(len(seeds))]
		other := seeds[m.r.IntN(len(seeds))]
		add(m.mutate(s, other))
	}
	return out
}

// randomField is a URL field value: empty, a seed, a token run or a
// mutation.
func (m *mutator) randomField(seeds []string) string {
	switch m.r.IntN(5) {
	case 0:
		return ""
	case 1:
		return seeds[m.r.IntN(len(seeds))]
	case 2:
		var b strings.Builder
		for range 1 + m.r.IntN(5) {
			b.WriteString(m.token())
		}
		return b.String()
	default:
		return m.mutate(seeds[m.r.IntN(len(seeds))], seeds[m.r.IntN(len(seeds))])
	}
}

var schemes = []string{"", "", "http", "https", "mailto", "file", "redis", "mysql", "x+y.z-w"}

// randomURL builds a URL field by field.
func (m *mutator) randomURL(seeds []string) *url.URL {
	u := &url.URL{
		Scheme:     schemes[m.r.IntN(len(schemes))],
		Host:       m.randomField(seeds),
		Path:       m.randomField(seeds),
		ForceQuery: m.r.IntN(4) == 0,
		RawQuery:   m.randomField(seeds),
		Fragment:   m.randomField(seeds),
		OmitHost:   m.r.IntN(3) == 0,
	}
	if m.r.IntN(4) == 0 {
		u.Opaque = m.randomField(seeds)
	}
	if m.r.IntN(3) == 0 {
		u.Host = ""
	}
	if m.r.IntN(4) == 0 && len(u.Path) > 0 && u.Path[0] != '/' {
		u.Path = "//" + u.Path
	}
	switch m.r.IntN(3) {
	case 1:
		u.User = url.User(m.randomField(seeds))
	case 2:
		u.User = url.UserPassword(m.randomField(seeds), m.randomField(seeds))
	}
	// RawPath and RawFragment: empty, the input's own escaping (which
	// EscapedPath accepts), or something else (which it may not).
	switch m.r.IntN(3) {
	case 1:
		u.RawPath = strings.ReplaceAll(url.PathEscape(u.Path), "%2F", "%2f")
	case 2:
		u.RawPath = m.randomField(seeds)
	}
	switch m.r.IntN(3) {
	case 1:
		u.RawFragment = strings.ReplaceAll(url.PathEscape(u.Fragment), "%2F", "%2f")
	case 2:
		u.RawFragment = m.randomField(seeds)
	}
	return u
}

var addrSeeds = []string{
	"", "1.2.3.4", "0.0.0.0", "255.255.255.255", "192.168.0.1", "01.2.3.4",
	"1.2.3", "1.2.3.4.5", "1.2.3.256", "1..2.3", ".1.2.3", "1.2.3.", "1.2.3.4x",
	"::", "::1", "1::", "fe80::1", "fe80::1%en0", "fe80::1%25en0", "fe80::1%",
	"%en0", "::ffff:1.2.3.4", "::ffff:192.168.0.1000", "::ffff:c0a8:1",
	"1:2:3:4:5:6:7:8", "1:2:3:4:5:6:7:8:9", "1:2:3:4:5:6:7", "1::2::3",
	"1:2:3:4:5:6:1.2.3.4", "1:2:3:4:5:6:7:1.2.3.4", "1:2:3:4:5:1.2.3.4",
	"12345::", "fffff::", "1:2:3:", ":1", "1:", "::1:2:3:4:5:6:7", "::1:2:3:4:5:6:7:8",
	"2001:db8::a:b:c:d", "2001:0db8:85a3:0000:0000:8a2e:0370:7334", "abc", "test.com",
	"0:0::test.com", "2020::2020:20:2020:2020%Windows Loves Spaces",
}

var addrAlphabet = []string{
	"0", "1", "9", "a", "f", "F", "g", ".", ":", "::", "%", "%25", "x", " ", "\x00", "\xff", "1.2.3.4",
}

// addrCorpus mutates addrSeeds over addrAlphabet.
func addrCorpus(m *mutator, n int) []string {
	seen := map[string]bool{}
	var out []string
	for _, s := range addrSeeds {
		if !seen[s] {
			seen[s] = true
			out = append(out, s)
		}
	}
	for len(out) < len(addrSeeds)+n {
		b := []byte(addrSeeds[m.r.IntN(len(addrSeeds))])
		for range 1 + m.r.IntN(3) {
			pos := 0
			if len(b) > 0 {
				pos = m.r.IntN(len(b) + 1)
			}
			tok := addrAlphabet[m.r.IntN(len(addrAlphabet))]
			if m.r.IntN(3) == 0 && pos < len(b) {
				b = append(b[:pos:pos], b[pos+1:]...)
			} else {
				b = append(b[:pos:pos], append([]byte(tok), b[pos:]...)...)
			}
		}
		if s := string(b); !seen[s] {
			seen[s] = true
			out = append(out, s)
		}
	}
	return out
}

var queryAlphabet = []string{
	"&", "&", "=", ";", "%", "%3B", "%26", "%3D", "%zz", "%4", "+", " ", "a", "b", "k", "1", "\xff", "é", "#", "?",
}

// queryCorpus mutates the query-like seeds over queryAlphabet.
func queryCorpus(m *mutator, seeds []string, n int) []string {
	seen := map[string]bool{}
	var out []string
	for _, s := range seeds {
		if !seen[s] {
			seen[s] = true
			out = append(out, s)
		}
	}
	for len(out) < len(seeds)+n {
		b := []byte(seeds[m.r.IntN(len(seeds))])
		for range 1 + m.r.IntN(4) {
			pos := 0
			if len(b) > 0 {
				pos = m.r.IntN(len(b) + 1)
			}
			tok := queryAlphabet[m.r.IntN(len(queryAlphabet))]
			if m.r.IntN(4) == 0 && pos < len(b) {
				b = append(b[:pos:pos], b[pos+1:]...)
			} else {
				b = append(b[:pos:pos], append([]byte(tok), b[pos:]...)...)
			}
		}
		if s := string(b); !seen[s] {
			seen[s] = true
			out = append(out, s)
		}
	}
	return out
}

// hostColonSeeds are inputs whose hosts have colons, for urlstrictcolons.
var hostColonSeeds = []string{
	"https://1:2:3:4:5:6:7:8",
	"https://1:2:3:4:5:6:7:8:80",
	"https://example.com:80:",
	"http://a:1:2/",
	"http://a::/",
	"http://a:b:80/",
	"http://a:80:b/",
	"HTTP://x:1:2",
	"Https://x:1:2",
	"httpx://x:1:2",
	"postgres://host1:1,host2:2,host3:3",
	"mongodb://u:p@host1:1,host2:2",
	"ftp://h:1:2/",
	"http://[::1]:80:90/",
	"http://u:p@h:1:2/",
	"//h:1:2/p",
	"http://h:/",
	"http://h:12a/",
}

var godebugParse = []string{
	"urlstrictcolons=0",
	"urlstrictcolons=1",
	"urlstrictcolons=",
	"urlstrictcolons=00",
	"urlstrictcolons=0,urlstrictcolons=1",
	"urlstrictcolons=1,urlstrictcolons=0",
	"urlstrictcolons=0#",
	"x=1,urlstrictcolons=0,y=2",
	"urlstrictcolons=0=1",
}

var godebugLimits = []string{
	"",
	"urlmaxqueryparams=9",
	"urlmaxqueryparams=0",
	"urlmaxqueryparams=10",
	"urlmaxqueryparams=1",
	"urlmaxqueryparams=-1",
	"urlmaxqueryparams=+10",
	"urlmaxqueryparams=abc",
	"urlmaxqueryparams=",
	"urlmaxqueryparams=10001",
	"urlmaxqueryparams=5#",
	"urlmaxqueryparams=7,urlmaxqueryparams=3",
	"urlmaxqueryparams=3,urlmaxqueryparams=7",
	"x=1,urlmaxqueryparams=4,y",
	"urlmaxqueryparams=99999999999999999999",
	"urlmaxqueryparams= 5",
	"urlmaxqueryparams=5 ",
	"urlmaxqueryparams=0x10",
	"urlmaxqueryparams=1_0",
}

var limitParams = []int{1, 2, 3, 4, 5, 9, 10, 11, 10000, 10001}

func build() golden {
	m := &mutator{r: rand.New(rand.NewPCG(1, 14))}
	seeds := append(append(append([]string{}, testSeeds...), exampleSeeds...), gorseSeeds...)

	g := golden{GoVersion: runtime.Version()}
	parseInputs := corpus(m, seeds, 2500)
	for _, in := range parseInputs {
		g.Parse = append(g.Parse, parseCaseOf(in, ""))
	}

	for _, in := range parseInputs {
		c := escapeCase{
			In:          bs(in),
			QueryEscape: bs(url.QueryEscape(in)),
			PathEscape:  bs(url.PathEscape(in)),
		}
		s, err := url.QueryUnescape(in)
		c.QueryUnescape, c.QueryUnescapeEr = bs(s), errText(err)
		s, err = url.PathUnescape(in)
		c.PathUnescape, c.PathUnescapeErr = bs(s), errText(err)
		g.Escape = append(g.Escape, c)
	}

	for _, in := range queryCorpus(m, seeds, 1500) {
		v, err := url.ParseQuery(in)
		g.Query = append(g.Query, queryCase{
			In:     bs(in),
			Values: sortedValues(v),
			Err:    errText(err),
			Encode: bs(v.Encode()),
		})
	}

	for range 1200 {
		u := m.randomURL(seeds)
		g.Methods = append(g.Methods, methodsCase{URL: toFields(u), Methods: urlMethods(u)})
	}

	for _, in := range addrCorpus(m, 1500) {
		c := addrCase{In: bs(in)}
		a, err := netip.ParseAddr(in)
		if err != nil {
			c.Err = err.Error()
		} else {
			c.Is4 = a.Is4()
			c.Zone = bs(a.Zone())
			if a.Is4() {
				b := a.As4()
				c.Bytes = hex.EncodeToString(b[:])
			} else {
				b := a.As16()
				c.Bytes = hex.EncodeToString(b[:])
			}
		}
		g.ParseAddr = append(g.ParseAddr, c)
	}

	hostInputs := corpus(m, hostColonSeeds, 100)
	for _, gd := range godebugParse {
		must(os.Setenv("GODEBUG", gd))
		for _, in := range hostInputs {
			g.GodebugParse = append(g.GodebugParse, parseCaseOf(in, gd))
		}
	}

	for _, gd := range godebugLimits {
		must(os.Setenv("GODEBUG", gd))
		for _, n := range limitParams {
			ps := make([]string, n)
			for i := range ps {
				ps[i] = fmt.Sprintf("p%v", i)
			}
			v, err := url.ParseQuery(strings.Join(ps, "&"))
			g.QueryLimits = append(g.QueryLimits, limitCase{Godebug: gd, Params: n, Err: errText(err), Len: len(v)})
		}
	}
	must(os.Unsetenv("GODEBUG"))

	return g
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
