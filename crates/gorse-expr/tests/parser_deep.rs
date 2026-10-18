//! Trees deeper than any stack the tests run on: Go's stacks grow to a
//! gigabyte, so the port must take any depth the parser accepts without
//! overflowing (the node budget is off here, as a config may turn it off).

use gorse_expr::ast::{self, Node, Visitor};
use gorse_expr::conf::Config;
use gorse_expr::parser;

struct Count(usize);

impl Visitor for Count {
    fn visit(&mut self, _: &mut Node) {
        self.0 += 1;
    }
}

fn check(input: &str, nodes: usize, printed: &str) {
    let config = Config::default();
    let mut tree = parser::parse_with_config(input, Some(&config)).expect("parses");
    let mut count = Count(0);
    ast::walk(&mut tree.node, &mut count);
    assert_eq!(count.0, nodes);
    assert_eq!(tree.node.to_string(), printed);
    let copy = tree.node.clone();
    assert_eq!(copy, tree.node);
    assert!(ast::find(&tree.node, |_| false).is_none());
    assert!(!format!("{copy:?}").is_empty());
    drop(copy);
}

#[test]
fn parser_deep_trees() {
    // On a small stack, to show the depth does not rest on the stack size.
    let worker = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            const N: usize = 100_000;
            check(&("-".repeat(N) + "1"), N + 1, &("-".repeat(N) + "1"));
            check(
                &("[".repeat(N) + &"]".repeat(N)),
                N,
                &("[".repeat(N) + &"]".repeat(N)),
            );
            check(&("(".repeat(N) + "a" + &")".repeat(N)), 1, "a");
            let ifs = "if a { 1 } else ".repeat(N) + "{ 2 }";
            check(&ifs, 3 * N + 1, &ifs);
            // Deep chains on the left too.
            check(
                &("a".to_owned() + &".b".repeat(N)),
                2 * N + 1,
                &("a".to_owned() + &".b".repeat(N)),
            );
        })
        .expect("thread");
    worker.join().expect("no stack overflow");
}
