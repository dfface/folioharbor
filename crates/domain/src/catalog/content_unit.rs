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
    let decoded = percent_decoded(&value)?;
    if value.is_empty()
        || value.len() > 2_048
        || value.contains(['?', '#'])
        || decoded.starts_with('/')
        || decoded.contains(['\\', ':', '?', '#'])
        || decoded
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || decoded.chars().any(char::is_control)
    {
        return Err(CatalogValueError::InvalidMetadata);
    }
    Ok(value)
}

fn percent_decoded(value: &str) -> Result<String, CatalogValueError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes.get(index + 1).and_then(|byte| hex_digit(*byte));
            let low = bytes.get(index + 2).and_then(|byte| hex_digit(*byte));
            let (Some(high), Some(low)) = (high, low) else {
                return Err(CatalogValueError::InvalidMetadata);
            };
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| CatalogValueError::InvalidMetadata)
}

const fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
