use knotq_model::{FolderId, ItemId, SchemeId};
use std::str::FromStr;

#[test]
fn id_round_trips_through_string() {
    let folder = FolderId::new();
    let scheme = SchemeId::new();
    let item = ItemId::new();
    assert_eq!(FolderId::from_str(&folder.to_string()).unwrap(), folder);
    assert_eq!(SchemeId::from_str(&scheme.to_string()).unwrap(), scheme);
    assert_eq!(ItemId::from_str(&item.to_string()).unwrap(), item);
}
