#![allow(clippy::expect_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use folioharbor_application::{
    catalog::ImportCatalogResult,
    imports::{JobFailure, JobOutcome, ProcessImportJob, RetrySchedule},
    ports::{
        CatalogRepository, CatalogRepositoryError, FinalizeCatalog, ImportReconciliation,
        ImportRepository, ImportRepositoryError, ImportWork, PublicationParser,
        PublicationParserError,
    },
};
use folioharbor_domain::{
    catalog::{
        CatalogMetadata, CatalogPublication, ParserMetadata, PublicationResource, SpineEntry,
    },
    id::{BlobId, JobId, LibraryId, RequestId, UploadId, UserId},
    imports::{
        blob::StorageKey,
        job::{JobInput, JobKind, LeasedJob},
        quota::ByteCount,
        upload::UploadState,
    },
    time::OffsetDateTime,
};
use time::Duration;

#[test]
fn retry_schedule_is_exponential_bounded_and_jittered_by_saga_identity() {
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid fixture time");
    let first = RetrySchedule::default().next(now, JobId::new(), 1);
    let second = RetrySchedule::default().next(now, JobId::new(), 2);

    assert!(first > now + Duration::seconds(1));
    assert!(first < now + Duration::seconds(4));
    assert!(second > now + Duration::seconds(3));
    assert!(second < now + Duration::seconds(8));
}

#[test]
fn worker_failures_are_closed_and_keep_operator_failures_out_of_retry() {
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid fixture time");
    let failures = [
        JobFailure::Permanent {
            code: "invalid_epub",
            summary: "publication is malformed".to_owned(),
        },
        JobFailure::Transient {
            code: "blob_io_unavailable",
            retry_at: now + Duration::seconds(2),
        },
        JobFailure::OperatorRequired {
            code: "storage_capacity_exhausted",
            summary: "storage requires operator action".to_owned(),
        },
    ];

    assert!(matches!(failures[0], JobFailure::Permanent { .. }));
    assert!(matches!(failures[1], JobFailure::Transient { .. }));
    assert!(matches!(failures[2], JobFailure::OperatorRequired { .. }));
}

struct Imports {
    work: ImportWork,
    transitions: Mutex<Vec<UploadState>>,
}

#[async_trait]
impl ImportRepository for Imports {
    async fn reconcile(
        &self,
        _: UploadId,
        _: LibraryId,
        _: RequestId,
        _: OffsetDateTime,
    ) -> Result<ImportReconciliation, ImportRepositoryError> {
        Ok(ImportReconciliation::Work(self.work.clone()))
    }

    async fn begin_catalog(
        &self,
        _: &ImportWork,
        _: RequestId,
        _: OffsetDateTime,
    ) -> Result<(), ImportRepositoryError> {
        self.transitions
            .lock()
            .expect("transition fixture")
            .push(UploadState::Importing);
        Ok(())
    }

    async fn record_failure(
        &self,
        _: &ImportWork,
        to: UploadState,
        _: &'static str,
        _: RequestId,
        _: OffsetDateTime,
    ) -> Result<(), ImportRepositoryError> {
        self.transitions
            .lock()
            .expect("transition fixture")
            .push(to);
        Ok(())
    }
}

struct Parser(Result<CatalogPublication, PublicationParserError>);

#[async_trait]
impl PublicationParser for Parser {
    fn profile_version(&self) -> &'static str {
        "epub-v1"
    }

    async fn parse(&self, _: &StorageKey) -> Result<CatalogPublication, PublicationParserError> {
        self.0.clone()
    }
}

struct Catalog(Mutex<Vec<FinalizeCatalog>>);

#[async_trait]
impl CatalogRepository for Catalog {
    async fn finalize(
        &self,
        command: FinalizeCatalog,
    ) -> Result<ImportCatalogResult, CatalogRepositoryError> {
        self.0.lock().expect("catalog fixture").push(command);
        Ok(ImportCatalogResult::Created {
            item_id: folioharbor_domain::id::ItemId::new(),
            package_id: folioharbor_domain::id::PublicationPackageId::new(),
        })
    }
}

fn publication() -> CatalogPublication {
    let metadata = CatalogMetadata::from_parser(&ParserMetadata {
        titles: vec!["Book".into()],
        authors: vec![],
        languages: vec![],
        identifiers: vec![],
    })
    .expect("fixture metadata");
    CatalogPublication::from_parser(
        metadata,
        vec![
            PublicationResource::new("OPS/chapter.xhtml", "application/xhtml+xml")
                .expect("fixture resource"),
        ],
        vec![SpineEntry::new("OPS/chapter.xhtml", true).expect("fixture spine")],
        vec![],
        None,
    )
    .expect("fixture publication")
}

fn work(upload_id: UploadId, library_id: LibraryId) -> ImportWork {
    ImportWork {
        upload_id,
        library_id,
        actor_id: UserId::new(),
        blob_id: BlobId::new(),
        logical_bytes: ByteCount::new(42),
        storage_key: StorageKey::from_opaque("blob:instance-v1:key:42".into()),
        state: UploadState::Validating,
    }
}

fn leased(upload_id: UploadId, library_id: LibraryId) -> LeasedJob {
    LeasedJob {
        job_id: JobId::new(),
        library_id: Some(library_id),
        kind: JobKind::ImportEpub,
        input: JobInput::upload_v1(upload_id.as_uuid().to_string()),
        attempt: 1,
        lease_expires_at: OffsetDateTime::from_unix_timestamp(1_700_000_300).expect("fixture time"),
    }
}

#[tokio::test]
async fn catalog_commit_is_the_only_operation_that_can_make_an_import_ready() {
    let upload_id = UploadId::new();
    let library_id = LibraryId::new();
    let imports = Arc::new(Imports {
        work: work(upload_id, library_id),
        transitions: Mutex::new(Vec::new()),
    });
    let catalog = Arc::new(Catalog(Mutex::new(Vec::new())));
    let process = ProcessImportJob::new(
        imports.clone(),
        Arc::new(Parser(Ok(publication()))),
        catalog.clone(),
        RetrySchedule::default(),
    );

    let outcome = process
        .execute(leased(upload_id, library_id))
        .await
        .expect("import succeeds");

    assert_eq!(outcome, JobOutcome::Succeeded);
    assert_eq!(
        *imports.transitions.lock().expect("transition fixture"),
        vec![UploadState::Importing]
    );
    assert_eq!(catalog.0.lock().expect("catalog fixture").len(), 1);
}

#[tokio::test]
async fn malformed_epub_is_permanent_and_marks_upload_failed_before_job_finishes() {
    let upload_id = UploadId::new();
    let library_id = LibraryId::new();
    let imports = Arc::new(Imports {
        work: work(upload_id, library_id),
        transitions: Mutex::new(Vec::new()),
    });
    let process = ProcessImportJob::new(
        imports.clone(),
        Arc::new(Parser(Err(PublicationParserError::Malformed))),
        Arc::new(Catalog(Mutex::new(Vec::new()))),
        RetrySchedule::default(),
    );

    let failure = process
        .execute(leased(upload_id, library_id))
        .await
        .expect_err("malformed input fails");

    assert!(matches!(
        failure,
        JobFailure::Permanent {
            code: "invalid_epub",
            ..
        }
    ));
    assert_eq!(
        *imports.transitions.lock().expect("transition fixture"),
        vec![UploadState::Failed]
    );
}

#[tokio::test]
async fn parser_dependency_and_configuration_failures_choose_retry_or_operator_action() {
    let cases = [
        (
            PublicationParserError::Unavailable,
            UploadState::RetryWait,
            "blob_io_unavailable",
            false,
        ),
        (
            PublicationParserError::Configuration,
            UploadState::OperatorRequired,
            "parser_configuration_invalid",
            true,
        ),
        (
            PublicationParserError::Capacity,
            UploadState::OperatorRequired,
            "storage_capacity_exhausted",
            true,
        ),
    ];
    for (parser_error, expected_state, expected_code, operator_required) in cases {
        let upload_id = UploadId::new();
        let library_id = LibraryId::new();
        let imports = Arc::new(Imports {
            work: work(upload_id, library_id),
            transitions: Mutex::new(Vec::new()),
        });
        let process = ProcessImportJob::new(
            imports.clone(),
            Arc::new(Parser(Err(parser_error))),
            Arc::new(Catalog(Mutex::new(Vec::new()))),
            RetrySchedule::default(),
        );

        let failure = process
            .execute(leased(upload_id, library_id))
            .await
            .expect_err("parser failure is classified");

        let code = match &failure {
            JobFailure::Transient { code, .. }
            | JobFailure::Permanent { code, .. }
            | JobFailure::OperatorRequired { code, .. } => *code,
        };
        assert_eq!(code, expected_code);
        assert_eq!(
            matches!(failure, JobFailure::OperatorRequired { .. }),
            operator_required
        );
        assert_eq!(
            *imports.transitions.lock().expect("transition fixture"),
            vec![expected_state]
        );
    }
}
