use manager_api::{auth::AuthStore, servers::ServerStore};
use manager_core::model::ServerId;
use uuid::Uuid;

#[test]
fn disabled_members_see_only_summaries_and_owner_admin_keep_access() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.sqlite");
    let mut auth = AuthStore::open(&path).unwrap();
    let admin = auth
        .bootstrap_admin("admin", "a long test password")
        .unwrap();
    let invite = auth.invite(&admin, 100).unwrap();
    let owner = auth
        .register(&invite, "owner", "a long test password", 100)
        .unwrap();
    let invite = auth.invite(&admin, 100).unwrap();
    let friend = auth
        .register(&invite, "friend", "a long test password", 100)
        .unwrap();
    let mut store = ServerStore::open(&path).unwrap();
    let server = ServerId(Uuid::new_v4());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "INSERT INTO servers (id, name, owner_id) VALUES (?1, 'Minecraft', ?2)",
        rusqlite::params![server.0.to_string(), owner.id.0.to_string()],
    )
    .unwrap();
    let list = store.list(friend.id).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "Minecraft");
    assert!(!list[0].can_access);
    assert!(list[0].usage.is_none());
    let json = serde_json::to_value(&list[0]).unwrap();
    assert!(json.get("owner").is_none());
    assert!(store.require_access(friend.id, server).is_err());
    assert!(store.require_access(owner.id, server).is_ok());
    assert!(store.require_access(admin.id, server).is_ok());
    store.set_member(owner.id, server, friend.id, true).unwrap();
    assert!(store.require_access(friend.id, server).is_ok());
    assert!(
        store
            .set_member(friend.id, server, owner.id, false)
            .is_err()
    );
    assert!(
        store
            .set_member(friend.id, server, admin.id, false)
            .is_err()
    );
    store
        .set_member(friend.id, server, friend.id, false)
        .unwrap();
    assert!(store.require_access(friend.id, server).is_err());
    assert!(store.set_member(friend.id, server, owner.id, true).is_err());
}
