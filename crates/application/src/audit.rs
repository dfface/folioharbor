use folioharbor_domain::{
    id::{LibraryId, RequestId, UserId},
    time::OffsetDateTime,
};

use crate::authorization::{Action, ResourceRef};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditDecision {
    Allowed,
    Denied,
}

impl AuditDecision {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Denied => "denied",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditSource {
    Api,
    Worker,
}

impl AuditSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Worker => "worker",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditEvent {
    pub actor: Option<UserId>,
    pub effective_actor: Option<UserId>,
    pub library_id: LibraryId,
    pub action: Action,
    pub resource: ResourceRef,
    pub decision: AuditDecision,
    pub reason_code: Option<&'static str>,
    pub request_id: RequestId,
    pub source: AuditSource,
    pub occurred_at: OffsetDateTime,
    pub network_hmac: Option<[u8; 32]>,
}

impl AuditEvent {
    #[must_use]
    pub fn allowed(
        actor: UserId,
        action: Action,
        resource: ResourceRef,
        request_id: RequestId,
        occurred_at: OffsetDateTime,
    ) -> Self {
        Self {
            actor: Some(actor),
            effective_actor: Some(actor),
            library_id: resource.library_id(),
            action,
            resource,
            decision: AuditDecision::Allowed,
            reason_code: None,
            request_id,
            source: AuditSource::Api,
            occurred_at,
            network_hmac: None,
        }
    }

    #[must_use]
    pub fn denied(
        actor: UserId,
        action: Action,
        resource: ResourceRef,
        reason_code: &'static str,
        request_id: RequestId,
        occurred_at: OffsetDateTime,
    ) -> Self {
        Self {
            actor: Some(actor),
            effective_actor: Some(actor),
            library_id: resource.library_id(),
            action,
            resource,
            decision: AuditDecision::Denied,
            reason_code: Some(reason_code),
            request_id,
            source: AuditSource::Api,
            occurred_at,
            network_hmac: None,
        }
    }
}
