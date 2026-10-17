// Ports: github.com/lafikl/consistent/consistent.go @ add21b5 (v0.0.0-20220512074542-bdd3606bfc3e in go.mod)

//! The consistent-hash ring the workers split users with
//! (`contracts/grpc.md`, "User partitioning between workers"): every host
//! owns 10 points, the hashes of the host name followed by `0` to `9`, and a
//! key belongs to the host of the first point at or after the key's hash,
//! wrapping round to the first point. The hash is the first 8 bytes of
//! BLAKE2b-512, read little-endian.
//!
//! The bounded-load half of the Go package (`GetLeast`, `Inc`, `Done`,
//! `UpdateLoad`, `MaxLoad`) is ported too, with its quirks; Gorse itself only
//! calls `New`, `Add` and `Get` (`worker/worker.go`).
//!
//! ```
//! use gorse_common::consistent::Consistent;
//!
//! let mut c = Consistent::new();
//! // adds the hosts to the ring
//! c.add("127.0.0.1:8000");
//! c.add("92.0.0.1:8000");
//! // Returns the host that owns `key`.
//! let host = c.get("/app.html").expect("hosts were added");
//! assert!(host == "127.0.0.1:8000" || host == "92.0.0.1:8000");
//!
//! // Consistent hashing with bounded loads picks the least loaded host that
//! // can serve the key.
//! let host = c.get_least("/app.html").expect("hosts were added");
//! // increases the load of `host` before sending the request
//! c.inc(&host);
//! // call it when the work is done, to update the load of `host`.
//! c.done(&host);
//! ```

use std::collections::HashMap;

use blake2::{Blake2b512, Digest};

/// Points per host (Go `replicationFactor`).
const REPLICATION_FACTOR: usize = 10;

/// An error from the ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The ring has no hosts (Go `ErrNoHosts`).
    #[error("no hosts added")]
    NoHosts,
}

/// A consistent-hash ring with optional bounded loads (Go `Consistent`).
///
/// Go guards the ring with an internal `sync.RWMutex`; here mutation takes
/// `&mut self` and a caller sharing a ring wraps it in a lock
/// (`PORTING_NOTES.md` X-14).
#[derive(Debug, Default)]
pub struct Consistent {
    hosts: HashMap<u64, String>,
    sorted_set: Vec<u64>,
    load_map: HashMap<String, i64>,
    total_load: i64,
}

impl Consistent {
    /// `New`: an empty ring.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `Add`: places the host's 10 points on the ring, with a load of 0.
    /// Adding a host already present does nothing.
    pub fn add(&mut self, host: &str) {
        if self.load_map.contains_key(host) {
            return;
        }

        self.load_map.insert(host.to_owned(), 0);
        for i in 0..REPLICATION_FACTOR {
            let h = hash(&format!("{host}{i}"));
            self.hosts.insert(h, host.to_owned());
            self.sorted_set.push(h);
        }
        // sort hashes ascendingly
        self.sorted_set.sort_unstable();
    }

    /// `Get`: the host that owns `key`. That is `""` when the point found
    /// has lost its host to a hash collision (`PORTING_NOTES.md`
    /// Q-40).
    ///
    /// # Errors
    ///
    /// [`Error::NoHosts`] when the ring has no hosts.
    pub fn get(&self, key: &str) -> Result<&str, Error> {
        if self.hosts.is_empty() {
            return Err(Error::NoHosts);
        }

        let idx = self.search(hash(key));
        Ok(self.host_at(idx))
    }

    /// `GetLeast`: walks the ring from `key`'s point to the first host whose
    /// load is within the bound (consistent hashing with bounded loads).
    ///
    /// Takes `&mut self` because Go's `loadOK` resets a negative total load
    /// to 0 (`PORTING_NOTES.md` Q-42).
    ///
    /// # Errors
    ///
    /// [`Error::NoHosts`] when the ring has no hosts.
    ///
    /// # Panics
    ///
    /// Panics, as Go does, when it reaches a point that has lost its host to
    /// a hash collision.
    pub fn get_least(&mut self, key: &str) -> Result<String, Error> {
        if self.hosts.is_empty() {
            return Err(Error::NoHosts);
        }

        let mut i = self.search(hash(key));
        loop {
            let host = self.host_at(i).to_owned();
            if self.load_ok(&host) {
                return Ok(host);
            }
            i += 1;
            // Go wraps at the number of distinct points, not the length of
            // the sorted set; they differ only after a hash collision.
            if i >= self.hosts.len() {
                i = 0;
            }
        }
    }

    /// The host of the point at `idx` in the sorted set: Go's
    /// `c.hosts[c.sortedSet[idx]]`, which is `""` for a point whose host
    /// entry a hash collision removed.
    fn host_at(&self, idx: usize) -> &str {
        self.hosts
            .get(&self.sorted_set[idx])
            .map_or("", String::as_str)
    }

    /// Go `search`: the first point at or after `key`, wrapping to 0.
    fn search(&self, key: u64) -> usize {
        let idx = self.sorted_set.partition_point(|&p| p < key);
        if idx >= self.sorted_set.len() { 0 } else { idx }
    }

    /// `UpdateLoad`: sets the load of `host`; does nothing for a host not in
    /// the ring.
    pub fn update_load(&mut self, host: &str, load: i64) {
        let Some(current) = self.load_map.get_mut(host) else {
            return;
        };
        // Go's int64 arithmetic wraps.
        self.total_load = self.total_load.wrapping_sub(*current).wrapping_add(load);
        *current = load;
    }

    /// `Inc`: adds 1 to the load of `host`; does nothing for a host not in
    /// the ring.
    pub fn inc(&mut self, host: &str) {
        self.add_load(host, 1);
    }

    /// `Done`: subtracts 1 from the load of `host`; does nothing for a host
    /// not in the ring.
    pub fn done(&mut self, host: &str) {
        self.add_load(host, -1);
    }

    /// The body of `Inc` and `Done`.
    fn add_load(&mut self, host: &str, delta: i64) {
        let Some(load) = self.load_map.get_mut(host) else {
            return;
        };
        *load = load.wrapping_add(delta);
        self.total_load = self.total_load.wrapping_add(delta);
    }

    /// `Remove`: takes the host's points off the ring and forgets its load
    /// (but not its share of the total load, `PORTING_NOTES.md`
    /// Q-43). Always returns `true`, as Go does.
    pub fn remove(&mut self, host: &str) -> bool {
        for i in 0..REPLICATION_FACTOR {
            let h = hash(&format!("{host}{i}"));
            self.hosts.remove(&h);
            self.del_slice(h);
        }
        self.load_map.remove(host);
        true
    }

    /// `Hosts`: the hosts in the ring, in no particular order.
    #[must_use]
    pub fn hosts(&self) -> Vec<String> {
        self.load_map.keys().cloned().collect()
    }

    /// `GetLoads`: the load of every host.
    #[must_use]
    pub fn get_loads(&self) -> HashMap<String, i64> {
        self.load_map.clone()
    }

    /// `MaxLoad`: the most load one host may carry,
    /// `ceil(max(1, total / hosts) * 1.25)` with an integer division.
    ///
    /// Takes `&mut self` because Go's `MaxLoad` turns a total load of 0 into
    /// 1 (`PORTING_NOTES.md` Q-42).
    ///
    /// # Panics
    ///
    /// Panics when the ring has no hosts (a division by zero), as Go does.
    pub fn max_load(&mut self) -> i64 {
        if self.total_load == 0 {
            self.total_load = 1;
        }
        let mut avg_load_per_node = (self.total_load / self.load_map.len() as i64) as f64;
        if avg_load_per_node == 0.0 {
            avg_load_per_node = 1.0;
        }
        avg_load_per_node = (avg_load_per_node * 1.25).ceil();
        avg_load_per_node as i64
    }

    /// Go `loadOK`: whether `host` can take one more unit of load.
    fn load_ok(&mut self, host: &str) -> bool {
        // a safety check if someone performed c.Done more than needed
        if self.total_load < 0 {
            self.total_load = 0;
        }

        let mut avg_load_per_node =
            (self.total_load.wrapping_add(1) / self.load_map.len() as i64) as f64;
        if avg_load_per_node == 0.0 {
            avg_load_per_node = 1.0;
        }
        avg_load_per_node = (avg_load_per_node * 1.25).ceil();

        let Some(&load) = self.load_map.get(host) else {
            panic!("given host({host}) not in loadsMap");
        };

        load as f64 + 1.0 <= avg_load_per_node
    }

    /// Go `delSlice`: removes one occurrence of `val` from the sorted points.
    fn del_slice(&mut self, val: u64) {
        // Go's binary search, so that with repeated values the same one goes.
        let mut idx = None;
        let mut l: isize = 0;
        let mut r = self.sorted_set.len() as isize - 1;
        while l <= r {
            let m = (l + r) / 2;
            let item = self.sorted_set[m as usize];
            if item == val {
                idx = Some(m as usize);
                break;
            } else if item < val {
                l = m + 1;
            } else {
                r = m - 1;
            }
        }
        if let Some(idx) = idx {
            self.sorted_set.remove(idx);
        }
    }
}

/// Go `hash`: the first 8 bytes of BLAKE2b-512 of `key`, little-endian.
fn hash(key: &str) -> u64 {
    let out = Blake2b512::digest(key.as_bytes());
    let mut b = [0; 8];
    b.copy_from_slice(&out[..8]);
    u64::from_le_bytes(b)
}

#[cfg(test)]
mod tests {
    // Ports: github.com/lafikl/consistent consistent_test.go (example_test.go
    // has no assertions and is not run by `go test`; it is the module example)
    use super::*;

    #[test]
    fn test_add() {
        let mut c = Consistent::new();

        c.add("127.0.0.1:8000");
        assert_eq!(
            REPLICATION_FACTOR,
            c.sorted_set.len(),
            "vnodes number is incorrect"
        );
    }

    #[test]
    fn test_get() {
        let mut c = Consistent::new();

        c.add("127.0.0.1:8000");
        let host = c.get("127.0.0.1:8000").expect("get");

        assert_eq!("127.0.0.1:8000", host, "returned host is not what expected");
    }

    #[test]
    fn test_remove() {
        let mut c = Consistent::new();

        c.add("127.0.0.1:8000");
        c.remove("127.0.0.1:8000");

        // Go's condition, `&&` included.
        assert!(
            !(!c.sorted_set.is_empty() && !c.hosts.is_empty()),
            "remove is not working"
        );
    }

    #[test]
    fn test_get_least() {
        let mut c = Consistent::new();

        c.add("127.0.0.1:8000");
        c.add("92.0.0.1:8000");

        for _ in 0..100 {
            let host = c.get_least("92.0.0.1:80001").expect("get_least");
            c.inc(&host);
        }

        for (k, v) in c.get_loads() {
            let max = c.max_load();
            assert!(v <= max, "host {k} is overloaded. {v} > {max}");
        }
    }

    #[test]
    fn test_inc_done() {
        let mut c = Consistent::new();

        c.add("127.0.0.1:8000");
        c.add("92.0.0.1:8000");

        let host = c.get_least("92.0.0.1:80001").expect("get_least");

        c.inc(&host);
        assert_eq!(
            Some(&1),
            c.load_map.get(&host),
            "host {host} load should be 1"
        );

        c.done(&host);
        assert_eq!(
            Some(&0),
            c.load_map.get(&host),
            "host {host} load should be 0"
        );
    }

    #[test]
    fn test_hosts() {
        let hosts = ["127.0.0.1:8000", "92.0.0.1:8000"];

        let mut c = Consistent::new();
        for h in hosts {
            c.add(h);
        }

        let added_hosts = c.hosts();
        for h in hosts {
            assert!(added_hosts.iter().any(|ah| ah == h), "missing host {h}");
        }
        c.remove("127.0.0.1:8000");
    }

    #[test]
    fn test_del_slice() {
        let items: [u64; 16] = [
            0, 1, 2, 3, 5, 20, 22, 23, 25, 27, 28, 30, 35, 37, 1008, 1009,
        ];
        let deletes: [u64; 5] = [25, 37, 1009, 3, 100_000];

        let mut c = Consistent::new();
        c.sorted_set.extend_from_slice(&items);

        for val in deletes {
            c.del_slice(val);
        }

        for val in deletes {
            for &item in &c.sorted_set {
                assert_ne!(item, val, "{val} wasn't deleted");
            }
        }
    }

    // Behaviour the Go tests leave out.

    #[test]
    fn empty_ring_has_no_owner() {
        let mut c = Consistent::new();
        assert_eq!(Err(Error::NoHosts), c.get("k"));
        assert_eq!(Err(Error::NoHosts), c.get_least("k"));
        assert_eq!("no hosts added", Error::NoHosts.to_string());
    }

    #[test]
    fn del_slice_removes_only_present_values() {
        let mut c = Consistent::new();
        c.sorted_set = vec![1, 3, 5];
        c.del_slice(4);
        assert_eq!(vec![1, 3, 5], c.sorted_set);
        c.del_slice(1);
        assert_eq!(vec![3, 5], c.sorted_set);
    }
}

#[cfg(test)]
mod golden_tests {
    //! Replays the cases of `tests/data/consistent_golden.json`, written by
    //! Go with `go run ./contracts/tools/consistentgolden -out
    //! crates/gorse-common/tests/data/consistent_golden.json`: the ring hash,
    //! the ring's points after every `Add` and `Remove`, the owner of 2816
    //! keys on 14 rings, every result and load of the bounded-load calls, and
    //! what the ring does in states only a hash collision produces.
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use serde_json::Value as Json;

    use super::*;

    fn golden() -> Json {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/consistent_golden.json"
        );
        let text = std::fs::read_to_string(path).expect("golden file");
        serde_json::from_str(&text).expect("golden JSON")
    }

    fn list<'a>(v: &'a Json, what: &str) -> &'a [Json] {
        v.as_array()
            .unwrap_or_else(|| panic!("{what} is not a list"))
    }

    fn str_of(v: &Json) -> &str {
        v.as_str().expect("string")
    }

    fn u64_of(v: &Json) -> u64 {
        str_of(v).parse().expect("decimal u64")
    }

    fn strings(v: &Json) -> Vec<String> {
        list(v, "strings")
            .iter()
            .map(|s| str_of(s).to_owned())
            .collect()
    }

    fn sorted<T: Ord>(mut v: Vec<T>) -> Vec<T> {
        v.sort();
        v
    }

    fn points(c: &Consistent) -> Vec<(u64, String)> {
        sorted(c.hosts.iter().map(|(&h, s)| (h, s.clone())).collect())
    }

    fn loads(v: &Json) -> HashMap<String, i64> {
        v.as_object()
            .expect("loads")
            .iter()
            .map(|(k, v)| (k.clone(), v.as_i64().expect("load")))
            .collect()
    }

    #[test]
    fn consistent_golden_hashes() {
        let g = golden();
        let hashes = list(&g["hashes"], "hashes");
        assert!(!hashes.is_empty());
        for h in hashes {
            let key = str_of(&h["key"]);
            assert_eq!(u64_of(&h["hash"]), hash(key), "hash of {key:?}");
        }
    }

    #[test]
    fn consistent_golden_rings() {
        let g = golden();
        let keys = strings(&g["keys"]);
        let rings = list(&g["rings"], "rings");
        assert!(!rings.is_empty());
        for ring in rings {
            let name = str_of(&ring["name"]);
            let mut c = Consistent::new();
            for (i, op) in list(&ring["ops"], "ops").iter().enumerate() {
                let host = str_of(&op["host"]);
                match str_of(&op["op"]) {
                    "add" => c.add(host),
                    "remove" => assert_eq!(
                        op["result"].as_bool(),
                        Some(c.remove(host)),
                        "{name}: op {i} result"
                    ),
                    other => panic!("unknown op {other}"),
                }
                let want: Vec<u64> = list(&op["sorted_set"], "sorted_set")
                    .iter()
                    .map(u64_of)
                    .collect();
                assert_eq!(want, c.sorted_set, "{name}: sorted set after op {i}");
                let want: Vec<(u64, String)> = list(&op["points"], "points")
                    .iter()
                    .map(|p| (u64_of(&p[0]), str_of(&p[1]).to_owned()))
                    .collect();
                assert_eq!(want, points(&c), "{name}: points after op {i}");
                assert_eq!(
                    strings(&op["hosts"]),
                    sorted(c.hosts()),
                    "{name}: hosts after op {i}"
                );
            }
            let hosts = strings(&ring["hosts"]);
            assert_eq!(hosts, sorted(c.hosts()), "{name}: hosts");
            let owners = list(&ring["owners"], "owners");
            assert_eq!(keys.len(), owners.len(), "{name}: one owner per key");
            for (key, owner) in keys.iter().zip(owners) {
                let got = c.get(key);
                match usize::try_from(owner.as_i64().expect("owner")) {
                    Ok(i) => assert_eq!(Ok(hosts[i].as_str()), got, "{name}: owner of {key:?}"),
                    Err(_) => {
                        let err = got.expect_err("Go failed");
                        assert_eq!(str_of(&ring["get_error"]), err.to_string(), "{name}");
                    }
                }
            }
        }
    }

    /// Replays bounded-load operations on `c`, checking every result, the
    /// loads and the total load after each.
    fn replay(name: &str, c: &mut Consistent, ops: &[Json]) {
        for (i, op) in ops.iter().enumerate() {
            let host = op.get("host").map_or("", str_of);
            let want = op.get("result").map(str_of);
            let got = match str_of(&op["op"]) {
                "add" => {
                    c.add(host);
                    None
                }
                "remove" => Some(c.remove(host).to_string()),
                "get" => Some(match c.get(str_of(&op["key"])) {
                    Ok(host) => host.to_owned(),
                    Err(err) => format!("error: {err}"),
                }),
                "get_least" => Some(
                    match catch_unwind(AssertUnwindSafe(|| c.get_least(str_of(&op["key"])))) {
                        Ok(Ok(host)) => host,
                        Ok(Err(err)) => format!("error: {err}"),
                        Err(_) => "panic".to_owned(),
                    },
                ),
                "inc" => {
                    c.inc(host);
                    None
                }
                "done" => {
                    c.done(host);
                    None
                }
                "update_load" => {
                    c.update_load(host, op["load"].as_i64().expect("load"));
                    None
                }
                "max_load" => Some(match catch_unwind(AssertUnwindSafe(|| c.max_load())) {
                    Ok(n) => n.to_string(),
                    Err(_) => "panic".to_owned(),
                }),
                "loads" => None,
                other => panic!("unknown op {other}"),
            };
            match (want, got.as_deref()) {
                // Go panics with a runtime error ("integer divide by zero",
                // "nil pointer dereference"); Rust's panic text differs.
                (Some(w), Some("panic")) => {
                    assert!(w.starts_with("panic: "), "{name}: op {i}: Go returned {w}");
                }
                (w, g) => assert_eq!(w, g, "{name}: op {i} result"),
            }
            assert_eq!(
                loads(&op["loads"]),
                c.get_loads(),
                "{name}: loads after op {i}"
            );
            assert_eq!(
                op["total_load"].as_i64(),
                Some(c.total_load),
                "{name}: total load after op {i}"
            );
        }
    }

    #[test]
    fn consistent_golden_loads() {
        let g = golden();
        let cases = list(&g["loads"], "loads");
        assert!(!cases.is_empty());
        for case in cases {
            let mut c = Consistent::new();
            replay(str_of(&case["name"]), &mut c, list(&case["ops"], "ops"));
        }
    }

    fn sorted_set(v: &Json) -> Vec<u64> {
        list(v, "sorted_set").iter().map(u64_of).collect()
    }

    fn point_list(v: &Json) -> Vec<(u64, String)> {
        list(v, "points")
            .iter()
            .map(|p| (u64_of(&p[0]), str_of(&p[1]).to_owned()))
            .collect()
    }

    /// States only a 64-bit hash collision produces, written by Go straight
    /// into the ring's fields.
    #[test]
    fn consistent_golden_injected() {
        let g = golden();
        let cases = list(&g["injected"], "injected");
        assert!(!cases.is_empty());
        for case in cases {
            let name = str_of(&case["name"]);
            let init = &case["init"];
            let mut c = Consistent {
                hosts: point_list(&init["points"]).into_iter().collect(),
                sorted_set: sorted_set(&init["sorted_set"]),
                load_map: loads(&init["loads"]),
                total_load: init["total_load"].as_i64().expect("total load"),
            };
            replay(name, &mut c, list(&case["ops"], "ops"));
            assert_eq!(
                sorted_set(&case["sorted_set"]),
                c.sorted_set,
                "{name}: sorted set"
            );
            assert_eq!(point_list(&case["points"]), points(&c), "{name}: points");
            assert_eq!(loads(&case["loads"]), c.get_loads(), "{name}: loads");
        }
    }
}
