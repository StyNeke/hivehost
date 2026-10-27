use manager_api::jobs::JobStore;
use manager_core::model::JobKind;
use std::sync::{Arc, Barrier};

#[test]
fn concurrent_claimers_get_one_job_and_expired_lease_keeps_operation_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jobs.sqlite");
    let mut store = JobStore::open(&path).unwrap();
    let id = store.enqueue(JobKind::Install).unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = (0..2)
        .map(|n| {
            let mut connection = JobStore::open(&path).unwrap();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                connection
                    .claim_job(&format!("worker-{n}"), 100, 30)
                    .unwrap()
            })
        })
        .collect();
    barrier.wait();
    let claims: Vec<_> = handles
        .into_iter()
        .filter_map(|h| h.join().unwrap())
        .collect();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].id, id);
    assert_eq!(claims[0].kind, JobKind::Install);
    assert_eq!(claims[0].attempt, 1);
    assert!(store.claim_job("recovery", 129, 30).unwrap().is_none());
    drop(store);
    let mut store = JobStore::open(&path).unwrap();
    let recovered = store.claim_job("recovery", 130, 30).unwrap().unwrap();
    assert_eq!(recovered.id, id);
    assert_eq!(recovered.attempt, 2);
    assert!(!store.complete(&claims[0], "worker-0", 131).unwrap());
    assert!(!store.complete(&recovered, "wrong-worker", 131).unwrap());
    assert!(store.complete(&recovered, "recovery", 131).unwrap());
    assert!(store.claim_job("next", 200, 30).unwrap().is_none());
}

#[test]
fn invalid_lease_cannot_claim_or_complete_expired_work() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = JobStore::open(&dir.path().join("jobs.sqlite")).unwrap();
    store.enqueue(JobKind::Backup).unwrap();
    assert!(store.claim_job("", 100, 30).is_err());
    assert!(store.claim_job("worker", 100, 0).is_err());
    assert!(store.claim_job("worker", i64::MAX, 30).is_err());
    let job = store.claim_job("worker", 100, 30).unwrap().unwrap();
    assert!(!store.complete(&job, "worker", 130).unwrap());
}

#[test]
fn renew_requires_current_unexpired_worker_and_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = JobStore::open(&dir.path().join("jobs.sqlite")).unwrap();
    store.enqueue(JobKind::Update).unwrap();
    let job = store.claim_job("worker", 100, 30).unwrap().unwrap();
    assert!(!store.renew(&job, "other", 110, 30).unwrap());
    assert!(store.renew(&job, "worker", 120, 30).unwrap());
    assert!(store.claim_job("other", 130, 30).unwrap().is_none());
    assert!(!store.renew(&job, "worker", 150, 30).unwrap());
    let retry = store.claim_job("worker", 150, 30).unwrap().unwrap();
    assert!(!store.renew(&job, "worker", 151, 30).unwrap());
    assert!(store.complete(&retry, "worker", 151).unwrap());
}
