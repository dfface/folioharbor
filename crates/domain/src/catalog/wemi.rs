use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParserMetadata {
    pub titles: Vec<String>,
    pub authors: Vec<String>,
    pub languages: Vec<String>,
    pub identifiers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogMetadata {
    titles: Vec<CatalogText>,
    authors: Vec<CatalogText>,
    languages: Vec<CatalogText>,
    identifiers: Vec<CatalogText>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CatalogText(String);

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum CatalogValueError {
    #[error("publication title is required")]
    MissingTitle,
    #[error("catalog metadata value is invalid")]
    InvalidMetadata,
}

impl CatalogMetadata {
    /// Maps parser strings through catalog invariants without interpreting them as identity keys.
    ///
    /// # Errors
    /// Returns an error for a missing title, blank values, control characters, or oversized text.
    pub fn from_parser(metadata: &ParserMetadata) -> Result<Self, CatalogValueError> {
        if metadata.titles.is_empty() {
            return Err(CatalogValueError::MissingTitle);
        }
        Ok(Self {
            titles: map_values(&metadata.titles)?,
            authors: map_values(&metadata.authors)?,
            languages: map_values(&metadata.languages)?,
            identifiers: map_values(&metadata.identifiers)?,
        })
    }

    #[must_use]
    pub fn primary_title(&self) -> &str {
        self.titles.first().map_or("", CatalogText::as_str)
    }

    pub fn titles(&self) -> impl Iterator<Item = &str> {
        self.titles.iter().map(CatalogText::as_str)
    }

    pub fn authors(&self) -> impl Iterator<Item = &str> {
        self.authors.iter().map(CatalogText::as_str)
    }

    pub fn languages(&self) -> impl Iterator<Item = &str> {
        self.languages.iter().map(CatalogText::as_str)
    }

    pub fn identifiers(&self) -> impl Iterator<Item = &str> {
        self.identifiers.iter().map(CatalogText::as_str)
    }
}

impl CatalogText {
    fn parse(value: &str) -> Result<Self, CatalogValueError> {
        let normalized = value.trim();
        if normalized.is_empty()
            || normalized.len() > 2_048
            || normalized.chars().any(char::is_control)
        {
            return Err(CatalogValueError::InvalidMetadata);
        }
        Ok(Self(normalized.to_owned()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

fn map_values(values: &[String]) -> Result<Vec<CatalogText>, CatalogValueError> {
    values
        .iter()
        .map(String::as_str)
        .map(CatalogText::parse)
        .collect()
}
