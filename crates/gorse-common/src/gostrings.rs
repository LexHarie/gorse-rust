// Ports: $GOROOT/src/strings/replace.go (Replacer, NewReplacer) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's strings package: Copyright 2011 The Go Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that
// can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's `strings.Replacer`, which gorm's naming strategy (table and column
//! names) and Gorse's Redis and xvec stores use.
//!
//! As in Go, [`Replacer::new`] picks one of four algorithms; all give the same
//! output, the first match in argument order at each position, without
//! overlapping matches. Go strings are bytes, so this works on bytes.

use std::io::{self, Write};

/// Go's `*strings.Replacer`.
#[derive(Debug, Clone)]
pub struct Replacer {
    r: Algorithm,
}

#[derive(Debug, Clone)]
enum Algorithm {
    Generic(Box<GenericReplacer>),
    SingleString(SingleStringReplacer),
    Byte(Box<[u8; 256]>),
    ByteString(Box<ByteStringReplacer>),
}

impl Replacer {
    /// Go's `strings.NewReplacer(oldnew...)`: pairs of old and new strings.
    /// (Go panics on an odd argument count; pairs cannot be odd.)
    pub fn new<O: AsRef<[u8]>, N: AsRef<[u8]>>(oldnew: &[(O, N)]) -> Replacer {
        let oldnew: Vec<(&[u8], &[u8])> = oldnew
            .iter()
            .map(|(o, n)| (o.as_ref(), n.as_ref()))
            .collect();
        Replacer { r: build(&oldnew) }
    }

    /// Go's `Replace`: a copy of s with all replacements performed.
    pub fn replace(&self, s: impl AsRef<[u8]>) -> Vec<u8> {
        let s = s.as_ref();
        let mut buf = Vec::with_capacity(s.len());
        // Writing to a Vec cannot fail.
        let _ = self.write_string(&mut buf, s);
        buf
    }

    /// [`Replacer::replace`] for UTF-8 input with UTF-8 replacements, which
    /// gives UTF-8 output.
    pub fn replace_str(&self, s: &str) -> String {
        String::from_utf8_lossy(&self.replace(s)).into_owned()
    }

    /// Go's `WriteString`: writes s to w with all replacements performed and
    /// returns the bytes written. On an error, the count is what was written
    /// before it.
    pub fn write_string<W: Write>(
        &self,
        w: &mut W,
        s: impl AsRef<[u8]>,
    ) -> (usize, io::Result<()>) {
        let s = s.as_ref();
        match &self.r {
            Algorithm::Generic(r) => r.write_string(w, s),
            Algorithm::SingleString(r) => r.write_string(w, s),
            Algorithm::Byte(r) => byte_write_string(r, w, s),
            Algorithm::ByteString(r) => r.write_string(w, s),
        }
    }

    /// The Go type of the algorithm `NewReplacer` picked (what Go's tests
    /// read through `Replacer.Replacer()` with `%T`).
    #[doc(hidden)]
    pub fn algorithm(&self) -> &'static str {
        match self.r {
            Algorithm::Generic(_) => "*strings.genericReplacer",
            Algorithm::SingleString(_) => "*strings.singleStringReplacer",
            Algorithm::Byte(_) => "*strings.byteReplacer",
            Algorithm::ByteString(_) => "*strings.byteStringReplacer",
        }
    }

    /// Go's `PrintTrie` from `export_test.go`, for the generic algorithm.
    #[doc(hidden)]
    pub fn print_trie(&self) -> Option<String> {
        match &self.r {
            Algorithm::Generic(g) => {
                let mut out = Vec::new();
                g.print_node(&g.root, 0, &mut out);
                Some(String::from_utf8_lossy(&out).into_owned())
            }
            _ => None,
        }
    }
}

fn build(oldnew: &[(&[u8], &[u8])]) -> Algorithm {
    if oldnew.len() == 1 && oldnew[0].0.len() > 1 {
        return Algorithm::SingleString(SingleStringReplacer {
            pattern: oldnew[0].0.to_vec(),
            value: oldnew[0].1.to_vec(),
        });
    }

    let mut all_new_bytes = true;
    for &(old, new) in oldnew {
        if old.len() != 1 {
            return Algorithm::Generic(Box::new(GenericReplacer::new(oldnew)));
        }
        if new.len() != 1 {
            all_new_bytes = false;
        }
    }

    if all_new_bytes {
        let mut r = [0u8; 256];
        for (i, b) in r.iter_mut().enumerate() {
            *b = i as u8;
        }
        // The first occurrence of old->new map takes precedence
        // over the others with the same old string.
        for &(old, new) in oldnew.iter().rev() {
            r[usize::from(old[0])] = new[0];
        }
        return Algorithm::Byte(Box::new(r));
    }

    let mut r = ByteStringReplacer {
        replacements: std::array::from_fn(|_| None),
    };
    // The first occurrence of old->new map takes precedence
    // over the others with the same old string.
    for &(old, new) in oldnew.iter().rev() {
        r.replacements[usize::from(old[0])] = Some(new.to_vec());
    }
    Algorithm::ByteString(Box::new(r))
}

/// Go's `trieNode`: a node in a lookup trie for prioritized key/value pairs.
#[derive(Debug, Clone, Default)]
struct TrieNode {
    // The value of the node's key/value pair; empty if not a complete key.
    value: Vec<u8>,
    // Positive if this node is a complete key (higher is more important),
    // zero otherwise.
    priority: usize,
    // One child: prefix and next. Several: table, indexed by the remapped
    // next byte.
    prefix: Vec<u8>,
    next: Option<Box<TrieNode>>,
    table: Option<Vec<Option<Box<TrieNode>>>>,
}

impl TrieNode {
    fn add(&mut self, key: &[u8], val: &[u8], priority: usize, r: &GenericReplacer) {
        if key.is_empty() {
            if self.priority == 0 {
                self.value = val.to_vec();
                self.priority = priority;
            }
            return;
        }

        if !self.prefix.is_empty() {
            // Need to split the prefix among multiple nodes.
            let mut n = 0; // length of the longest common prefix
            while n < self.prefix.len() && n < key.len() {
                if self.prefix[n] != key[n] {
                    break;
                }
                n += 1;
            }
            let next = self.next.as_mut().expect("a prefix node has a next node");
            if n == self.prefix.len() {
                next.add(&key[n..], val, priority, r);
            } else if n == 0 {
                // First byte differs, start a new lookup table here. Looking
                // up what is currently prefix[0] will lead to prefix_node, and
                // looking up key[0] will lead to key_node.
                let next = self.next.take().expect("a prefix node has a next node");
                let prefix_node = if self.prefix.len() == 1 {
                    next
                } else {
                    Box::new(TrieNode {
                        prefix: self.prefix[1..].to_vec(),
                        next: Some(next),
                        ..TrieNode::default()
                    })
                };
                let mut key_node = Box::<TrieNode>::default();
                let mut table = vec![None; r.table_size];
                table[usize::from(r.mapping[usize::from(self.prefix[0])])] = Some(prefix_node);
                self.prefix.clear();
                key_node.add(&key[1..], val, priority, r);
                table[usize::from(r.mapping[usize::from(key[0])])] = Some(key_node);
                self.table = Some(table);
            } else {
                // Insert new node after the common section of the prefix.
                let mut next = Box::new(TrieNode {
                    prefix: self.prefix[n..].to_vec(),
                    next: self.next.take(),
                    ..TrieNode::default()
                });
                self.prefix.truncate(n);
                next.add(&key[n..], val, priority, r);
                self.next = Some(next);
            }
        } else if let Some(table) = self.table.as_mut() {
            // Insert into existing table.
            let m = usize::from(r.mapping[usize::from(key[0])]);
            table[m]
                .get_or_insert_with(Box::default)
                .add(&key[1..], val, priority, r);
        } else {
            self.prefix = key.to_vec();
            let mut next = Box::<TrieNode>::default();
            next.add(b"", val, priority, r);
            self.next = Some(next);
        }
    }
}

/// Go's `genericReplacer`, the fully generic algorithm.
#[derive(Debug, Clone)]
struct GenericReplacer {
    root: TrieNode,
    // The number of unique key bytes: the size of a node's lookup table.
    table_size: usize,
    // Maps key bytes to a dense index for TrieNode.table; table_size for
    // bytes no key uses.
    mapping: [u8; 256],
}

impl GenericReplacer {
    fn new(oldnew: &[(&[u8], &[u8])]) -> GenericReplacer {
        let mut r = GenericReplacer {
            root: TrieNode::default(),
            table_size: 0,
            mapping: [0; 256],
        };
        // Find each byte used, then assign them each an index.
        for &(key, _) in oldnew {
            for &b in key {
                r.mapping[usize::from(b)] = 1;
            }
        }
        r.table_size = r.mapping.iter().map(|&b| usize::from(b)).sum();
        let mut index = 0u8;
        for b in r.mapping.iter_mut() {
            if *b == 0 {
                // (A table of all 256 bytes wraps to 0 here, as Go's byte
                // conversion does; no byte then maps to "unused".)
                *b = r.table_size as u8;
            } else {
                *b = index;
                index = index.wrapping_add(1);
            }
        }
        // Ensure root node uses a lookup table (for performance).
        let mut root = TrieNode {
            table: Some(vec![None; r.table_size]),
            ..TrieNode::default()
        };
        let n = oldnew.len() * 2;
        for (i, &(old, new)) in oldnew.iter().enumerate() {
            root.add(old, new, n - 2 * i, &r);
        }
        r.root = root;
        r
    }

    /// Go's `lookup`: the value and key length with the highest priority
    /// among the keys that prefix s.
    fn lookup<'a>(&'a self, mut s: &[u8], ignore_root: bool) -> Option<(&'a [u8], usize)> {
        let mut best_priority = 0;
        let mut found = None;
        let mut node = Some(&self.root);
        let mut n = 0;
        while let Some(t) = node {
            if t.priority > best_priority && !(ignore_root && std::ptr::eq(t, &self.root)) {
                best_priority = t.priority;
                found = Some((t.value.as_slice(), n));
            }

            if s.is_empty() {
                break;
            }
            if let Some(table) = &t.table {
                let index = usize::from(self.mapping[usize::from(s[0])]);
                if index == self.table_size {
                    break;
                }
                node = table[index].as_deref();
                s = &s[1..];
                n += 1;
            } else if !t.prefix.is_empty() && s.starts_with(&t.prefix) {
                n += t.prefix.len();
                s = &s[t.prefix.len()..];
                node = t.next.as_deref();
            } else {
                break;
            }
        }
        found
    }

    fn write_string<W: Write>(&self, w: &mut W, s: &[u8]) -> (usize, io::Result<()>) {
        let mut n = 0;
        let mut last = 0;
        let mut prev_match_empty = false;
        let mut i = 0;
        while i <= s.len() {
            // Fast path: s[i] is not a prefix of any pattern.
            if i != s.len() && self.root.priority == 0 {
                let index = usize::from(self.mapping[usize::from(s[i])]);
                let root_table = self.root.table.as_ref().expect("the root has a table");
                if index == self.table_size || root_table[index].is_none() {
                    i += 1;
                    continue;
                }
            }

            // Ignore the empty match iff the previous loop found the empty
            // match.
            let found = self.lookup(&s[i..], prev_match_empty);
            prev_match_empty = matches!(found, Some((_, 0)));
            if let Some((val, keylen)) = found {
                if let Err(e) = write_counted(w, &s[last..i], &mut n) {
                    return (n, Err(e));
                }
                if let Err(e) = write_counted(w, val, &mut n) {
                    return (n, Err(e));
                }
                i += keylen;
                last = i;
                continue;
            }
            i += 1;
        }
        if last != s.len()
            && let Err(e) = write_counted(w, &s[last..], &mut n)
        {
            return (n, Err(e));
        }
        (n, Ok(()))
    }

    /// Go's `printNode` from `export_test.go`.
    fn print_node(&self, t: &TrieNode, depth: usize, out: &mut Vec<u8>) {
        out.push(if t.priority > 0 { b'+' } else { b'-' });
        out.push(b'\n');

        if !t.prefix.is_empty() {
            out.extend(std::iter::repeat_n(b'.', depth));
            out.extend_from_slice(&t.prefix);
            let next = t.next.as_ref().expect("a prefix node has a next node");
            self.print_node(next, depth + t.prefix.len(), out);
        } else if let Some(table) = &t.table {
            for (b, &m) in self.mapping.iter().enumerate() {
                if usize::from(m) != self.table_size
                    && let Some(child) = &table[usize::from(m)]
                {
                    out.extend(std::iter::repeat_n(b'.', depth));
                    out.push(b as u8);
                    self.print_node(child, depth + 1, out);
                }
            }
        }
    }
}

/// Go's `singleStringReplacer`: one old string of more than one byte. Go
/// finds it with a Boyer-Moore `stringFinder`; any search for the leftmost
/// occurrence gives the same result.
#[derive(Debug, Clone)]
struct SingleStringReplacer {
    pattern: Vec<u8>,
    value: Vec<u8>,
}

impl SingleStringReplacer {
    fn find(&self, s: &[u8]) -> Option<usize> {
        s.windows(self.pattern.len())
            .position(|w| w == self.pattern)
    }

    fn write_string<W: Write>(&self, w: &mut W, s: &[u8]) -> (usize, io::Result<()>) {
        let mut n = 0;
        let mut i = 0;
        while let Some(m) = self.find(&s[i..]) {
            if let Err(e) = write_counted(w, &s[i..i + m], &mut n) {
                return (n, Err(e));
            }
            if let Err(e) = write_counted(w, &self.value, &mut n) {
                return (n, Err(e));
            }
            i += m + self.pattern.len();
        }
        let r = write_counted(w, &s[i..], &mut n);
        (n, r)
    }
}

/// Go's `byteReplacer.WriteString`: every old and new string is one byte.
fn byte_write_string<W: Write>(r: &[u8; 256], w: &mut W, s: &[u8]) -> (usize, io::Result<()>) {
    let mut n = 0;
    let mut last = 0;
    for (i, &b) in s.iter().enumerate() {
        let to = r[usize::from(b)];
        if to == b {
            continue;
        }
        if last != i
            && let Err(e) = write_counted(w, &s[last..i], &mut n)
        {
            return (n, Err(e));
        }
        last = i + 1;
        if let Err(e) = write_counted(w, &[to], &mut n) {
            return (n, Err(e));
        }
    }
    if last != s.len()
        && let Err(e) = write_counted(w, &s[last..], &mut n)
    {
        return (n, Err(e));
    }
    (n, Ok(()))
}

/// Go's `byteStringReplacer`: every old string is one byte, the new ones
/// vary in length.
#[derive(Debug, Clone)]
struct ByteStringReplacer {
    // Replacement bytes indexed by old byte; None for bytes kept as they are.
    replacements: [Option<Vec<u8>>; 256],
}

impl ByteStringReplacer {
    fn write_string<W: Write>(&self, w: &mut W, s: &[u8]) -> (usize, io::Result<()>) {
        let mut n = 0;
        let mut last = 0;
        for (i, &b) in s.iter().enumerate() {
            let Some(rep) = &self.replacements[usize::from(b)] else {
                continue;
            };
            if last != i
                && let Err(e) = write_counted(w, &s[last..i], &mut n)
            {
                return (n, Err(e));
            }
            last = i + 1;
            if let Err(e) = write_counted(w, rep, &mut n) {
                return (n, Err(e));
            }
        }
        if last != s.len()
            && let Err(e) = write_counted(w, &s[last..], &mut n)
        {
            return (n, Err(e));
        }
        (n, Ok(()))
    }
}

/// Writes all of b, adding to n what was written (Go's `WriteString` on a
/// writer counts the bytes the writer reports; a failing write reports 0).
fn write_counted<W: Write>(w: &mut W, b: &[u8], n: &mut usize) -> io::Result<()> {
    w.write_all(b)?;
    *n += b.len();
    Ok(())
}

/// Go's `strings.ReplaceAll(s, old, new)` for a non-empty old string.
pub fn replace_all(s: &[u8], old: &[u8], new: &[u8]) -> Vec<u8> {
    assert!(!old.is_empty(), "replace_all with an empty old string");
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while let Some(m) = s[i..].windows(old.len()).position(|w| w == old) {
        out.extend_from_slice(&s[i..i + m]);
        out.extend_from_slice(new);
        i += m + old.len();
    }
    out.extend_from_slice(&s[i..]);
    out
}
