#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EpubErrorCode {
    InvalidArchive,
    EntryLimit,
    TotalSizeLimit,
    CompressionRatioLimit,
    PathDepthLimit,
    XmlDepthLimit,
    ResourceSizeLimit,
    DeadlineExceeded,
    UnsafePath,
    DuplicatePath,
    EncryptedContent,
    InvalidContainer,
    MissingPackage,
    InvalidPackage,
    InvalidSpine,
    InvalidNavigation,
    InvalidContent,
}

#[derive(Debug, thiserror::Error)]
#[error("EPUB inspection failed ({code:?})")]
pub struct EpubError {
    code: EpubErrorCode,
}

impl EpubError {
    pub(crate) const fn new(code: EpubErrorCode) -> Self {
        Self { code }
    }

    #[must_use]
    pub const fn code(&self) -> EpubErrorCode {
        self.code
    }
}
