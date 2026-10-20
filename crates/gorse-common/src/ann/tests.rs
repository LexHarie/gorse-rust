// Ports: common/ann/ann_test.go @ add21b5

//! The Go tests of `common/ann`, and the indexes against data produced by Go
//! (`contracts/tools/anngolden`): every Add result, every search result bit
//! for bit, and every HNSW as its marshalled bytes, built from the layer draws
//! Go's inserts made.

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::Value as Json;
use sha2::{Digest, Sha256};

use super::*;
use crate::floats;

fn golden() -> &'static Json {
    static GOLDEN: std::sync::LazyLock<Json> = std::sync::LazyLock::new(|| {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/ann_golden.json");
        let text = std::fs::read_to_string(path).expect("golden file");
        serde_json::from_str(&text).expect("golden JSON")
    });
    &GOLDEN
}

fn str_of(v: &Json) -> &str {
    v.as_str().expect("string")
}

fn bits(v: &Json) -> f32 {
    f32::from_bits(u32::from_str_radix(str_of(v), 16).expect("hex bits"))
}

fn list(v: &Json) -> &[Json] {
    v.as_array().map_or(&[], Vec::as_slice)
}

fn float_vectors(v: &Json) -> Vec<Vec<f32>> {
    list(v)
        .iter()
        .map(|x| list(x).iter().map(bits).collect())
        .collect()
}

fn int_vectors(v: &Json) -> Vec<Vec<i64>> {
    list(v)
        .iter()
        .map(|x| list(x).iter().map(|i| i.as_i64().expect("int")).collect())
        .collect()
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The squared Euclidean distance of the generator, a plain loop.
#[allow(clippy::ptr_arg)] // A `Distance<Vec<f32>>` takes `&Vec<f32>`.
fn l2(a: &Vec<f32>, b: &Vec<f32>) -> f32 {
    let mut s = 0.0;
    for (x, y) in a.iter().zip(b) {
        let d = x - y;
        s += d * d;
    }
    s
}

/// `jaccard` of `ann_test.go`, between sorted sets.
#[allow(clippy::ptr_arg)] // A `Distance<Vec<i64>>` takes `&Vec<i64>`.
fn jaccard(a: &Vec<i64>, b: &Vec<i64>) -> f32 {
    let (mut i, mut j, mut intersection) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            intersection += 1;
            i += 1;
            j += 1;
        } else if a[i] < b[j] {
            i += 1;
        } else {
            j += 1;
        }
    }
    if a.len() + b.len() - intersection == 0 {
        return 1.0;
    }
    1.0 - intersection as f32 / (a.len() + b.len() - intersection) as f32
}

/// A uniform source replaying the floats Go's inserts drew. It panics with
/// `EXHAUSTED` if asked for more, which would mean the inserts draw at
/// different points than Go's.
fn replay(
    uniforms: &Json,
) -> (
    impl FnMut() -> f32 + Send + Sync + 'static,
    std::sync::Arc<Mutex<usize>>,
) {
    let values: Vec<f32> = list(uniforms).iter().map(bits).collect();
    let used = std::sync::Arc::new(Mutex::new(0));
    let counter = used.clone();
    let source = move || {
        let mut n = counter.lock().expect("lock");
        let u = *values.get(*n).expect(EXHAUSTED);
        *n += 1;
        u
    };
    (source, used)
}

const EXHAUSTED: &str = "more layer draws than Go made";

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_default()
}

fn format_neighbors(r: &[Neighbor]) -> Vec<String> {
    r.iter()
        .map(|(i, d)| format!("{i}:{:08x}", d.to_bits()))
        .collect()
}

/// Replays the recorded searches on `index`.
fn check_searches<T>(name: &str, index: &dyn Index<T>, queries: &[T], searches: &Json) {
    for s in list(searches) {
        let k = usize::try_from(s["k"].as_u64().expect("k")).expect("k");
        let prune0 = s["prune0"].as_bool().expect("prune0");
        let what = format!("{name}: {s}");
        let outcome = catch_unwind(AssertUnwindSafe(|| match str_of(&s["kind"]) {
            "vector" => {
                let q =
                    &queries[usize::try_from(s["query"].as_u64().expect("query")).expect("query")];
                Ok(index.search_vector(q, k, prune0))
            }
            _ => index.search_index(
                usize::try_from(s["q"].as_u64().expect("q")).expect("q"),
                k,
                prune0,
            ),
        }));
        match outcome {
            Err(p) => {
                assert!(
                    s.get("panic").is_some(),
                    "{what}: Rust panicked: {}",
                    panic_message(&*p)
                );
                assert_ne!(panic_message(&*p), EXHAUSTED, "{what}");
            }
            Ok(_) if s.get("panic").is_some() => panic!("{what}: Go panicked, Rust did not"),
            Ok(Err(err)) => assert_eq!(err.to_string(), str_of(&s["error"]), "{what}"),
            Ok(Ok(r)) => {
                assert!(
                    s.get("error").is_none(),
                    "{what}: Go failed, Rust returned {r:?}"
                );
                let expected: Vec<String> = list(&s["result"])
                    .iter()
                    .map(|x| str_of(x).to_owned())
                    .collect();
                assert_eq!(format_neighbors(&r), expected, "{what}");
            }
        }
    }
}

fn marshal<T: GobValue>(h: &Hnsw<T>) -> Vec<u8> {
    let mut b = Vec::new();
    h.marshal(&mut b).expect("marshal");
    b
}

/// Builds an index of `case` from `vectors`, checks the adds (and the add Go
/// panicked on), the HNSW's bytes and every search.
fn check_index<T: GobValue + Clone>(
    case: &Json,
    vectors: Vec<T>,
    queries: Vec<T>,
    distance: Distance<T>,
) {
    let name = str_of(&case["name"]);
    let adds: Vec<usize> = list(&case["adds"])
        .iter()
        .map(|a| usize::try_from(a.as_u64().expect("add")).expect("add"))
        .collect();
    if str_of(&case["index"]) == "bruteforce" {
        let mut bf = Bruteforce::new(distance);
        let got: Vec<usize> = vectors.into_iter().map(|v| bf.add(v)).collect();
        assert_eq!(got, adds, "{name}: adds");
        check_searches(name, &bf, &queries, &case["searches"]);
        return;
    }
    let (source, used) = replay(&case["uniforms"]);
    let mut h = Hnsw::with_uniform(distance, source);
    let mut got = Vec::new();
    let mut vectors = vectors.into_iter();
    if let Some(limits) = case.get("limits") {
        // Limits written into the blob of the index holding the first vector.
        got.push(h.add(vectors.next().expect("a first vector")));
        let mut blob = marshal(&h);
        for (i, l) in list(limits).iter().enumerate() {
            blob[4 + 8 * i..12 + 8 * i].copy_from_slice(&l.as_i64().expect("limit").to_le_bytes());
        }
        h.unmarshal(&mut blob.as_slice()).expect("unmarshal");
    }
    for v in vectors {
        match catch_unwind(AssertUnwindSafe(|| h.add(v))) {
            Ok(j) => got.push(j),
            Err(p) => {
                assert!(
                    case.get("add_panic").is_some(),
                    "{name}: add {} panicked: {}",
                    got.len(),
                    panic_message(&*p)
                );
                assert_ne!(panic_message(&*p), EXHAUSTED, "{name}");
                break;
            }
        }
    }
    assert_eq!(got, adds, "{name}: adds");
    assert_eq!(
        *used.lock().expect("lock"),
        list(&case["uniforms"]).len(),
        "{name}: layer draws"
    );
    if case.get("add_panic").is_some() {
        assert!(
            got.len() < list(&case["vectors"]).len(),
            "{name}: Go panicked in an add"
        );
        return;
    }
    assert_eq!(
        h.layers() as u64,
        case.get("layers").and_then(Json::as_u64).unwrap_or(0),
        "{name}: layers"
    );
    let bytes = marshal(&h);
    assert_eq!(
        hash(&bytes),
        str_of(&case["blob_sha256"]),
        "{name}: marshalled index"
    );
    if let Some(blob) = case.get("blob") {
        // Go's own bytes, upper-layer entries in Go's order, read back.
        let mut copy = Hnsw::with_uniform(distance, || 0.5);
        copy.unmarshal(&mut hex::decode(str_of(blob)).expect("hex").as_slice())
            .expect("unmarshal Go's blob");
        assert_eq!(marshal(&copy), bytes, "{name}: Go's blob re-marshalled");
    }
    check_searches(name, &h, &queries, &case["searches"]);
    if let Some(searches) = case.get("ef_searches") {
        // ef = 1 written into the blob: searches use max(1, k).
        let mut patched = bytes;
        patched[20..28].copy_from_slice(&1_i64.to_le_bytes());
        let mut g = Hnsw::with_uniform(distance, || 0.5);
        g.unmarshal(&mut patched.as_slice()).expect("unmarshal");
        check_searches(name, &g, &queries, searches);
    }
}

fn case_named(name: &str) -> &'static Json {
    list(&golden()["indexes"])
        .iter()
        .find(|c| str_of(&c["name"]) == name)
        .expect("case")
}

#[test]
fn level_factor_and_empty_index_match_go() {
    let h: Hnsw<Vec<f32>> = Hnsw::new(l2);
    assert_eq!(
        h.level_factor.to_bits(),
        bits(&golden()["level_factor"]).to_bits()
    );
    assert_eq!(hex::encode(marshal(&h)), str_of(&golden()["empty_blob"]));
}

#[test]
fn levels_match_go() {
    let factor = bits(&golden()["level_factor"]);
    let cases = list(&golden()["levels"]);
    assert!(cases.len() > 200);
    for c in cases {
        let u = bits(&c["u"]);
        assert_eq!(
            level(u, factor).to_string(),
            str_of(&c["level"]),
            "u = {u:e}"
        );
    }
}

#[test]
fn indexes_match_go() {
    let cases = list(&golden()["indexes"]);
    assert_eq!(cases.len(), 14);
    for case in cases {
        if str_of(&case["elem"]) == "f32" {
            check_index(
                case,
                float_vectors(&case["vectors"]),
                float_vectors(&case["queries"]),
                l2,
            );
        } else {
            check_index(
                case,
                int_vectors(&case["vectors"]),
                int_vectors(&case["queries"]),
                jaccard,
            );
        }
    }
}

/// The blobs the unmarshal cases start from.
fn bases() -> HashMap<&'static str, Vec<u8>> {
    let small = case_named("hnsw small");
    let blob = hex::decode(str_of(&small["blob"])).expect("hex");
    let mut h: Hnsw<Vec<f32>> = Hnsw::new(l2);
    h.unmarshal(&mut blob.as_slice()).expect("unmarshal");
    let canonical = marshal(&h);
    assert_eq!(hash(&canonical), str_of(&small["blob_sha256"]));
    let empty = hex::decode(str_of(&golden()["empty_blob"])).expect("hex");
    HashMap::from([("blob", blob), ("canonical", canonical), ("empty", empty)])
}

/// Go returns an error, or panics where noted (`PORTING_NOTES.md` X-22), and
/// Rust returns an error.
fn check_unmarshal_error(name: &str, case: &Json, err: &Error) {
    if let Some(go) = case.get("error") {
        // binary.Read and ReadBytes report io.EOF or io.ErrUnexpectedEOF.
        assert!(
            matches!(str_of(go), "EOF" | "unexpected EOF"),
            "{name}: Go error {go}"
        );
        assert!(
            matches!(err, Error::Encoding(encoding::Error::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof),
            "{name}: {err:?}"
        );
    } else {
        let go = str_of(&case["panic"]);
        assert!(
            go.contains("makeslice: len out of range"),
            "{name}: Go panic {go}"
        );
        // A negative count is reported as such; a huge one reads on until
        // the data stops making sense, whatever the error is then.
        if name.starts_with("negative") {
            assert!(
                matches!(
                    err,
                    Error::NegativeCount(-1) | Error::Encoding(encoding::Error::NegativeLength(-2))
                ),
                "{name}: {err:?}"
            );
        }
    }
}

#[test]
fn unmarshal_matches_go() {
    let bases = bases();
    let small = case_named("hnsw small");
    let queries = float_vectors(&small["queries"]);
    let cases = list(&golden()["unmarshal"]);
    assert!(cases.len() > 150);
    for case in cases {
        let name = str_of(&case["name"]);
        let base = &bases[str_of(&case["base"])];
        let mut data = base
            [..usize::try_from(case["truncate"].as_u64().expect("truncate")).expect("truncate")]
            .to_vec();
        if let Some(patch) = case.get("patch") {
            let patch = hex::decode(str_of(patch)).expect("hex");
            let at = usize::try_from(case["offset"].as_u64().expect("offset")).expect("offset");
            data[at..at + patch.len()].copy_from_slice(&patch);
        }
        let (source, used) = replay(case.get("uniforms").unwrap_or(&Json::Null));
        let mut h = if case.get("deadlock").is_some() {
            // Go draws nothing before it hangs; any level gives the same graph.
            Hnsw::with_uniform(l2, || 0.5)
        } else {
            Hnsw::with_uniform(l2, source)
        };
        let before = marshal(&h);
        match h.unmarshal(&mut data.as_slice()) {
            Err(err) => {
                assert!(
                    case.get("error").is_some() || case.get("panic").is_some(),
                    "{name}: Go read it, Rust: {err:?}"
                );
                check_unmarshal_error(name, case, &err);
                // Unlike Go's, a failed unmarshal changes nothing.
                assert_eq!(marshal(&h), before, "{name}");
                continue;
            }
            Ok(()) => assert!(
                case.get("error").is_none() && case.get("panic").is_none(),
                "{name}: Go failed with {case}"
            ),
        }
        if let Some(add) = case.get("add") {
            let vectors = float_vectors(add);
            if case.get("deadlock").is_some() {
                // Go deadlocks: the first insert into an unmarshalled empty
                // index is its own enter point, and the insert read-locks the
                // vector it holds the write lock of (X-23). Here it links the
                // vector to itself.
                for v in vectors {
                    h.add(v);
                }
                assert_eq!(h.len(), 1);
                assert_eq!(h.bottom_neighbors[0].values(), vec![0], "{name}");
                assert_eq!(
                    h.search_vector(&vec![0.5, 0.25], 3, false),
                    vec![(0, 0.0)],
                    "{name}"
                );
                continue;
            }
            let adds: Vec<usize> = vectors.into_iter().map(|v| h.add(v)).collect();
            let expected: Vec<usize> = list(&case["adds"])
                .iter()
                .map(|a| usize::try_from(a.as_u64().expect("add")).expect("add"))
                .collect();
            assert_eq!(adds, expected, "{name}: adds");
            assert_eq!(
                *used.lock().expect("lock"),
                list(&case["uniforms"]).len(),
                "{name}: layer draws"
            );
            assert_eq!(
                hash(&marshal(&h)),
                str_of(&case["blob_sha256"]),
                "{name}: marshalled index"
            );
        }
        check_searches(name, &h, &queries, &case["searches"]);
    }
}

#[test]
fn bruteforce_add_returns_the_length_and_hnsw_the_index() {
    let mut bf = Bruteforce::new(l2);
    let mut h = Hnsw::with_uniform(l2, || 0.5);
    assert_eq!(bf.add(vec![0.0]), 1);
    assert_eq!(h.add(vec![0.0]), 0);
    assert_eq!(bf.add(vec![1.0]), 2);
    assert_eq!(h.add(vec![1.0]), 1);
    // HNSW finds the query vector itself; brute force leaves it out.
    assert_eq!(
        h.search_index(0, 2, false).expect("search"),
        vec![(0, 0.0), (1, 1.0)]
    );
    assert_eq!(
        bf.search_index(0, 2, false).expect("search"),
        vec![(1, 1.0)]
    );
    assert_eq!(h.search_index(0, 2, true).expect("search"), vec![(1, 1.0)]);
    assert_eq!(
        bf.search_index(2, 1, false)
            .expect_err("out of range")
            .to_string(),
        "index out of range: 2"
    );
    assert_eq!(
        h.search_index(2, 1, false)
            .expect_err("out of range")
            .to_string(),
        "index out of range: 2"
    );
}

#[test]
#[cfg(target_arch = "x86_64")]
fn level_of_a_zero_uniform_is_go_amd64_int_of_infinity() {
    // -Log(0) = +Inf; Go's int(+Inf) on amd64 is MinInt64.
    assert_eq!(level(0.0, 1.0 / math32::log(48.0)), i64::MIN);
}

/// Go on amd64, with `rand.Float32` replaced by a function returning 0, panics
/// with `runtime error: index out of range [-2]` on the second add: the
/// descent runs past layer 0 (`PORTING_NOTES.md` Q-58).
#[test]
#[cfg(target_arch = "x86_64")]
#[should_panic(expected = "index out of range [-2]")]
fn a_zero_uniform_makes_the_insert_panic_as_in_go() {
    let mut h = Hnsw::with_uniform(|a: &Vec<f32>, b: &Vec<f32>| a[0] - b[0], || 0.0);
    assert_eq!(h.add(vec![1.0]), 0);
    h.add(vec![2.0]);
}

#[test]
fn random_uniforms_are_in_the_unit_interval() {
    let mut u = random_uniform();
    let draws: Vec<f32> = (0..10_000).map(|_| u()).collect();
    assert!(draws.iter().all(|&x| (0.0..1.0).contains(&x)));
    let mean = draws.iter().map(|&x| f64::from(x)).sum::<f64>() / 10_000.0;
    assert!((mean - 0.5).abs() < 0.02, "{mean}");
}

/// splitmix64 floats in [-1, 1), for the recall checks below.
fn splitmix_floats(seed: u64) -> impl FnMut() -> f32 {
    let mut state = seed;
    move || {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        ((z >> 40) as i32 - (1 << 23)) as f32 / (1 << 23) as f32
    }
}

#[allow(clippy::ptr_arg)] // A `Distance<Vec<f32>>` takes `&Vec<f32>`.
fn euclidean(a: &Vec<f32>, b: &Vec<f32>) -> f32 {
    floats::euclidean(a, b)
}

/// `recall` of `ann_test.go`.
fn recall(gt: &[Neighbor], pred: &[Neighbor]) -> f64 {
    let s: HashSet<usize> = gt.iter().map(|p| p.0).collect();
    let hit = pred.iter().filter(|p| s.contains(&p.0)).count();
    hit as f64 / gt.len() as f64
}

/// The shape of `TestMNIST` on data this environment has: 2000 vectors in 20
/// clusters, 100 queries, recall of the 100 nearest against brute force. Not a
/// parity check (the golden tests are); the threshold is Go's MNIST one.
#[test]
fn hnsw_recall_on_clustered_vectors() {
    let mut next = splitmix_floats(7);
    let centers: Vec<Vec<f32>> = (0..20)
        .map(|_| (0..16).map(|_| next() * 4.0).collect())
        .collect();
    let mut point =
        |i: usize| -> Vec<f32> { centers[i % 20].iter().map(|c| c + next() * 0.5).collect() };
    let train: Vec<Vec<f32>> = (0..2000).map(&mut point).collect();
    let test: Vec<Vec<f32>> = (0..100).map(&mut point).collect();
    let mut levels = splitmix_floats(11);
    let mut bf = Bruteforce::new(euclidean);
    let mut h = Hnsw::with_uniform(euclidean, move || (levels() + 1.0) / 2.0);
    for v in &train {
        bf.add(v.clone());
        h.add(v.clone());
    }
    let r: f64 = test
        .iter()
        .map(|q| {
            let gt = bf.search_vector(q, 100, false);
            assert_eq!(gt.len(), 100);
            let scores = h.search_vector(q, 100, false);
            assert_eq!(scores.len(), 100);
            recall(&gt, &scores)
        })
        .sum::<f64>()
        / test.len() as f64;
    assert!(r > 0.99, "recall {r}");

    // Save and load.
    let mut copy = Hnsw::new(euclidean);
    copy.unmarshal(&mut marshal(&h).as_slice())
        .expect("unmarshal");
    for q in &test {
        assert_eq!(
            copy.search_vector(q, 100, false),
            h.search_vector(q, 100, false)
        );
    }
}

// The Go tests, which need the datasets Go's datautil downloads from
// cdn.gorse.io into ~/.gorse/dataset (PORTING_NOTES.md O-1). They read the
// files Go downloaded; WP-1-8's datautil port will fetch them itself.

const TRAIN_SIZE: usize = 6000;
const TEST_SIZE: usize = 1000;

fn dataset(name: &str) -> PathBuf {
    let home = std::env::var_os("HOME").expect("HOME");
    PathBuf::from(home)
        .join(".gorse")
        .join("dataset")
        .join(name)
}

struct Mnist {
    train_images: Vec<Vec<f32>>,
    test_images: Vec<Vec<f32>>,
}

fn read_libfm(path: PathBuf) -> Vec<Vec<f32>> {
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .map(|line| {
            let mut splits = line.split(' ');
            let _label: u8 = splits.next().expect("label").parse().expect("label");
            let mut image = vec![0.0; 784];
            for split in splits {
                let (index, value) = split.split_once(':').expect("index:value");
                image[index.parse::<usize>().expect("index")] = value.parse().expect("value");
            }
            image
        })
        .collect()
}

fn mnist() -> Mnist {
    let path = dataset("mnist");
    Mnist {
        train_images: read_libfm(path.join("train.libfm")),
        test_images: read_libfm(path.join("test.libfm")),
    }
}

#[test]
#[ignore = "needs the mnist dataset from cdn.gorse.io (PORTING_NOTES.md O-1)"]
fn test_mnist() {
    let dat = mnist();
    let mut bf = Bruteforce::new(euclidean);
    let mut hnsw = Hnsw::new(euclidean);
    for image in &dat.train_images[..TRAIN_SIZE] {
        bf.add(image.clone());
        hnsw.add(image.clone());
    }
    let search = |hnsw: &Hnsw<Vec<f32>>| {
        let mut r = 0.0;
        for image in &dat.test_images[..TEST_SIZE] {
            let gt = bf.search_vector(image, 100, false);
            assert_eq!(gt.len(), 100);
            let scores = hnsw.search_vector(image, 100, false);
            assert_eq!(scores.len(), 100);
            r += recall(&gt, &scores);
        }
        r / TEST_SIZE as f64
    };
    assert!(search(&hnsw) > 0.99);
    // Save and load.
    let path = std::env::temp_dir().join(format!("gorse-ann-mnist-{}.bin", std::process::id()));
    let mut f = std::fs::File::create(&path).expect("create");
    hnsw.marshal(&mut f).expect("marshal");
    drop(f);
    let mut f = std::fs::File::open(&path).expect("open");
    hnsw.unmarshal(&mut f).expect("unmarshal");
    std::fs::remove_file(&path).expect("remove");
    assert!(search(&hnsw) > 0.99);
}

#[test]
#[ignore = "needs the mnist dataset from cdn.gorse.io (PORTING_NOTES.md O-1)"]
fn test_multithread() {
    let dat = mnist();
    // Inserts take &mut self (X-21): the threads take turns, in whatever
    // order they get the lock, as Go's goroutines do.
    let hnsw = Mutex::new(Hnsw::new(euclidean));
    let indices: Vec<usize> = std::thread::scope(|s| {
        let handles: Vec<_> = dat.train_images[..TRAIN_SIZE]
            .iter()
            .map(|image| s.spawn(|| hnsw.lock().expect("lock").add(image.clone())))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("insert"))
            .collect()
    });
    let hnsw = hnsw.into_inner().expect("lock");
    let mut reverse = vec![0; TRAIN_SIZE];
    for (i, &index) in indices.iter().enumerate() {
        reverse[index] = i;
    }
    let mut bf = Bruteforce::new(euclidean);
    for &i in &reverse {
        bf.add(dat.train_images[i].clone());
    }
    let r: f64 = std::thread::scope(|s| {
        let handles: Vec<_> = dat.test_images[..TEST_SIZE]
            .iter()
            .map(|image| {
                let (bf, hnsw) = (&bf, &hnsw);
                s.spawn(move || {
                    let gt = bf.search_vector(image, 100, false);
                    assert_eq!(gt.len(), 100);
                    let scores = hnsw.search_vector(image, 100, false);
                    assert_eq!(scores.len(), 100);
                    recall(&gt, &scores)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("search")).sum()
    });
    assert!(r / TEST_SIZE as f64 > 0.99);
}

fn movie_lens() -> Vec<Vec<i64>> {
    let path = dataset("ml-1m").join("train.txt");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut movies: Vec<Vec<i64>> = Vec::new();
    for line in text.lines() {
        let mut splits = line.split('\t');
        let user_id: i64 = splits.next().expect("user").parse().expect("user");
        let movie_id: usize = splits.next().expect("movie").parse().expect("movie");
        while movie_id >= movies.len() {
            movies.push(Vec::new());
        }
        movies[movie_id].push(user_id);
    }
    movies
}

#[test]
#[ignore = "needs the ml-1m dataset from cdn.gorse.io (PORTING_NOTES.md O-1)"]
fn test_movie_lens() {
    let movies = movie_lens();
    let mut bf = Bruteforce::new(jaccard);
    let mut hnsw = Hnsw::new(jaccard);
    for movie in &movies {
        bf.add(movie.clone());
        hnsw.add(movie.clone());
    }
    let mut r = 0.0;
    for i in 0..TEST_SIZE {
        let gt = bf.search_index(i, 100, false).expect("search");
        let scores = hnsw.search_index(i, 100, false).expect("search");
        r += recall(&gt, &scores);
    }
    assert!(r / TEST_SIZE as f64 > 0.98);
}
