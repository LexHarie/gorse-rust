// Checks the go-toml v2.2.4 port on documents nested deeper than the golden cases

//! Documents that nest a hundred thousand levels deep, decoded on the test
//! thread's default stack. Go's stacks grow up to 1 GB, so go-toml takes
//! these; the golden cases go ten thousand levels deep (`edge nesting` in
//! `toml_golden.json`), and these check the same shapes and errors ten times
//! deeper, where only the depth differs.

use std::sync::{Mutex, MutexGuard};

use gorse_config::formats::toml::{self, Error};
use gorse_config::value::{Map, Value};

const N: usize = 100_000;

/// The tests take a few hundred MB of stack each in a debug build; they run
/// one at a time.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Drops a decoded map without recursing (the compiler's drop of a `Value`
/// recurses once per level).
fn drop_deep(m: Map) {
    let mut stack: Vec<Value> = m.into_iter().map(|(_, v)| v).collect();
    while let Some(v) = stack.pop() {
        match v {
            Value::Slice(items) => stack.extend(items),
            Value::Map(m) => stack.extend(m.into_iter().map(|(_, v)| v)),
            _ => {}
        }
    }
}

/// The value under `key`, which must be the map's only key.
fn only<'m>(m: &'m Map, key: &str) -> &'m Value {
    assert_eq!(m.keys().collect::<Vec<_>>(), vec![key]);
    m.get(key).expect("key")
}

/// Follows `levels` single-key maps keyed `key` from `v` and returns the
/// value at the bottom.
fn descend_maps<'v>(mut v: &'v Value, key: &str, levels: usize) -> &'v Value {
    for level in 0..levels {
        match v {
            Value::Map(m) => v = only(m, key),
            other => panic!("level {level}: {other:?}"),
        }
    }
    v
}

fn decode_ok(doc: &str) -> Map {
    match toml::unmarshal(doc.as_bytes()) {
        Ok(m) => m,
        Err(err) => panic!("{err}"),
    }
}

fn decode_err(doc: &str) -> Error {
    match toml::unmarshal(doc.as_bytes()) {
        Ok(m) => {
            drop_deep(m);
            panic!("decoded")
        }
        Err(err) => err,
    }
}

fn assert_decode_error(err: &Error, message: &str, position: (i64, i64)) {
    match err {
        Error::Decode(de) => {
            assert_eq!(de.message(), message);
            assert_eq!(de.position(), position);
        }
        Error::Other(text) => panic!("not a decode error: {text}"),
    }
}

#[test]
fn deep_arrays() {
    let _serial = serial();
    let doc = format!("a={}{}", "[".repeat(N), "]".repeat(N));
    let m = decode_ok(&doc);
    let mut v = only(&m, "a");
    for level in 1..N {
        match v {
            Value::Slice(items) if items.len() == 1 => v = &items[0],
            other => panic!("level {level}: {other:?}"),
        }
    }
    assert_eq!(v, &Value::Slice(Vec::new()));
    drop_deep(m);
}

#[test]
fn deep_inline_tables() {
    let _serial = serial();
    let doc = format!("a={}1{}", "{a=".repeat(N), "}".repeat(N));
    let m = decode_ok(&doc);
    let v = descend_maps(only(&m, "a"), "a", N);
    assert_eq!(v, &Value::Int64(1));
    drop_deep(m);
}

#[test]
fn deep_arrays_of_inline_tables() {
    let _serial = serial();
    let n = N / 2;
    let doc = format!("a={}1{}", "[{a=".repeat(n), "}]".repeat(n));
    let m = decode_ok(&doc);
    let mut v = only(&m, "a");
    for level in 0..n {
        match v {
            Value::Slice(items) if items.len() == 1 => match &items[0] {
                Value::Map(inner) => v = only(inner, "a"),
                other => panic!("level {level}: {other:?}"),
            },
            other => panic!("level {level}: {other:?}"),
        }
    }
    assert_eq!(v, &Value::Int64(1));
    drop_deep(m);
}

#[test]
fn long_dotted_keys() {
    let _serial = serial();
    let doc = format!("{}b=1", "a.".repeat(N));
    let m = decode_ok(&doc);
    let v = descend_maps(only(&m, "a"), "a", N - 1);
    match v {
        Value::Map(inner) => assert_eq!(only(inner, "b"), &Value::Int64(1)),
        other => panic!("{other:?}"),
    }
    drop_deep(m);

    let doc = format!("x={{{}b=1}}", "a.".repeat(N));
    let m = decode_ok(&doc);
    let v = descend_maps(only(&m, "x"), "a", N);
    match v {
        Value::Map(inner) => assert_eq!(only(inner, "b"), &Value::Int64(1)),
        other => panic!("{other:?}"),
    }
    drop_deep(m);
}

#[test]
fn long_table_keys() {
    let _serial = serial();
    let doc = format!("[{}b]\nc=1\n[[{}d]]\ne=2", "a.".repeat(N), "a.".repeat(N));
    let m = decode_ok(&doc);
    let v = descend_maps(only(&m, "a"), "a", N - 1);
    match v {
        Value::Map(inner) => {
            assert_eq!(inner.keys().collect::<Vec<_>>(), vec!["b", "d"]);
            match inner.get("b") {
                Some(Value::Map(b)) => assert_eq!(only(b, "c"), &Value::Int64(1)),
                other => panic!("{other:?}"),
            }
            match inner.get("d") {
                Some(Value::Slice(d)) => match d.as_slice() {
                    [Value::Map(t)] => assert_eq!(only(t, "e"), &Value::Int64(2)),
                    other => panic!("{other:?}"),
                },
                other => panic!("{other:?}"),
            }
        }
        other => panic!("{other:?}"),
    }
    drop_deep(m);
}

#[test]
fn deep_errors() {
    let _serial = serial();
    let n = i64::try_from(N).expect("N");

    // `edge nesting 4`: the error is at the end of the document.
    let err = decode_err(&format!("a={}", "[".repeat(N)));
    assert_decode_error(
        &err,
        "expected character ] but the document ended here",
        (1, n + 3),
    );

    // `edge nesting 5`.
    let err = decode_err(&format!("a={}", "{a=".repeat(N)));
    assert_decode_error(&err, "expected value, not eof", (1, 3 * n + 3));

    // `edge nesting 3`: the innermost value is `}`.
    let err = decode_err(&format!("a={}{}", "{a=".repeat(N), "}".repeat(N)));
    assert_decode_error(&err, "incomplete number", (1, 3 * n + 3));

    // `edge nesting 10`: one brace short.
    let doc = format!("a={}1{}", "{a=".repeat(N), "}".repeat(N - 1));
    let err = decode_err(&doc);
    let end = i64::try_from(doc.len()).expect("len") + 1;
    assert_decode_error(
        &err,
        "expected character } but the document ended here",
        (1, end),
    );

    // `edge nesting 11`: the key is defined again after a deep value, which
    // the decoder drops.
    let doc = format!(
        "a={}1{}\na={}{}",
        "{a=".repeat(N),
        "}".repeat(N),
        "[".repeat(N),
        "]".repeat(N)
    );
    assert_eq!(
        decode_err(&doc),
        Error::Other("toml: key a is already defined".to_owned())
    );
}

/// X-38: Go's goroutine stacks stop at 1 GB, and go-toml's parser
/// recursion reaches that at about 828,000 nested arrays: Go dies with
/// `fatal error: stack overflow` (827,734 levels decode, 828,613 crash; see
/// the notes). The port grows its stack without a limit and decodes them.
/// It needs about 5 GB and 20 s in a debug build (1.8 GB and 2 s in a
/// release build), so it runs on request:
/// `cargo test -p gorse-config --release --test toml_deep -- --ignored`.
#[test]
#[ignore = "needs about 5 GB in a debug build; run with --release -- --ignored"]
fn deeper_than_go_stack_limit() {
    let _serial = serial();
    let n = 1_000_000;
    let doc = format!("a={}{}", "[".repeat(n), "]".repeat(n));
    let m = decode_ok(&doc);
    let mut v = only(&m, "a");
    let mut depth = 1;
    while let Value::Slice(items) = v {
        match items.as_slice() {
            [inner] => {
                v = inner;
                depth += 1;
            }
            [] => break,
            other => panic!("depth {depth}: {} items", other.len()),
        }
    }
    assert_eq!(depth, n);
    drop_deep(m);
}
