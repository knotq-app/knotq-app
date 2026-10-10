use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Item;

/// A scheme's lines, shared between copies of the scheme until one of them is
/// changed.
///
/// A workspace is copied far more often than it is edited: the app keeps a
/// read-only view beside the store's copy, a save and a sync each take a
/// snapshot, and a first sync keeps what every scheme held beforehand. With a
/// plain `Vec` each of those is another full copy of every line of text in the
/// workspace, held for as long as the copy lives. Here a clone is a reference
/// count, and the lines are only duplicated — for the one scheme, at the
/// moment it is first written to — when a copy actually diverges.
///
/// It behaves as the `Vec<Item>` it wraps: reads go straight through, and any
/// mutable access takes a private copy first if the lines are still shared.
#[derive(Clone, Debug, Default)]
pub struct ItemList(Arc<Vec<Item>>);

impl ItemList {
    pub fn new() -> Self {
        Self::default()
    }

    /// The lines as an owned vector, without copying them if nothing else
    /// shares them.
    pub fn into_vec(self) -> Vec<Item> {
        Arc::try_unwrap(self.0).unwrap_or_else(|shared| (*shared).clone())
    }

    /// Whether `self` and `other` are the same allocation — equal without
    /// looking, because neither has been written to since one was cloned from
    /// the other.
    pub fn shares_storage_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Deref for ItemList {
    type Target = Vec<Item>;

    fn deref(&self) -> &Vec<Item> {
        &self.0
    }
}

impl DerefMut for ItemList {
    fn deref_mut(&mut self) -> &mut Vec<Item> {
        Arc::make_mut(&mut self.0)
    }
}

impl PartialEq for ItemList {
    fn eq(&self, other: &Self) -> bool {
        self.shares_storage_with(other) || *self.0 == *other.0
    }
}

impl PartialEq<Vec<Item>> for ItemList {
    fn eq(&self, other: &Vec<Item>) -> bool {
        *self.0 == *other
    }
}

impl PartialEq<ItemList> for Vec<Item> {
    fn eq(&self, other: &ItemList) -> bool {
        *self == *other.0
    }
}

impl From<Vec<Item>> for ItemList {
    fn from(items: Vec<Item>) -> Self {
        Self(Arc::new(items))
    }
}

impl From<ItemList> for Vec<Item> {
    fn from(items: ItemList) -> Self {
        items.into_vec()
    }
}

impl FromIterator<Item> for ItemList {
    fn from_iter<I: IntoIterator<Item = Item>>(iter: I) -> Self {
        Vec::from_iter(iter).into()
    }
}

impl IntoIterator for ItemList {
    type Item = Item;
    type IntoIter = std::vec::IntoIter<Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.into_vec().into_iter()
    }
}

impl<'a> IntoIterator for &'a ItemList {
    type Item = &'a Item;
    type IntoIter = std::slice::Iter<'a, Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'a> IntoIterator for &'a mut ItemList {
    type Item = &'a mut Item;
    type IntoIter = std::slice::IterMut<'a, Item>;

    fn into_iter(self) -> Self::IntoIter {
        Arc::make_mut(&mut self.0).iter_mut()
    }
}

impl Serialize for ItemList {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ItemList {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<Item>::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clone_shares_the_lines_until_either_side_is_written() {
        let original: ItemList = vec![Item::new("one"), Item::new("two")].into();
        let mut copy = original.clone();
        assert!(copy.shares_storage_with(&original));

        copy[0].set_text("changed");

        assert!(!copy.shares_storage_with(&original));
        assert_eq!(original[0].text(), "one");
        assert_eq!(copy[0].text(), "changed");
        assert_eq!(copy[1], original[1]);
    }

    #[test]
    fn reading_through_a_mutable_reference_does_not_copy() {
        let original: ItemList = vec![Item::new("one")].into();
        let mut copy = original.clone();
        let through_mut = &mut copy;

        assert_eq!(through_mut.len(), 1);
        assert_eq!(through_mut.iter().count(), 1);

        assert!(copy.shares_storage_with(&original));
    }

    #[test]
    fn it_serializes_as_the_plain_list_it_wraps() {
        let items = vec![Item::new("one"), Item::new("two")];
        let list: ItemList = items.clone().into();

        let json = serde_json::to_string(&list).unwrap();

        assert_eq!(json, serde_json::to_string(&items).unwrap());
        assert_eq!(serde_json::from_str::<ItemList>(&json).unwrap(), list);
    }
}
