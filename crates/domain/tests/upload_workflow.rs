use folioharbor_domain::imports::upload::UploadState;

#[test]
fn upload_state_machine_allows_only_documented_transitions() {
    use UploadState::{
        Created, Duplicate, Expired, Failed, Importing, Queued, Ready, Received, Receiving,
        RetryWait, Validating,
    };

    let allowed = [
        (Created, Receiving),
        (Created, Expired),
        (Receiving, Received),
        (Receiving, Failed),
        (Received, Queued),
        (Queued, Validating),
        (Validating, Importing),
        (Validating, Failed),
        (Validating, RetryWait),
        (Importing, Ready),
        (Importing, Duplicate),
        (Importing, Failed),
        (Importing, RetryWait),
        (RetryWait, Queued),
        (Failed, Receiving),
    ];
    for (from, to) in allowed {
        assert!(from.can_transition_to(to), "{from:?} -> {to:?}");
    }
    for from in UploadState::ALL {
        for to in UploadState::ALL {
            if !allowed.contains(&(from, to)) {
                assert!(!from.can_transition_to(to), "{from:?} -> {to:?}");
            }
        }
    }
}
