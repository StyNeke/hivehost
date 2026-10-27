use manager_api::{auth::AuthStore, database};
use std::sync::{Arc, Barrier};

#[test]
fn schema_initialization_preserves_existing_accounts_and_rejects_future_versions() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.sqlite");
    let mut auth = AuthStore::open(&path).unwrap();
    auth.bootstrap_admin("admin", "a long test password")
        .unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 0)
        .unwrap();
    let db = database::open(&path).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM users", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert!(auth.login("admin", "a long test password", 100).is_ok());
    db.pragma_update(None, "user_version", 999).unwrap();
    assert!(database::open(&path).is_err());
    assert!(AuthStore::open(&path).is_err());
}

#[test]
fn simultaneous_first_connections_initialize_one_consistent_schema() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.sqlite");
    let barrier = Arc::new(Barrier::new(3));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                database::open(&path)
                    .unwrap()
                    .query_row("SELECT count(*) FROM jobs", [], |row| row.get::<_, i64>(0))
                    .unwrap()
            })
        })
        .collect();
    barrier.wait();
    for worker in workers {
        assert_eq!(worker.join().unwrap(), 0);
    }
}
