use folioharbor_domain::id::{ErrorId, LibraryId, RequestId, UserId};

#[test]
fn ids_of_different_domains_are_not_interchangeable_and_round_trip() {
    let raw = uuid::Uuid::now_v7();
    let user = UserId::from_uuid(raw);
    let library = LibraryId::from_uuid(raw);

    assert_eq!(user.as_uuid(), raw);
    assert_eq!(library.as_uuid(), raw);
    assert_ne!(format!("{user:?}"), format!("{library:?}"));
}

#[test]
fn correlation_ids_of_different_domains_round_trip() {
    let raw = ulid::Ulid::new();
    let request = RequestId::from_ulid(raw);
    let error = ErrorId::from_ulid(raw);

    assert_eq!(request.as_ulid(), raw);
    assert_eq!(error.as_ulid(), raw);
    assert_ne!(format!("{request:?}"), format!("{error:?}"));
}
