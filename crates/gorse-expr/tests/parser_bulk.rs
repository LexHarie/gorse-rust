//! The parser against what Go's (expr-lang v1.17.8) does with the bulk
//! corpus of `parser_bulk_golden.json` (written by
//! `contracts/tools/exprparsegolden`): for every input, whether it parses,
//! and a digest of the error or the tree (dump, printed form, walk) and of
//! the tokens.

mod common;

use common::{array, full_digest, load, str_of};

#[test]
fn parser_bulk_golden() {
    let golden = load("parser_bulk_golden.json");
    let sources: Vec<&str> = array(&golden["sources"]).iter().map(str_of).collect();
    assert_eq!(sources.len(), 9);
    assert_eq!(
        golden["skipped_invalid_utf8"],
        serde_json::json!({"string literals of the other expr-lang tests": 1})
    );
    let entries = array(&golden["entries"]);
    assert_eq!(entries.len(), 95_585);

    // Each thread checks a share of the entries and returns, per source, the
    // accepted and rejected counts and the mismatches.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = entries.len().div_ceil(threads);
    let results: Vec<(Vec<[usize; 2]>, Vec<String>)> = std::thread::scope(|s| {
        let handles: Vec<_> = entries
            .chunks(chunk)
            .map(|part| {
                let n = sources.len();
                s.spawn(move || {
                    let mut counts = vec![[0usize; 2]; n];
                    let mut mismatches = Vec::new();
                    for e in part {
                        let src = usize::try_from(e[0].as_u64().expect("source")).expect("source");
                        let input = str_of(&e[1]);
                        let (accepted, d) = full_digest(input);
                        if Some(accepted) != e[2].as_bool() || d != str_of(&e[3]) {
                            mismatches.push(format!(
                                "{input:?}: accepted {accepted}, digest {d}, Go {} {}",
                                e[2], e[3]
                            ));
                        }
                        counts[src][usize::from(accepted)] += 1;
                    }
                    (counts, mismatches)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("no panic"))
            .collect()
    });

    let mut counts = vec![[0usize; 2]; sources.len()];
    let mut mismatches = Vec::new();
    for (c, m) in results {
        for (total, part) in counts.iter_mut().zip(c) {
            total[0] += part[0];
            total[1] += part[1];
        }
        mismatches.extend(m);
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} differ from Go, the first: {:#?}",
        mismatches.len(),
        entries.len(),
        &mismatches[..mismatches.len().min(20)]
    );

    // Every source contributes; the generated corpus is all valid, and the
    // random ones are mostly invalid.
    for (name, [rejected, accepted]) in sources.iter().zip(&counts) {
        assert!(rejected + accepted > 0, "{name} is empty");
        eprintln!("{name}: {accepted} accepted, {rejected} rejected");
    }
    assert_eq!(counts[0], [0, 43_682], "testdata/generated.txt");
}
