use crate::model::{Server, User};

pub fn can_manage(actor: &User, server: &Server, member_enabled: bool) -> bool {
    actor.is_admin || actor.id == server.owner || member_enabled
}

pub fn can_change_member(
    actor: &User,
    server: &Server,
    actor_enabled: bool,
    target: &User,
    enable: bool,
) -> bool {
    can_manage(actor, server, actor_enabled)
        && (enable || (target.id != server.owner && !target.is_admin))
}
