// Ports: none. Checks the bench command's flags and help against `contracts/cli/gorse-bench.help.txt`, the `--help` output of the pinned Go binary.

//! The bench command's flags, short forms, value types and defaults, and its
//! root help text.

use std::path::PathBuf;

use super::*;

/// The root help of the contract. Its `-j` default is `runtime.NumCPU()` on the
/// machine that built the Go binary, so this machine's count stands in for it.
fn root_contract() -> String {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/cli/gorse-bench.help.txt");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    text.replace("(default 4)", &format!("(default {})", num_cpu()))
}

#[test]
fn bench_root_help_matches_the_contract() {
    assert_eq!(help_text(&command(), NAME), root_contract());
}

/// The help of one subcommand of the contract, read from `gorse-bench-<name>.help.txt`.
fn subcommand_contract(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../contracts/cli/gorse-bench-{name}.help.txt"));
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

#[test]
fn bench_subcommand_help_matches_the_contract() {
    for name in ["reranker", "decision", "embedding"] {
        let sub = named(name);
        assert_eq!(
            help_text(&sub, &format!("{NAME} {name}")),
            subcommand_contract(name),
            "{name} help"
        );
    }
}

#[test]
fn bench_embedding_flags_have_the_go_defaults() {
    let matches = command()
        .try_get_matches_from([NAME, "embedding"])
        .expect("the embedding command parses");
    let (_, sub) = matches.subcommand().expect("a subcommand");
    let flags = EmbeddingFlags::from(sub);
    assert_eq!(flags.top, 10);
    assert_eq!(flags.shots, i64::MAX);
    assert_eq!(flags.dimensions, 0);
    assert_eq!(flags.model, None);
    assert!(flags.embedding_column.is_empty());
    assert!(flags.text_column.is_empty());
}

#[test]
fn bench_embedding_flags_take_their_short_forms() {
    let matches = command()
        .try_get_matches_from([
            NAME,
            "embedding",
            "-k",
            "5",
            "-s",
            "3",
            "--text-column=body",
        ])
        .expect("the short forms parse");
    let (_, sub) = matches.subcommand().expect("a subcommand");
    let flags = EmbeddingFlags::from(sub);
    assert_eq!((flags.top, flags.shots), (5, 3));
    assert_eq!(flags.text_column, "body");
}

#[test]
fn bench_embedding_model_is_set_only_when_the_flag_is_given() {
    let given = command()
        .try_get_matches_from([
            NAME,
            "embedding",
            "--embedding-model=text-embedding-3-small",
        ])
        .expect("the flag parses");
    let (_, sub) = given.subcommand().expect("a subcommand");
    assert_eq!(
        EmbeddingFlags::from(sub).model.as_deref(),
        Some("text-embedding-3-small")
    );
    let absent = command()
        .try_get_matches_from([NAME, "embedding"])
        .expect("the command parses");
    let (_, sub) = absent.subcommand().expect("a subcommand");
    assert_eq!(EmbeddingFlags::from(sub).model, None);
}

#[test]
fn bench_user_auc_is_a_boolean_flag_of_the_ranking_commands() {
    for name in ["reranker", "decision"] {
        let bare = command()
            .try_get_matches_from([NAME, name, "--user-auc"])
            .expect("a bare flag parses");
        let (_, sub) = bare.subcommand().expect("a subcommand");
        assert!(
            cli::flag(sub, "user-auc"),
            "{name}: --user-auc alone is true"
        );
        let off = command()
            .try_get_matches_from([NAME, name, "--user-auc=false"])
            .expect("the value parses");
        let (_, sub) = off.subcommand().expect("a subcommand");
        assert!(
            !cli::flag(sub, "user-auc"),
            "{name}: --user-auc=false is false"
        );
    }
}

#[test]
fn bench_jobs_default_is_the_cpu_count_and_config_is_empty() {
    let matches = command()
        .try_get_matches_from([NAME, "reranker"])
        .expect("the command parses");
    let common = Common::from(&matches, matches.subcommand().expect("a subcommand").1);
    assert_eq!(common.jobs, num_cpu());
    assert!(common.config.is_empty());
}
