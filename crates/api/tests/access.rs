use manager_api::auth::AuthStore;
use std::sync::{Arc, Barrier};

const PASSWORD: &str = "correct horse battery staple";

#[test]
fn invitation_is_required_single_use_and_not_an_admin_grant() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.sqlite");
    let mut auth = AuthStore::open(&path).unwrap();
    let admin = auth.bootstrap_admin("stijn", PASSWORD).unwrap();
    assert!(admin.is_admin);
    assert!(auth.bootstrap_admin("another", PASSWORD).is_err());
    assert!(
        auth.register("random-token", "friend", PASSWORD, 100)
            .is_err()
    );
    let invite = auth.invite(&admin, 100).unwrap();
    let member = auth.register(&invite, "friend", PASSWORD, 101).unwrap();
    assert!(!member.is_admin);
    assert!(auth.register(&invite, "other", PASSWORD, 102).is_err());
    assert!(auth.invite(&member, 103).is_err());
    let expired = auth.invite(&admin, 100).unwrap();
    assert!(auth.register(&expired, "late", PASSWORD, 86500).is_err());
}

#[test]
fn simultaneous_registration_consumes_invitation_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.sqlite");
    let mut auth = AuthStore::open(&path).unwrap();
    let admin = auth.bootstrap_admin("stijn", PASSWORD).unwrap();
    let invite = auth.invite(&admin, 100).unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = (0..2)
        .map(|n| {
            let mut auth = AuthStore::open(&path).unwrap();
            let invite = invite.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                auth.register(&invite, &format!("friend{n}"), PASSWORD, 101)
                    .is_ok()
            })
        })
        .collect();
    barrier.wait();
    assert_eq!(
        handles
            .into_iter()
            .map(|h| usize::from(h.join().unwrap()))
            .sum::<usize>(),
        1
    );
}

#[test]
fn sessions_expire_logout_revokes_and_passwords_are_not_stored_in_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.sqlite");
    let mut auth = AuthStore::open(&path).unwrap();
    let admin = auth.bootstrap_admin("stijn", PASSWORD).unwrap();
    assert!(auth.login("stijn", "incorrect password", 100).is_err());
    assert!(auth.login("unknown", PASSWORD, 100).is_err());
    let token = auth.login("STIJN", PASSWORD, 100).unwrap();
    assert_eq!(
        auth.authenticate(&token, 101).unwrap().unwrap().id,
        admin.id
    );
    assert!(
        auth.authenticate(&token, 100 + 7 * 86400)
            .unwrap()
            .is_none()
    );
    auth.logout(&token).unwrap();
    assert!(auth.authenticate(&token, 102).unwrap().is_none());
    let db = rusqlite::Connection::open(&path).unwrap();
    let hash: String = db
        .query_row("SELECT password_hash FROM users", [], |row| row.get(0))
        .unwrap();
    assert!(hash.starts_with("$argon2id$"));
    assert!(!hash.contains(PASSWORD));
    let invite = auth.invite(&admin, 100).unwrap();
    let stored: String = db
        .query_row("SELECT token_hash FROM invites", [], |row| row.get(0))
        .unwrap();
    assert_ne!(stored, invite);
    assert!(
        auth.register(&invite, "invalid user", PASSWORD, 101)
            .is_err()
    );
    assert!(auth.register(&invite, "valid", "short", 101).is_err());
    assert!(auth.register(&invite, "valid", PASSWORD, 101).is_ok());
}
