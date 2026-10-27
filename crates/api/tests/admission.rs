use manager_api::{
    auth::AuthStore,
    eggs::EggStore,
    provisioning::{
        CreateServer, HostAdmission, PortBinding, ProvisioningError, ProvisioningStore, Transport,
    },
    servers::ServerStore,
};
use manager_core::{
    capacity::Resources,
    model::{EggRevisionId, UserId},
};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Barrier},
};
use uuid::Uuid;

fn resources(n: u64) -> Resources {
    Resources {
        memory_bytes: n,
        disk_bytes: n,
        cpu_millis: n as u32,
    }
}
fn binding(port: u16) -> PortBinding {
    PortBinding {
        ip: "100.116.183.201".parse().unwrap(),
        port,
        transport: Transport::Tcp,
    }
}

fn setup(path: &Path) -> (UserId, UserId, EggRevisionId, HostAdmission) {
    let mut auth = AuthStore::open(path).unwrap();
    let admin = auth
        .bootstrap_admin("admin", "a long test password")
        .unwrap();
    let invite = auth.invite(&admin, 100).unwrap();
    let member = auth
        .register(&invite, "friend", "a long test password", 100)
        .unwrap();
    let revision = EggStore::open(path)
        .unwrap()
        .import(
            member.id,
            include_bytes!("../../../tests/fixtures/eggs/java.json"),
        )
        .unwrap()
        .revision;
    let host = HostAdmission {
        measured_at: 100,
        budget: resources(10),
        legacy_reserved: resources(2),
        disk_quotas_ready: true,
        qualified_revisions: vec![revision],
        private_ips: vec![binding(25600).ip],
        unavailable_bindings: vec![],
    };
    (admin.id, member.id, revision, host)
}

fn request(revision: EggRevisionId, allocation_id: Uuid) -> CreateServer {
    CreateServer {
        name: "Minecraft".into(),
        egg_revision: revision,
        limits: resources(5),
        allocation_id,
        image: "ghcr.io/ptero-eggs/yolks:java_21".into(),
        variables: BTreeMap::new(),
        license_accepted: true,
    }
}

#[test]
fn simultaneous_creates_cannot_spend_the_same_remaining_budget() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.sqlite");
    let (admin, member, revision, host) = setup(&path);
    let mut store = ProvisioningStore::open(&path).unwrap();
    let allocations: Vec<_> = [25600, 25601]
        .into_iter()
        .map(|port| {
            store
                .add_allocation(admin, binding(port), &host, 100)
                .unwrap()
        })
        .collect();
    let barrier = Arc::new(Barrier::new(3));
    let workers: Vec<_> = allocations
        .into_iter()
        .map(|allocation| {
            let mut store = ProvisioningStore::open(&path).unwrap();
            let host = host.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                store.create(member, request(revision, allocation), &host, 101)
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .any(|r| matches!(r, Err(ProvisioningError::Capacity(_))))
    );
    let db = rusqlite::Connection::open(&path).unwrap();
    for table in [
        "servers",
        "server_provisioning",
        "server_allocations",
        "jobs",
    ] {
        assert_eq!(
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    let created = results.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(
        ServerStore::open(&path)
            .unwrap()
            .require_access(member, created.id)
            .unwrap()
            .owner,
        member
    );
    assert_eq!(created.egg_revision, revision);
    let pin = db
        .query_row("SELECT egg_revision_id FROM server_provisioning", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap();
    assert_eq!(pin, revision.0.to_string());
}

#[test]
fn stale_or_unqualified_host_and_bad_settings_leave_no_partial_server() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.sqlite");
    let (admin, member, revision, host) = setup(&path);
    let mut store = ProvisioningStore::open(&path).unwrap();
    let allocation = store
        .add_allocation(admin, binding(25600), &host, 100)
        .unwrap();
    let mut wrong_host = host.clone();
    wrong_host.disk_quotas_ready = false;
    assert!(matches!(
        store.create(member, request(revision, allocation), &wrong_host, 101),
        Err(ProvisioningError::HostNotReady(_))
    ));
    assert!(matches!(
        store.create(member, request(revision, allocation), &host, 131),
        Err(ProvisioningError::HostNotReady(_))
    ));
    wrong_host = host.clone();
    wrong_host.qualified_revisions.clear();
    assert!(
        store
            .create(member, request(revision, allocation), &wrong_host, 101)
            .is_err()
    );
    let mut invalid = request(revision, allocation);
    invalid
        .variables
        .insert("SERVER_JARFILE".into(), "../escape.jar".into());
    assert!(store.create(member, invalid, &host, 101).is_err());
    let mut invalid = request(revision, allocation);
    invalid.image = "arbitrary-image".into();
    assert!(store.create(member, invalid, &host, 101).is_err());
    let mut invalid = request(revision, allocation);
    invalid.license_accepted = false;
    assert!(store.create(member, invalid, &host, 101).is_err());
    let mut invalid = request(revision, allocation);
    invalid.variables.insert(
        "DL_PATH".into(),
        "https://unexpected.test/server.jar".into(),
    );
    assert!(store.create(member, invalid, &host, 101).is_err());
    assert!(
        ServerStore::open(&path)
            .unwrap()
            .list(member)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .create(member, request(revision, allocation), &host, 101)
            .is_ok()
    );
}

#[test]
fn pools_require_admin_owned_ips_and_conflict_checks_then_claim_ports_once() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.sqlite");
    let (admin, member, revision, mut host) = setup(&path);
    let mut store = ProvisioningStore::open(&path).unwrap();
    assert!(
        store
            .add_allocation(member, binding(25600), &host, 100)
            .is_err()
    );
    let mut foreign = binding(25600);
    foreign.ip = "192.0.2.5".parse().unwrap();
    assert!(store.add_allocation(admin, foreign, &host, 100).is_err());
    host.unavailable_bindings.push(PortBinding {
        ip: "0.0.0.0".parse().unwrap(),
        port: 25600,
        transport: Transport::Tcp,
    });
    assert!(
        store
            .add_allocation(admin, binding(25600), &host, 100)
            .is_err()
    );
    host.unavailable_bindings.clear();
    let allocation = store
        .add_allocation(admin, binding(25600), &host, 100)
        .unwrap();
    assert!(
        store
            .add_allocation(admin, binding(25600), &host, 100)
            .is_err()
    );
    store
        .create(member, request(revision, allocation), &host, 101)
        .unwrap();
    let mut smaller = request(revision, allocation);
    smaller.limits = resources(1);
    assert!(matches!(
        store.create(member, smaller, &host, 101),
        Err(ProvisioningError::AllocationUnavailable)
    ));
}
