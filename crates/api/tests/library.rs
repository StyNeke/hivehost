use manager_api::{auth::AuthStore, eggs::EggStore};
use serde_json::{Value, json};

const JAVA: &[u8] = include_bytes!("../../../tests/fixtures/eggs/java.json");

#[test]
fn members_import_shared_eggs_and_revisions_preserve_originals() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.sqlite");
    let mut auth = AuthStore::open(&path).unwrap();
    let admin = auth
        .bootstrap_admin("admin", "a long test password")
        .unwrap();
    let invite = auth.invite(&admin, 100).unwrap();
    let friend = auth
        .register(&invite, "friend", "a long test password", 101)
        .unwrap();
    let mut library = EggStore::open(&path).unwrap();
    let first = library.import(friend.id, JAVA).unwrap();
    assert!(!first.runtime_qualified);
    assert_eq!(EggStore::open(&path).unwrap().list().unwrap().len(), 1);
    let mut changed: Value = serde_json::from_slice(JAVA).unwrap();
    changed["name"] = json!("Updated Paper");
    let second = library
        .revise(friend.id, first.id, &serde_json::to_vec(&changed).unwrap())
        .unwrap();
    assert_ne!(first.revision, second.revision);
    let original = library.revision(first.revision).unwrap();
    assert_eq!(original.raw, JAVA);
    assert_eq!(original.normalized.name, "Paper");
    assert_eq!(library.list().unwrap()[0].name, "Updated Paper");
    let admin_revision = library.revise(admin.id, first.id, JAVA).unwrap();
    assert_eq!(
        library
            .revision(admin_revision.revision)
            .unwrap()
            .normalized
            .name,
        "Paper"
    );
}

#[test]
fn one_member_cannot_modify_another_members_shared_egg() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.sqlite");
    let mut auth = AuthStore::open(&path).unwrap();
    let admin = auth
        .bootstrap_admin("admin", "a long test password")
        .unwrap();
    let invite = auth.invite(&admin, 100).unwrap();
    let friend = auth
        .register(&invite, "friend", "a long test password", 101)
        .unwrap();
    let mut library = EggStore::open(&path).unwrap();
    let egg = library.import(admin.id, JAVA).unwrap();
    assert!(library.revise(friend.id, egg.id, JAVA).is_err());
    assert!(library.import(friend.id, b"invalid JSON").is_err());
    assert_eq!(library.list().unwrap().len(), 1);
}
