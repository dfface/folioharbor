use folioharbor_domain::id::{RequestId, SessionId, UserId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Actor {
    pub user_id: UserId,
    pub session_id: SessionId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestContext {
    pub actor: Actor,
    pub request_id: RequestId,
}
