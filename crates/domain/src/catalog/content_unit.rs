use super::CatalogValueError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpineEntry {
    href: String,
    linear: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TocEntry {
    label: String,
    href: String,
}

impl SpineEntry {
    /// # Errors
    /// Returns an error unless the href is a normalized, internal EPUB path.
    pub fn new(href: impl Into<String>, linear: bool) -> Result<Self, CatalogValueError> {
        Ok(Self {
            href: normalized_href(href.into())?,
            linear,
        })
    }

    #[must_use]
    pub fn href(&self) -> &str {
        &self.href
    }

    #[must_use]
    pub const fn is_linear(&self) -> bool {
        self.linear
    }
}

impl TocEntry {
    /// # Errors
    /// Returns an error for an invalid label or non-internal href.
    pub fn new(
        label: impl Into<String>,
        href: impl Into<String>,
    ) -> Result<Self, CatalogValueError> {
        let label = label.into();
        let label = label.trim();
        if label.is_empty() || label.len() > 2_048 || label.chars().any(char::is_control) {
            return Err(CatalogValueError::InvalidMetadata);
        }
        Ok(Self {
            label: label.to_owned(),
            href: normalized_href(href.into())?,
        })
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    #[must_use]
    pub fn href(&self) -> &str {
        &self.href
    }
}

pub(crate) fn normalized_href(value: String) -> Result<String, CatalogValueError> {
    let path = value.split('#').next().unwrap_or_default();
    if path.is_empty()
        || path.len() > 2_048
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path.chars().any(char::is_control)
    {
        return Err(CatalogValueError::InvalidMetadata);
    }
    Ok(value)
}
