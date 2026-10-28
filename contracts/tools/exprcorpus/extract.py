#!/usr/bin/env python3
"""Builds the expression corpus that gorse-expr is tested against.

The corpus is every expression that the pinned Gorse tree's Go tests and
configs give to the expr-lang engine, plus every expression in the expr-lang
v1.17.8 documentation's `expr` code blocks. Each expression is listed once per
environment it can run in, so the Go oracle (contracts/tools/exprevalgolden)
gives the verdict and result of each pair.

Usage, from the repository root, with the expr-lang module in the Go module
cache (`go mod download github.com/expr-lang/expr@v1.17.8`):

    python3 contracts/tools/exprcorpus/extract.py \
        "$(go env GOMODCACHE)/github.com/expr-lang/expr@v1.17.8/docs" \
        > crates/gorse-expr/testdata/corpus/expressions.json
"""

import json
import re
import sys
from pathlib import Path

# Expressions the Go tests and configs give to Gorse's expr-lang sites, with the
# environment each one is compiled in: non_personalized (score and filter),
# item (item_to_item columns) and user (user_to_user columns).
GORSE = [
    ("item.Timestamp.Unix()", "non_personalized", "logics/non_personalized_test.go:32"),
    ("len(feedback)", "non_personalized", "logics/non_personalized_test.go:52"),
    ("1", "non_personalized", "master/tasks_test.go:663"),
    ("!item.IsHidden", "non_personalized", "logics/non_personalized_test.go:105"),
    ("count(feedback, .FeedbackType == 'star')", "non_personalized", "logics/non_personalized_test.go:148"),
    ("(now() - item.Timestamp).Hours() < 168", "non_personalized", "logics/non_personalized_test.go:149"),
    ("(now() - item.Timestamp).Nanoseconds() < 3600000000000", "non_personalized", "logics/non_personalized_test.go:73"),
    ("count(feedback, .FeedbackType == 'star')", "non_personalized", "config/config.toml:194"),
    ("(now() - item.Timestamp).Hours() < 168", "non_personalized", "config/config.toml:197"),
    ("len(feedback)", "non_personalized", "client/config.toml:111"),
    ("item.Labels.embedding", "item", "config/config.toml:211"),
    ("item.Labels", "item", "master/tasks_test.go:217"),
    ("item.Labels.embedding", "item", "server/rest_test.go:1076"),
    ("item.Labels.description", "item", "logics/item_to_item_test.go:50"),
    ("item.Labels.embedding", "item", "logics/item_to_item_test.go:146"),
    ("item.Labels.description", "item", "logics/item_to_item_test.go:168"),
    ("item.Labels", "item", "logics/item_to_item_test.go:218"),
    ("item.Labels.embedding", "item", "logics/recommend_test.go:101"),
    ("item.Labels", "item", "logics/recommend_test.go:126"),
    ("item.Labels.brand", "item", "server/rest_test.go:819"),
    ("item.ItemId", "item", "server/rest_test.go:819"),
    ("item.Categories", "item", "server/rest_test.go:819"),
    ("item.Comment", "item", "server/rest_test.go:819"),
    ("item.Labels.title", "item", "contracts/tools/schemadump/main.go:54"),
    ("item.Labels.description", "item", "config/config_test.go:563"),
    ("item.Labels.", "item", "config/config_test.go:566"),
    ("user.Labels.description", "user", "logics/user_to_user_test.go:51"),
    ("user.Labels.embedding", "user", "logics/user_to_user_test.go:76"),
    ("user.Labels", "user", "logics/user_to_user_test.go:103"),
    ("item.Labels.missing", "item", "synthetic: absent key"),
    ("user.Labels.missing", "user", "synthetic: absent key"),
]

# Expressions the docs and Gorse tests do not give: type rules, runtime errors
# and every builtin, each compiled in the bare engine. Run in the Gorse sites
# too where it names their variables.
SYNTHETIC = [
    # Literals and operators.
    "nil", "true", "false", "0x1F", "0o17", "0b101", "1_000", "1e3", "2.5e-3",
    "'it''s'", "\"tab\\tnew\\n\"", "`raw\\n`", "b\"abc\"", "b\"\\xff\"",
    "1 + 2 * 3", "(1 + 2) * 3", "2 ** 10", "2 ^ 10", "7 % 3", "-7 % 3", "7 / 2", "7.0 / 2",
    "1 / 0", "1 % 0", "1.0 / 0.0", "-1.0 / 0.0", "0.0 / 0.0", "1 == 1.0", "1 != 2",
    "'a' < 'b'", "'a' >= 'a'", "1 < 2 and 2 < 3", "1 > 2 or 2 < 3", "not true", "!false",
    "true && false", "true || false", "1 + 'a'", "'a' + 1", "'a' + 'b'", "[1] + [2]",
    "-'a'", "+1", "-1", "not 1", "!nil", "1 && 2", "1 || 2", "nil == nil", "nil != nil",
    "1 in [1, 2]", "3 in [1, 2]", "'a' in {'a': 1}", "'a' in 'abc'", "'b' not in ['a']",
    "'abc' matches '^a'", "'abc' matches '('", "'abc' contains 'b'", "'abc' startsWith 'a'",
    "'abc' endsWith 'c'", "'ab' + nil", "1 .. 3", "1..3", "[1..3]", "1..0",
    "'abc'[1]", "'abc'[1:2]", "'abc'[:2]", "'abc'[2:]", "'abc'[5]", "[1, 2, 3][1:]",
    "[1, 2, 3][-1]", "[1, 2, 3][3]", "[1, 2, 3][0:9]", "[1, 2, 3][2:1]", "{'a': 1}['a']",
    "{'a': 1}.a", "{'a': 1}['b']", "{'a': 1}.b", "{a: 1}.a", "{'a': {'b': 2}}.a.b",
    "{'a': nil}.a?.b", "nil?.a", "nil.a", "nil[0]", "1.foo", "'x'.foo",
    "1 ? 'y' : 'n'", "true ? 'y' : 'n'", "nil ?? 'd'", "1 ?? 2", "nil ?: 2", "0 ?: 2",
    "true ? 1 : 2.0", "if true { 1 } else { 2 }", "if 1 == 2 { 1 } else { 'two' }",
    "let x = 2; x * x", "let x = 2; let y = x + 1; x * y", "1; 2", "let f = 1; f",
    "[1, 2, 3] | len()", "'ab' | upper()", "#", "len(#)", "[1, 2] | map(# * 2)",
    # Builtins over arrays and maps.
    "all([true, false], #)", "all([1, 2], # > 0)", "none([1, 2], # > 2)", "any([1, 2], # > 1)",
    "one([1, 2], # > 1)", "filter([1, 2, 3], # > 1)", "filter([1, 2, 3], # > 5)", "map([1, 2], # * 10)",
    "map([1, 2], #index)", "find([1, 2, 3], # > 1)", "find([1, 2, 3], # > 9)",
    "findIndex([1, 2, 3], # > 1)", "findLast([1, 2, 3], # < 3)", "findLastIndex([1, 2, 3], # < 3)",
    "count([1, 2, 3], # > 1)", "count([1, 2, 3])", "count({'a': 1, 'b': 2}, # > 1)", "sum([1, 2, 3])",
    "sum([1.5, 2])", "sum([1, 2], # * 2)", "sum([])", "groupBy([1, 2, 3], # % 2)", "sortBy([3, 1, 2])",
    "sortBy([3, 1, 2], #, 'desc')", "reduce([1, 2, 3], #acc + #)", "reduce([1, 2, 3], #acc + #, 10)",
    "reduce([], #acc + #)", "len([1, 2])", "len('abc')", "len({'a': 1})", "len(nil)", "len(1)",
    "type(1)", "type('a')", "type(nil)", "type([])", "type({})", "type(now())", "type(1.5)",
    "first([1, 2])", "first([])", "last([1, 2])", "last([])", "get([1, 2], 1)", "get([1, 2], 5)",
    "get({'a': 1}, 'a')", "take([1, 2, 3], 2)", "take([1, 2, 3], 9)", "keys({'a': 1, 'b': 2})",
    "values({'a': 1, 'b': 2})", "toPairs({'a': 1})", "fromPairs([['a', 1]])", "reverse([1, 2, 3])",
    "reverse('abc')", "uniq([1, 1, 2])", "concat([1], [2, 3])", "flatten([[1], [2, 3]])",
    "sort([3, 1, 2])", "sort([3, 1, 2], 'desc')", "sort(['b', 'a'])",
    # Builtins over numbers, strings and times.
    "abs(-1.5)", "abs(-2)", "ceil(1.2)", "floor(-1.2)", "round(2.5)", "round(-2.5)", "int('12')",
    "int(3.9)", "int('x')", "float('1.5')", "float(3)", "string(3)", "string(nil)", "string([1])",
    "trim('  a ')", "trim('xxaxx', 'x')", "trimPrefix('abc', 'a')", "trimSuffix('abc', 'c')",
    "upper('a')", "lower('A')", "split('a,b', ',')", "split('abc', '')", "splitAfter('a,b', ',')",
    "replace('aaa', 'a', 'b')", "repeat('a', 3)", "repeat('a', -1)", "join(['a', 'b'], '-')",
    "indexOf('abcb', 'b')", "lastIndexOf('abcb', 'b')", "hasPrefix('abc', 'ab')", "hasSuffix('abc', 'bc')",
    "max(1, 2)", "max([1, 2.5])", "min(1, 2)", "min([])", "mean([1, 2])", "mean([])", "median([3, 1, 2])",
    "median([1, 2])", "toJSON({'a': 1})", "toJSON(nil)", "fromJSON('{\"a\": 1}')", "fromJSON('x')",
    "toBase64('a')", "fromBase64('YQ==')", "duration('1h')", "duration('1.5s')", "duration('1x')",
    "date('2000-01-02')", "date('2000-01-02T03:04:05Z')", "date('2000-01-02 03:04:05', 'UTC')",
    "date('x')", "timezone('UTC')", "date(1000, 'UTC')", "bitand(1, 3)", "bitor(1, 2)",
    "bitxor(3, 1)", "bitnand(3, 1)", "bitshl(1, 4)", "bitshr(16, 2)", "bitushr(-1, 60)", "bitnot(1)",
    "bitshl(1, -1)", "now() > date('2000-01-01')",
    "date('2000-01-02') + duration('1h')", "date('2000-01-02') - date('2000-01-01')",
    "date('2000-01-02') < date('2001-01-01')", "date('2000-01-02') == date('2000-01-02')",
    "date('2000-01-02').Year()", "date('2000-01-02').Month()", "date('2000-01-02').Weekday()",
    "date('2000-01-02').Unix()", "date('2000-01-02').Format('2006-01-02')", "duration('90m').Hours()",
    "duration('90m').Minutes()", "duration('90m').String()", "duration('90m').Seconds()",
    "date('2000-01-02').AddDate(0, 1, 0)", "date('2000-01-02').Add(duration('1h'))",
    "date('2000-01-02').Sub(date('2000-01-01'))", "date('2000-01-02').Before(date('2001-01-01'))",
    "date('2000-01-02').After(date('2001-01-01'))", "date('2000-01-02').Day()",
    "date('2000-01-02').YearDay()", "date('2000-01-02').Hour()", "date('2000-01-02').Nanosecond()",
    "date('2000-01-02').UnixMilli()", "date('2000-01-02').IsZero()", "date('2000-01-02').Truncate(duration('1h'))",
    # Common typing mistakes.
    "true + 1", "[1] - 1", "{} * 2", "nil + 1", "'a' * 2", "1 < 'a'", "1 == 'a'", "[1] == [1]",
    "{'a': 1} == {'a': 1}", "date('2000-01-02') > 1", "1 + duration('1h')", "1.5 % 2", "2 ** 0.5",
    "10 / 4 * 4", "1 << 2",
]

# Synthetic expressions also run in the Gorse sites, where the names exist.
SITE_SYNTHETIC = [
    "item.Labels.nested.x", "item.Labels.nested.y", "item.Labels.brand == 'acme'",
    "item.Labels.rating + 1", "item.Labels.count * 2", "item.Labels.count > 5",
    "item.Labels.brand + 'x'", "item.Labels.flags[2] == nil", "item.Labels.flags[0] ?? 'd'",
    "item.Labels.embedding[1]", "item.Labels.embedding[5]", "item.Labels.embedding | len()",
    "sum(item.Labels.embedding)", "item.Labels.empty == ''", "item.Labels.nope ?? 'default'",
    "item.Labels.nope?.x", "item.Labels.nested?.x", "item.Categories[0]", "item.Categories[5]",
    "item.Categories | len()", "'movie' in item.Categories", "item.ItemId + '-' + item.Comment",
    "item.Comment matches 'World$'", "item.Comment contains 'lo W'", "upper(item.Comment)",
    "split(item.Comment, ' ')", "len(item.Comment)", "item.IsHidden == false",
    "item.Timestamp.Year()", "item.Timestamp.Unix() / 3600", "item.Timestamp + duration('1h')",
    "item.Timestamp.Sub(now()).Hours() < 0", "item.Timestamp > date('1999-01-01')",
    "feedback[0].Value * 2", "feedback[10].Value", "feedback[0].FeedbackType", "feedback[1].Labels.score",
    "feedback[0].Labels.score", "feedback[-1].UserId", "len(feedback) / 2", "sum(feedback, .Value)",
    "sum(feedback, .Value) / len(feedback)", "map(feedback, .Value)", "filter(feedback, .Value > 0.5)",
    "all(feedback, .Value > 0)", "any(feedback, .FeedbackType == 'like')", "one(feedback, .Value == 1)",
    "none(feedback, .Value > 9)", "groupBy(feedback, .FeedbackType)", "count(feedback, .Value > 1)",
    "max(map(feedback, .Value))", "first(feedback).Comment", "last(feedback).Comment",
    "find(feedback, .UserId == '2').Value", "findIndex(feedback, .UserId == '3')",
    "sortBy(feedback, .Value)", "reduce(feedback, #acc + #.Value, 0.0)", "mean(map(feedback, .Value))",
    "count(feedback, #.FeedbackType == 'star')", "count(feedback, .Labels.score > 1)",
    "item.Labels.embedding[0] + item.Labels.embedding[1]", "int(item.Labels.rating)",
    "round(item.Labels.rating)", "string(item.Labels.count)", "toJSON(item.Labels)",
    "toJSON(feedback[0])", "user.Labels.description + ' fan'", "user.Labels.embedding[1]",
    "user.UserId", "user.Comment", "user.Labels", "item.Labels.embedding | sum()",
    "item.Labels.description", "item.Labels.missing.deep", "item.Labels.missing.deep.deeper",
]

# Environments each documentation expression is compiled in: the bare engine,
# and the Gorse sites, so a documented builtin is also checked against the
# item and feedback types.
DOC_ENVIRONMENTS = ["none", "non_personalized", "item"]

FENCE = re.compile(r"^```expr\s*$")
END_FENCE = re.compile(r"^```\s*$")


def strip_comment(line):
    """Drops a trailing `// comment` that is outside every string literal."""
    quote = None
    i = 0
    while i < len(line):
        ch = line[i]
        if quote:
            if ch == "\\" and quote != "`":
                i += 2
                continue
            if ch == quote:
                quote = None
        elif ch in "'\"`":
            quote = ch
        elif line.startswith("//", i) and (i == 0 or line[i - 1] == " "):
            return line[:i].rstrip()
        i += 1
    return line.rstrip()


def doc_expressions(path):
    """The expressions of the `expr` blocks of one document: each line of a
    block, and each whole block, with comments removed."""
    out = []
    block = None
    origin_line = 0
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if block is None:
            if FENCE.match(line):
                block = []
                origin_line = number
            continue
        if END_FENCE.match(line):
            lines = [strip_comment(text) for text in block]
            lines = [text for text in lines if text.strip()]
            if lines:
                origin = f"{path.name}:{origin_line}"
                for text in lines:
                    out.append((text, origin))
                if len(lines) > 1:
                    out.append(("\n".join(lines), origin))
            block = None
            continue
        block.append(line)
    return out


def main():
    docs = Path(sys.argv[1])
    cases = []
    seen = set()

    def add(source, env, origin):
        key = (source, env)
        if key in seen:
            return
        seen.add(key)
        cases.append({"source": source, "env": env, "origin": origin})

    for source, env, origin in GORSE:
        add(source, env, origin)

    for source in SYNTHETIC:
        add(source, "none", "synthetic: bare engine")
        add(source, "non_personalized", "synthetic: bare engine")
    for source in SITE_SYNTHETIC:
        add(source, "non_personalized", "synthetic: site")
        add(source, "item", "synthetic: site")
        add(source, "user", "synthetic: site")

    for name in [
        "language-definition.md",
        "getting-started.md",
        "environment.md",
        "configuration.md",
        "functions.md",
        "patch.md",
    ]:
        for source, origin in doc_expressions(docs / name):
            for env in DOC_ENVIRONMENTS:
                add(source, env, f"docs/{origin}")

    json.dump(cases, sys.stdout, indent=2, ensure_ascii=False)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
