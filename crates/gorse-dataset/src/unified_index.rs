// Ports: dataset/unified_index.go @ add21b5

//! The unified encoding space of factorisation machines: users, items, user
//! labels, item labels and context labels, numbered one after another.

use std::borrow::Cow;
use std::io::{Read, Write};

use gorse_common::gostrconv;

use crate::Error;
use crate::index::{Index, NOT_ID, read_i32, write_i32};

/// The type tag of a [`UnifiedMapIndex`] in a marshalled unified index.
const MAP_INDEX: u8 = 0;
/// The type tag of a [`UnifiedDirectIndex`] in a marshalled unified index.
const DIRECT_INDEX: u8 = 1;
/// The type tag of a nil index in a marshalled unified index.
const NIL_INDEX: u8 = 2;

/// Go's `UnifiedIndex` interface, as the two types that implement it. A nil
/// index is `Option::None` where a `UnifiedIndex` is expected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnifiedIndex {
    /// A [`UnifiedMapIndex`].
    Map(Box<UnifiedMapIndex>),
    /// A [`UnifiedDirectIndex`].
    Direct(UnifiedDirectIndex),
}

impl UnifiedIndex {
    /// The size of the encoding space.
    pub fn len(&self) -> i32 {
        match self {
            Self::Map(index) => index.len(),
            Self::Direct(index) => index.len(),
        }
    }

    /// Whether the encoding space is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The encoding of user `user_id`, or [`NOT_ID`].
    pub fn encode_user(&self, user_id: &str) -> i32 {
        match self {
            Self::Map(index) => index.encode_user(user_id),
            Self::Direct(index) => index.encode_user(user_id),
        }
    }

    /// The encoding of item `item_id`, or [`NOT_ID`].
    pub fn encode_item(&self, item_id: &str) -> i32 {
        match self {
            Self::Map(index) => index.encode_item(item_id),
            Self::Direct(index) => index.encode_item(item_id),
        }
    }

    /// The encoding of user label `user_label`, or [`NOT_ID`].
    pub fn encode_user_label(&self, user_label: &str) -> i32 {
        match self {
            Self::Map(index) => index.encode_user_label(user_label),
            Self::Direct(index) => index.encode_user_label(user_label),
        }
    }

    /// The encoding of item label `item_label`, or [`NOT_ID`].
    pub fn encode_item_label(&self, item_label: &str) -> i32 {
        match self {
            Self::Map(index) => index.encode_item_label(item_label),
            Self::Direct(index) => index.encode_item_label(item_label),
        }
    }

    /// The encoding of context label `label`, or [`NOT_ID`].
    pub fn encode_context_label(&self, label: &str) -> i32 {
        match self {
            Self::Map(index) => index.encode_context_label(label),
            Self::Direct(index) => index.encode_context_label(label),
        }
    }

    /// All users (`GetUsers`).
    pub fn get_users(&self) -> Cow<'_, [String]> {
        match self {
            Self::Map(index) => Cow::Borrowed(index.get_users()),
            Self::Direct(index) => Cow::Owned(index.get_users()),
        }
    }

    /// All items (`GetItems`).
    pub fn get_items(&self) -> Cow<'_, [String]> {
        match self {
            Self::Map(index) => Cow::Borrowed(index.get_items()),
            Self::Direct(index) => Cow::Owned(index.get_items()),
        }
    }

    /// All user labels (`GetUserLabels`).
    pub fn get_user_labels(&self) -> Cow<'_, [String]> {
        match self {
            Self::Map(index) => Cow::Borrowed(index.get_user_labels()),
            Self::Direct(index) => Cow::Owned(index.get_user_labels()),
        }
    }

    /// All item labels (`GetItemLabels`).
    pub fn get_item_labels(&self) -> Cow<'_, [String]> {
        match self {
            Self::Map(index) => Cow::Borrowed(index.get_item_labels()),
            Self::Direct(index) => Cow::Owned(index.get_item_labels()),
        }
    }

    /// All context labels (`GetContextLabels`).
    pub fn get_context_labels(&self) -> Cow<'_, [String]> {
        match self {
            Self::Map(index) => Cow::Borrowed(index.get_context_labels()),
            Self::Direct(index) => Cow::Owned(index.get_context_labels()),
        }
    }

    /// The number of users (`CountUsers`).
    pub fn count_users(&self) -> i32 {
        match self {
            Self::Map(index) => index.count_users(),
            Self::Direct(index) => index.count_users(),
        }
    }

    /// The number of items (`CountItems`).
    pub fn count_items(&self) -> i32 {
        match self {
            Self::Map(index) => index.count_items(),
            Self::Direct(index) => index.count_items(),
        }
    }

    /// The number of user labels (`CountUserLabels`).
    pub fn count_user_labels(&self) -> i32 {
        match self {
            Self::Map(index) => index.count_user_labels(),
            Self::Direct(index) => index.count_user_labels(),
        }
    }

    /// The number of item labels (`CountItemLabels`).
    pub fn count_item_labels(&self) -> i32 {
        match self {
            Self::Map(index) => index.count_item_labels(),
            Self::Direct(index) => index.count_item_labels(),
        }
    }

    /// The number of context labels (`CountContextLabels`).
    pub fn count_context_labels(&self) -> i32 {
        match self {
            Self::Map(index) => index.count_context_labels(),
            Self::Direct(index) => index.count_context_labels(),
        }
    }
}

/// Go's `MarshalUnifiedIndex`: a type tag, then the index. A `None` index is
/// the nil tag alone.
pub fn marshal_unified_index(
    w: &mut impl Write,
    index: Option<&UnifiedIndex>,
) -> Result<(), Error> {
    let index = match index {
        None => {
            w.write_all(&[NIL_INDEX])?;
            return Ok(());
        }
        Some(index) => index,
    };
    match index {
        UnifiedIndex::Map(index) => {
            w.write_all(&[MAP_INDEX])?;
            index.marshal(w)
        }
        UnifiedIndex::Direct(index) => {
            w.write_all(&[DIRECT_INDEX])?;
            index.marshal(w)
        }
    }
}

/// Go's `UnmarshalUnifiedIndex`: reads a type tag, then the index it names. The
/// nil tag gives `None`. An unknown tag is an error.
pub fn unmarshal_unified_index(r: &mut impl Read) -> Result<Option<UnifiedIndex>, Error> {
    let mut tag = [0];
    r.read_exact(&mut tag)?;
    match tag[0] {
        MAP_INDEX => Ok(Some(UnifiedIndex::Map(Box::new(
            UnifiedMapIndex::unmarshal(r)?,
        )))),
        DIRECT_INDEX => Ok(Some(UnifiedIndex::Direct(UnifiedDirectIndex::unmarshal(
            r,
        )?))),
        NIL_INDEX => Ok(None),
        other => Err(Error::UnknownIndexType(other)),
    }
}

/// Go's `UnifiedMapIndexBuilder`: collects the names of each kind, then builds
/// the index.
#[derive(Debug, Clone, Default)]
pub struct UnifiedMapIndexBuilder {
    /// The users (`UserIndex`).
    pub user_index: Index,
    /// The items (`ItemIndex`).
    pub item_index: Index,
    /// The user labels (`UserLabelIndex`).
    pub user_label_index: Index,
    /// The item labels (`ItemLabelIndex`).
    pub item_label_index: Index,
    /// The context labels (`CtxLabelIndex`).
    pub ctx_label_index: Index,
}

impl UnifiedMapIndexBuilder {
    /// `NewUnifiedMapIndexBuilder`: an empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a user (`AddUser`).
    pub fn add_user(&mut self, user_id: &str) {
        self.user_index.add(user_id);
    }

    /// Adds an item (`AddItem`).
    pub fn add_item(&mut self, item_id: &str) {
        self.item_index.add(item_id);
    }

    /// Adds a user label (`AddUserLabel`).
    pub fn add_user_label(&mut self, user_label: &str) {
        self.user_label_index.add(user_label);
    }

    /// Adds an item label (`AddItemLabel`).
    pub fn add_item_label(&mut self, item_label: &str) {
        self.item_label_index.add(item_label);
    }

    /// Adds a context label (`AddCtxLabel`).
    pub fn add_ctx_label(&mut self, ctx_label: &str) {
        self.ctx_label_index.add(ctx_label);
    }

    /// `Build`: the index with the names collected so far.
    pub fn build(self) -> UnifiedIndex {
        UnifiedIndex::Map(Box::new(UnifiedMapIndex {
            user_index: self.user_index,
            item_index: self.item_index,
            user_label_index: self.user_label_index,
            item_label_index: self.item_label_index,
            ctx_label_index: self.ctx_label_index,
        }))
    }
}

/// Go's `UnifiedMapIndex`, the id to index mapper for factorization machines.
/// Its division of the encoding space is: users | items | user labels | item
/// labels | context labels.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnifiedMapIndex {
    /// The users (`UserIndex`).
    pub user_index: Index,
    /// The items (`ItemIndex`).
    pub item_index: Index,
    /// The user labels (`UserLabelIndex`).
    pub user_label_index: Index,
    /// The item labels (`ItemLabelIndex`).
    pub item_label_index: Index,
    /// The context labels (`CtxLabelIndex`).
    pub ctx_label_index: Index,
}

impl UnifiedMapIndex {
    /// All user labels (`GetUserLabels`).
    pub fn get_user_labels(&self) -> &[String] {
        self.user_label_index.get_names()
    }

    /// All item labels (`GetItemLabels`).
    pub fn get_item_labels(&self) -> &[String] {
        self.item_label_index.get_names()
    }

    /// All context labels (`GetContextLabels`).
    pub fn get_context_labels(&self) -> &[String] {
        self.ctx_label_index.get_names()
    }

    /// The number of user labels (`CountUserLabels`).
    pub fn count_user_labels(&self) -> i32 {
        self.user_label_index.len()
    }

    /// The number of item labels (`CountItemLabels`).
    pub fn count_item_labels(&self) -> i32 {
        self.item_label_index.len()
    }

    /// The number of context labels (`CountContextLabels`).
    pub fn count_context_labels(&self) -> i32 {
        self.ctx_label_index.len()
    }

    /// The size of the encoding space (`Len`).
    pub fn len(&self) -> i32 {
        self.user_index.len()
            + self.item_index.len()
            + self.user_label_index.len()
            + self.item_label_index.len()
            + self.ctx_label_index.len()
    }

    /// Whether the encoding space is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The encoding of user `user_id`, or [`NOT_ID`].
    pub fn encode_user(&self, user_id: &str) -> i32 {
        self.user_index.to_number(user_id)
    }

    /// The encoding of item `item_id`, or [`NOT_ID`].
    pub fn encode_item(&self, item_id: &str) -> i32 {
        let item_index = self.item_index.to_number(item_id);
        if item_index != NOT_ID {
            return item_index + self.user_index.len();
        }
        item_index
    }

    /// The encoding of user label `user_label`, or [`NOT_ID`].
    pub fn encode_user_label(&self, user_label: &str) -> i32 {
        let user_label_index = self.user_label_index.to_number(user_label);
        if user_label_index != NOT_ID {
            return user_label_index + self.user_index.len() + self.item_index.len();
        }
        user_label_index
    }

    /// The encoding of item label `item_label`, or [`NOT_ID`].
    pub fn encode_item_label(&self, item_label: &str) -> i32 {
        let item_label_index = self.item_label_index.to_number(item_label);
        if item_label_index != NOT_ID {
            return item_label_index
                + self.user_index.len()
                + self.item_index.len()
                + self.user_label_index.len();
        }
        item_label_index
    }

    /// The encoding of context label `label`, or [`NOT_ID`].
    pub fn encode_context_label(&self, label: &str) -> i32 {
        let ctx_label_index = self.ctx_label_index.to_number(label);
        if ctx_label_index != NOT_ID {
            return ctx_label_index
                + self.user_index.len()
                + self.item_index.len()
                + self.user_label_index.len()
                + self.item_label_index.len();
        }
        ctx_label_index
    }

    /// All users (`GetUsers`).
    pub fn get_users(&self) -> &[String] {
        self.user_index.get_names()
    }

    /// All items (`GetItems`).
    pub fn get_items(&self) -> &[String] {
        self.item_index.get_names()
    }

    /// The number of users (`CountUsers`).
    pub fn count_users(&self) -> i32 {
        self.user_index.len()
    }

    /// The number of items (`CountItems`).
    pub fn count_items(&self) -> i32 {
        self.item_index.len()
    }

    /// `Marshal`: the five indices in order, users first.
    pub fn marshal(&self, w: &mut impl Write) -> Result<(), Error> {
        for index in [
            &self.user_index,
            &self.item_index,
            &self.user_label_index,
            &self.item_label_index,
            &self.ctx_label_index,
        ] {
            index.marshal(w)?;
        }
        Ok(())
    }

    /// `Unmarshal`: reads the five indices that [`UnifiedMapIndex::marshal`]
    /// wrote.
    pub fn unmarshal(r: &mut impl Read) -> Result<Self, Error> {
        Ok(UnifiedMapIndex {
            user_index: Index::unmarshal(r)?,
            item_index: Index::unmarshal(r)?,
            user_label_index: Index::unmarshal(r)?,
            item_label_index: Index::unmarshal(r)?,
            ctx_label_index: Index::unmarshal(r)?,
        })
    }
}

/// Go's `UnifiedDirectIndex`: encodes every string as the integer it spells.
/// Its methods are for unit tests only. An encode method panics on a string
/// that is not an integer, as Go's does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnifiedDirectIndex {
    /// The size of the encoding space (`N`).
    pub n: i32,
}

impl UnifiedDirectIndex {
    /// `NewUnifiedDirectIndex`.
    pub fn new(n: i32) -> Self {
        UnifiedDirectIndex { n }
    }

    /// The encoding of user label `user_label`: the integer it spells.
    pub fn encode_user_label(&self, user_label: &str) -> i32 {
        direct_encoding(user_label)
    }

    /// The encoding of item label `item_label`: the integer it spells.
    pub fn encode_item_label(&self, item_label: &str) -> i32 {
        direct_encoding(item_label)
    }

    /// The encoding of context label `label`: the integer it spells.
    pub fn encode_context_label(&self, label: &str) -> i32 {
        direct_encoding(label)
    }

    /// The user labels: the names from `N/5*3` up to `N/5*4`.
    pub fn get_user_labels(&self) -> Vec<String> {
        self.names(self.n / 5 * 3, self.n / 5 * 4)
    }

    /// The item labels: the names from `N/5*2` up to `N/5*3`.
    pub fn get_item_labels(&self) -> Vec<String> {
        self.names(self.n / 5 * 2, self.n / 5 * 3)
    }

    /// The context labels: the names from `N/5*4` up to `N`.
    pub fn get_context_labels(&self) -> Vec<String> {
        self.names(self.n / 5 * 4, self.n)
    }

    /// The number of user labels, `N/5`.
    pub fn count_user_labels(&self) -> i32 {
        self.n / 5
    }

    /// The number of item labels, `N/5`.
    pub fn count_item_labels(&self) -> i32 {
        self.n / 5
    }

    /// The number of context labels, `N - N/5*4`.
    pub fn count_context_labels(&self) -> i32 {
        self.n - self.n / 5 * 4
    }

    /// The size of the encoding space (`Len`), `N`.
    pub fn len(&self) -> i32 {
        self.n
    }

    /// Whether the encoding space is empty.
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// The encoding of user `user_id`: the integer it spells.
    pub fn encode_user(&self, user_id: &str) -> i32 {
        direct_encoding(user_id)
    }

    /// The encoding of item `item_id`: the integer it spells.
    pub fn encode_item(&self, item_id: &str) -> i32 {
        direct_encoding(item_id)
    }

    /// The users: the names from `N/5` up to `N/5*2`.
    pub fn get_users(&self) -> Vec<String> {
        self.names(self.n / 5, self.n / 5 * 2)
    }

    /// The items: the names from `0` up to `N/5`. Go logs an empty warning
    /// here, and this does too.
    pub fn get_items(&self) -> Vec<String> {
        tracing::warn!("");
        self.names(0, self.n / 5)
    }

    /// The number of users, `N/5`.
    pub fn count_users(&self) -> i32 {
        self.n / 5
    }

    /// The number of items, `N/5`.
    pub fn count_items(&self) -> i32 {
        self.n / 5
    }

    /// `Marshal`: `N` as a little-endian `i32`.
    pub fn marshal(&self, w: &mut impl Write) -> Result<(), Error> {
        write_i32(w, self.n)
    }

    /// `Unmarshal`: reads the `N` that [`UnifiedDirectIndex::marshal`] wrote.
    pub fn unmarshal(r: &mut impl Read) -> Result<Self, Error> {
        Ok(UnifiedDirectIndex { n: read_i32(r)? })
    }

    /// The decimal names of the integers from `begin` up to `end`, excluding `end`.
    fn names(&self, begin: i32, end: i32) -> Vec<String> {
        (begin..end).map(|i| i.to_string()).collect()
    }
}

/// `strconv.Atoi` followed by `int32`, which panics with Go's error text when
/// the string is not an integer.
fn direct_encoding(s: &str) -> i32 {
    let (val, err) = gostrconv::atoi(s);
    match err {
        Some(err) => panic!("{err}"),
        None => val as i32,
    }
}

#[cfg(test)]
mod tests;
