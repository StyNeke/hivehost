use manager_core::{
    backup_policy::BackupPolicy,
    capacity::{CapacityError, Resources, admit},
    model::{Server, ServerId, User, UserId},
    permissions::{can_change_member, can_manage},
};
use uuid::Uuid;

fn user(admin: bool) -> User {
    User {
        id: UserId(Uuid::new_v4()),
        is_admin: admin,
    }
}

#[test]
fn owner_admin_and_enabled_members_have_full_control() {
    let owner = user(false);
    let admin = user(true);
    let member = user(false);
    let server = Server {
        id: ServerId(Uuid::new_v4()),
        owner: owner.id,
    };
    assert!(can_manage(&owner, &server, false));
    assert!(can_manage(&admin, &server, false));
    assert!(can_manage(&member, &server, true));
    assert!(!can_manage(&member, &server, false));
    assert!(!can_change_member(&member, &server, true, &owner, false));
    assert!(!can_change_member(&owner, &server, false, &admin, false));
    assert!(can_change_member(
        &member,
        &server,
        true,
        &user(false),
        false
    ));
    assert!(!can_change_member(
        &member,
        &server,
        false,
        &user(false),
        true
    ));
}

fn resources(n: u64) -> Resources {
    Resources {
        memory_bytes: n,
        disk_bytes: n,
        cpu_millis: n as u32,
    }
}

#[test]
fn capacity_checks_every_dimension_and_rejects_overflow() {
    assert_eq!(admit(resources(4), resources(10), resources(6)), Ok(()));
    assert_eq!(
        admit(resources(5), resources(10), resources(6)),
        Err(CapacityError::Memory)
    );
    let mut request = resources(1);
    request.disk_bytes = 5;
    assert_eq!(
        admit(request, resources(10), resources(6)),
        Err(CapacityError::Disk)
    );
    request.disk_bytes = 1;
    request.cpu_millis = 5;
    assert_eq!(
        admit(request, resources(10), resources(6)),
        Err(CapacityError::Cpu)
    );
    assert_eq!(
        admit(resources(0), resources(10), resources(0)),
        Err(CapacityError::InvalidRequest)
    );
    let mut legacy = resources(0);
    legacy.memory_bytes = u64::MAX;
    assert_eq!(
        admit(resources(1), resources(10), legacy),
        Err(CapacityError::Memory)
    );
    assert_eq!(
        admit(resources(1), resources(10), resources(11)),
        Err(CapacityError::Memory)
    );
}

#[test]
fn retention_is_three_by_default_bounded_at_five_and_rotates_only_excess() {
    assert_eq!(BackupPolicy::default().retention(), 3);
    assert!(BackupPolicy::new(0).is_err());
    assert!(BackupPolicy::new(6).is_err());
    assert!(BackupPolicy::new(1).is_ok());
    let policy = BackupPolicy::new(5).unwrap();
    assert_eq!(policy.excess_after_success(5), 0);
    assert_eq!(policy.excess_after_success(6), 1);
    assert_eq!(policy.excess_after_success(0), 0);
}
