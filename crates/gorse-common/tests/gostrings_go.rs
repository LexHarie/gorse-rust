// Ports: $GOROOT/src/strings/replace_test.go (TestReplacer, TestPickAlgorithm, TestWriteStringError, TestGenericTrieBuilding) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's strings package tests: Copyright 2009 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's `replace_test.go`, case for case. The benchmarks are not ported.

mod gostrings {
    use std::io::{self, Write};

    use gorse_common::gostrconv::quote;
    use gorse_common::gostrings::Replacer;

    fn new_replacer(oldnew: &[&[u8]]) -> Replacer {
        assert!(oldnew.len() % 2 == 0, "strings.NewReplacer: odd argument count");
        let pairs: Vec<(&[u8], &[u8])> = oldnew.chunks(2).map(|p| (p[0], p[1])).collect();
        Replacer::new(&pairs)
    }

    fn html_escaper() -> Replacer {
        new_replacer(&[
            b"&", b"&amp;", b"<", b"&lt;", b">", b"&gt;", b"\"", b"&quot;", b"'", b"&apos;",
        ])
    }

    fn html_unescaper() -> Replacer {
        new_replacer(&[
            b"&amp;", b"&", b"&lt;", b"<", b"&gt;", b">", b"&quot;", b"\"", b"&apos;", b"'",
        ])
    }

    fn capital_letters() -> Replacer {
        new_replacer(&[b"a", b"A", b"b", b"B"])
    }

    fn repeat(s: &[u8], n: usize) -> Vec<u8> {
        s.repeat(n)
    }

    // TestReplacer tests the replacer implementations.
    #[test]
    fn test_replacer() {
        struct TestCase {
            r: Replacer,
            input: Vec<u8>,
            out: Vec<u8>,
        }
        let tc = |r: &Replacer, input: &[u8], out: &[u8]| TestCase {
            r: r.clone(),
            input: input.to_vec(),
            out: out.to_vec(),
        };
        let mut test_cases: Vec<TestCase> = Vec::new();

        // str converts 0xff to "\xff". This isn't just string(b) since that
        // converts to UTF-8.
        let str = |b: u8| vec![b];

        // inc maps "\x00"->"\x01", ..., "a"->"b", "b"->"c", ..., "\xff"->"\x00".
        let mut s: Vec<Vec<u8>> = Vec::new();
        for i in 0..256usize {
            s.push(str(i as u8));
            s.push(str((i + 1) as u8));
        }
        let inc = new_replacer(&s.iter().map(Vec::as_slice).collect::<Vec<_>>());

        // Test cases with 1-byte old strings, 1-byte new strings.
        let cap = capital_letters();
        test_cases.push(tc(&cap, b"brad", b"BrAd"));
        test_cases.push(tc(
            &cap,
            &repeat(b"a", (32 << 10) + 123),
            &repeat(b"A", (32 << 10) + 123),
        ));
        test_cases.push(tc(&cap, b"", b""));
        test_cases.push(tc(&inc, b"brad", b"csbe"));
        test_cases.push(tc(&inc, b"\x00\xff", b"\x01\x00"));
        test_cases.push(tc(&inc, b"", b""));
        test_cases.push(tc(&new_replacer(&[b"a", b"1", b"a", b"2"]), b"brad", b"br1d"));

        // repeat maps "a"->"a", "b"->"bb", "c"->"ccc", ...
        let mut s: Vec<Vec<u8>> = Vec::new();
        for i in 0..256i64 {
            let n = (i + 1 - i64::from(b'a')).max(1);
            s.push(str(i as u8));
            s.push(repeat(&str(i as u8), n as usize));
        }
        let repeat_r = new_replacer(&s.iter().map(Vec::as_slice).collect::<Vec<_>>());

        // Test cases with 1-byte old strings, variable length new strings.
        let esc = html_escaper();
        test_cases.push(tc(&esc, b"No changes", b"No changes"));
        test_cases.push(tc(
            &esc,
            b"I <3 escaping & stuff",
            b"I &lt;3 escaping &amp; stuff",
        ));
        test_cases.push(tc(&esc, b"&&&", b"&amp;&amp;&amp;"));
        test_cases.push(tc(&esc, b"", b""));
        test_cases.push(tc(&repeat_r, b"brad", b"bbrrrrrrrrrrrrrrrrrradddd"));
        test_cases.push(tc(&repeat_r, b"abba", b"abbbba"));
        test_cases.push(tc(&repeat_r, b"", b""));
        test_cases.push(tc(&new_replacer(&[b"a", b"11", b"a", b"22"]), b"brad", b"br11d"));

        // The remaining test cases have variable length old strings.
        let unesc = html_unescaper();
        test_cases.push(tc(&unesc, b"&amp;amp;", b"&amp;"));
        test_cases.push(tc(
            &unesc,
            b"&lt;b&gt;HTML&apos;s neat&lt;/b&gt;",
            b"<b>HTML's neat</b>",
        ));
        test_cases.push(tc(&unesc, b"", b""));
        test_cases.push(tc(
            &new_replacer(&[b"a", b"1", b"a", b"2", b"xxx", b"xxx"]),
            b"brad",
            b"br1d",
        ));
        test_cases.push(tc(
            &new_replacer(&[b"a", b"1", b"aa", b"2", b"aaa", b"3"]),
            b"aaaa",
            b"1111",
        ));
        test_cases.push(tc(
            &new_replacer(&[b"aaa", b"3", b"aa", b"2", b"a", b"1"]),
            b"aaaa",
            b"31",
        ));

        // gen1 has multiple old strings of variable length. There is no
        // overall non-empty common prefix, but some pairwise common prefixes.
        let gen1 = new_replacer(&[
            b"aaa",
            b"3[aaa]",
            b"aa",
            b"2[aa]",
            b"a",
            b"1[a]",
            b"i",
            b"i",
            b"longerst",
            b"most long",
            b"longer",
            b"medium",
            b"long",
            b"short",
            b"xx",
            b"xx",
            b"x",
            b"X",
            b"X",
            b"Y",
            b"Y",
            b"Z",
        ]);
        test_cases.push(tc(&gen1, b"fooaaabar", b"foo3[aaa]b1[a]r"));
        test_cases.push(tc(
            &gen1,
            b"long, longerst, longer",
            b"short, most long, medium",
        ));
        test_cases.push(tc(&gen1, b"xxxxx", b"xxxxX"));
        test_cases.push(tc(&gen1, b"XiX", b"YiY"));
        test_cases.push(tc(&gen1, b"", b""));

        // gen2 has multiple old strings with no pairwise common prefix.
        let gen2 = new_replacer(&[
            b"roses", b"red", b"violets", b"blue", b"sugar", b"sweet",
        ]);
        test_cases.push(tc(
            &gen2,
            b"roses are red, violets are blue...",
            b"red are red, blue are blue...",
        ));
        test_cases.push(tc(&gen2, b"", b""));

        // gen3 has multiple old strings with an overall common prefix.
        let gen3 = new_replacer(&[
            b"abracadabra",
            b"poof",
            b"abracadabrakazam",
            b"splat",
            b"abraham",
            b"lincoln",
            b"abrasion",
            b"scrape",
            b"abraham",
            b"isaac",
        ]);
        test_cases.push(tc(
            &gen3,
            b"abracadabrakazam abraham",
            b"poofkazam lincoln",
        ));
        test_cases.push(tc(&gen3, b"abrasion abracad", b"scrape abracad"));
        test_cases.push(tc(&gen3, b"abba abram abrasive", b"abba abram abrasive"));
        test_cases.push(tc(&gen3, b"", b""));

        // foo{1,2,3,4} have multiple old strings with an overall common prefix
        // and 1- or 2- byte extensions from the common prefix.
        let foo1 = new_replacer(&[b"foo1", b"A", b"foo2", b"B", b"foo3", b"C"]);
        let foo2 = new_replacer(&[b"foo1", b"A", b"foo2", b"B", b"foo31", b"C", b"foo32", b"D"]);
        let foo3 = new_replacer(&[
            b"foo11", b"A", b"foo12", b"B", b"foo31", b"C", b"foo32", b"D",
        ]);
        let foo4 = new_replacer(&[b"foo12", b"B", b"foo32", b"D"]);
        test_cases.push(tc(&foo1, b"fofoofoo12foo32oo", b"fofooA2C2oo"));
        test_cases.push(tc(&foo1, b"", b""));
        test_cases.push(tc(&foo2, b"fofoofoo12foo32oo", b"fofooA2Doo"));
        test_cases.push(tc(&foo2, b"", b""));
        test_cases.push(tc(&foo3, b"fofoofoo12foo32oo", b"fofooBDoo"));
        test_cases.push(tc(&foo3, b"", b""));
        test_cases.push(tc(&foo4, b"fofoofoo12foo32oo", b"fofooBDoo"));
        test_cases.push(tc(&foo4, b"", b""));

        // genAll maps "\x00\x01\x02...\xfe\xff" to "[all]", amongst other
        // things.
        let all_string: Vec<u8> = (0..=255u8).collect();
        let gen_all = new_replacer(&[&all_string, b"[all]", b"\xff", b"[ff]", b"\x00", b"[00]"]);
        test_cases.push(tc(&gen_all, &all_string, b"[all]"));
        let mut input = b"a\xff".to_vec();
        input.extend_from_slice(&all_string);
        input.push(0);
        test_cases.push(tc(&gen_all, &input, b"a[ff][all][00]"));
        test_cases.push(tc(&gen_all, b"", b""));

        // Test cases with empty old strings.
        let blank_to_x1 = new_replacer(&[b"", b"X"]);
        let blank_to_x2 = new_replacer(&[b"", b"X", b"", b""]);
        let blank_high_priority = new_replacer(&[b"", b"X", b"o", b"O"]);
        let blank_low_priority = new_replacer(&[b"o", b"O", b"", b"X"]);
        let blank_no_op1 = new_replacer(&[b"", b""]);
        let blank_no_op2 = new_replacer(&[b"", b"", b"", b"A"]);
        let blank_foo = new_replacer(&[b"", b"X", b"foobar", b"R", b"foobaz", b"Z"]);
        test_cases.push(tc(&blank_to_x1, b"foo", b"XfXoXoX"));
        test_cases.push(tc(&blank_to_x1, b"", b"X"));

        test_cases.push(tc(&blank_to_x2, b"foo", b"XfXoXoX"));
        test_cases.push(tc(&blank_to_x2, b"", b"X"));

        test_cases.push(tc(&blank_high_priority, b"oo", b"XOXOX"));
        test_cases.push(tc(&blank_high_priority, b"ii", b"XiXiX"));
        test_cases.push(tc(&blank_high_priority, b"oiio", b"XOXiXiXOX"));
        test_cases.push(tc(&blank_high_priority, b"iooi", b"XiXOXOXiX"));
        test_cases.push(tc(&blank_high_priority, b"", b"X"));

        test_cases.push(tc(&blank_low_priority, b"oo", b"OOX"));
        test_cases.push(tc(&blank_low_priority, b"ii", b"XiXiX"));
        test_cases.push(tc(&blank_low_priority, b"oiio", b"OXiXiOX"));
        test_cases.push(tc(&blank_low_priority, b"iooi", b"XiOOXiX"));
        test_cases.push(tc(&blank_low_priority, b"", b"X"));

        test_cases.push(tc(&blank_no_op1, b"foo", b"foo"));
        test_cases.push(tc(&blank_no_op1, b"", b""));

        test_cases.push(tc(&blank_no_op2, b"foo", b"foo"));
        test_cases.push(tc(&blank_no_op2, b"", b""));

        test_cases.push(tc(&blank_foo, b"foobarfoobaz", b"XRXZX"));
        test_cases.push(tc(&blank_foo, b"foobar-foobaz", b"XRX-XZX"));
        test_cases.push(tc(&blank_foo, b"", b"X"));

        // single string replacer
        let abc_matcher = new_replacer(&[b"abc", b"[match]"]);
        test_cases.push(tc(&abc_matcher, b"", b""));
        test_cases.push(tc(&abc_matcher, b"ab", b"ab"));
        test_cases.push(tc(&abc_matcher, b"abc", b"[match]"));
        test_cases.push(tc(&abc_matcher, b"abcd", b"[match]d"));
        test_cases.push(tc(
            &abc_matcher,
            b"cabcabcdabca",
            b"c[match][match]d[match]a",
        ));

        // Issue 6659 cases (more single string replacer)
        let no_hello = new_replacer(&[b"Hello", b""]);
        test_cases.push(tc(&no_hello, b"Hello", b""));
        test_cases.push(tc(&no_hello, b"Hellox", b"x"));
        test_cases.push(tc(&no_hello, b"xHello", b"x"));
        test_cases.push(tc(&no_hello, b"xHellox", b"xx"));

        // No-arg test cases.
        let nop = new_replacer(&[]);
        test_cases.push(tc(&nop, b"abc", b"abc"));
        test_cases.push(tc(&nop, b"", b""));

        // Run the test cases.
        let mut errors = Vec::new();
        for (i, tc) in test_cases.iter().enumerate() {
            let s = tc.r.replace(&tc.input);
            if s != tc.out {
                errors.push(format!(
                    "{i}. Replace({}) = {}, want {}",
                    quote(&tc.input),
                    quote(&s),
                    quote(&tc.out)
                ));
            }
            let mut buf = Vec::new();
            let (n, err) = tc.r.write_string(&mut buf, &tc.input);
            if let Err(err) = err {
                errors.push(format!("{i}. WriteString: {err}"));
                continue;
            }
            if buf != tc.out {
                errors.push(format!(
                    "{i}. WriteString({}) wrote {}, want {}",
                    quote(&tc.input),
                    quote(&buf),
                    quote(&tc.out)
                ));
                continue;
            }
            if n != tc.out.len() {
                errors.push(format!(
                    "{i}. WriteString({}) wrote correct string but reported {n} bytes; want {} ({})",
                    quote(&tc.input),
                    tc.out.len(),
                    quote(&tc.out)
                ));
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    fn algorithm_test_cases() -> Vec<(Replacer, &'static str)> {
        vec![
            (capital_letters(), "*strings.byteReplacer"),
            (html_escaper(), "*strings.byteStringReplacer"),
            (new_replacer(&[b"12", b"123"]), "*strings.singleStringReplacer"),
            (new_replacer(&[b"1", b"12"]), "*strings.byteStringReplacer"),
            (new_replacer(&[b"", b"X"]), "*strings.genericReplacer"),
            (
                new_replacer(&[b"a", b"1", b"b", b"12", b"cde", b"123"]),
                "*strings.genericReplacer",
            ),
        ]
    }

    // TestPickAlgorithm tests that NewReplacer picks the correct algorithm.
    #[test]
    fn test_pick_algorithm() {
        for (i, (r, want)) in algorithm_test_cases().iter().enumerate() {
            let got = r.algorithm();
            assert_eq!(got, *want, "{i}. algorithm = {got}, want {want}");
        }
    }

    struct ErrWriter;

    impl Write for ErrWriter {
        fn write(&mut self, _p: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("unwritable"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    // TestWriteStringError tests that WriteString returns an error received
    // from the underlying io.Writer.
    #[test]
    fn test_write_string_error() {
        for (i, (r, _)) in algorithm_test_cases().iter().enumerate() {
            let (n, err) = r.write_string(&mut ErrWriter, b"abc");
            let ok = n == 0 && err.as_ref().is_err_and(|e| e.to_string() == "unwritable");
            assert!(ok, "{i}. WriteStringError = {n}, {err:?}, want 0, unwritable");
        }
    }

    // TestGenericTrieBuilding verifies the structure of the generated trie.
    // There is one node per line, and the key ending with the current line is
    // in the trie if it ends with a "+".
    #[test]
    fn test_generic_trie_building() {
        let test_cases: &[(&str, &str)] = &[
            (
                "abc;abdef;abdefgh;xx;xy;z",
                "-
			a-
			.b-
			..c+
			..d-
			...ef+
			.....gh+
			x-
			.x+
			.y+
			z+
			",
            ),
            (
                "abracadabra;abracadabrakazam;abraham;abrasion",
                "-
			a-
			.bra-
			....c-
			.....adabra+
			...........kazam+
			....h-
			.....am+
			....s-
			.....ion+
			",
            ),
            (
                "aaa;aa;a;i;longerst;longer;long;xx;x;X;Y",
                "-
			X+
			Y+
			a+
			.a+
			..a+
			i+
			l-
			.ong+
			....er+
			......st+
			x+
			.x+
			",
            ),
            (
                "foo;;foo;foo1",
                "+
			f-
			.oo+
			...1+
			",
            ),
        ];

        for (input, out) in test_cases {
            let keys: Vec<&str> = input.split(';').collect();
            let mut args: Vec<&[u8]> = vec![b""; keys.len() * 2];
            for (i, key) in keys.iter().enumerate() {
                args[i * 2] = key.as_bytes();
            }

            let got = new_replacer(&args).print_trie().expect("a generic replacer");
            // Remove tabs from tc.out
            let want: String = out.chars().filter(|&c| c != '\t').collect();

            assert_eq!(got, want, "PrintTrie({input:?})\ngot\n{got}want\n{want}");
        }
    }
}
