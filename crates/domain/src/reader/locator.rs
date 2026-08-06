use std::collections::BTreeMap;

use thiserror::Error;

const MAX_HREF: usize = 2_048;
const MAX_MEDIA_TYPE: usize = 255;
const MAX_FRAGMENT: usize = 2_048;
const MAX_TEXT: usize = 4_096;
const MAX_EXTENSIONS: usize = 16;
const MAX_EXTENSION_KEY: usize = 128;
const MAX_EXTENSION_TEXT: usize = 1_024;

#[derive(Clone, Debug, PartialEq)]
pub struct ReadiumLocator {
    href: String,
    media_type: Option<String>,
    locations: LocatorLocations,
    text: Option<LocatorText>,
    extensions: LocatorExtensions,
}

impl ReadiumLocator {
    /// Builds a bounded, transport-neutral locator.
    ///
    /// # Errors
    ///
    /// Returns an error when a field is out of bounds or no durable position is present.
    pub fn new(
        href: String,
        media_type: Option<String>,
        locations: LocatorLocations,
        text: Option<LocatorText>,
        extensions: LocatorExtensions,
    ) -> Result<Self, LocatorError> {
        bounded(&href, MAX_HREF, "locator_href_invalid")?;
        if let Some(value) = &media_type {
            bounded(value, MAX_MEDIA_TYPE, "locator_media_type_invalid")?;
        }
        if !locations.has_transport_neutral_position() {
            return Err(LocatorError::new("locator_position_required"));
        }
        Ok(Self {
            href,
            media_type,
            locations,
            text,
            extensions,
        })
    }

    #[must_use]
    pub fn href(&self) -> &str {
        &self.href
    }
    #[must_use]
    pub fn media_type(&self) -> Option<&str> {
        self.media_type.as_deref()
    }
    #[must_use]
    pub const fn locations(&self) -> &LocatorLocations {
        &self.locations
    }
    #[must_use]
    pub const fn text(&self) -> Option<&LocatorText> {
        self.text.as_ref()
    }
    #[must_use]
    pub const fn extensions(&self) -> &LocatorExtensions {
        &self.extensions
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocatorLocations {
    progression: Option<f64>,
    position: Option<u32>,
    total_progression: Option<f64>,
    fragments: Vec<String>,
}

impl LocatorLocations {
    /// Builds bounded Readium location coordinates.
    ///
    /// # Errors
    ///
    /// Returns an error for non-finite/out-of-range progress or excessive fragments.
    pub fn new(
        progression: Option<f64>,
        position: Option<u32>,
        total_progression: Option<f64>,
        fragments: Vec<String>,
    ) -> Result<Self, LocatorError> {
        for value in [progression, total_progression].into_iter().flatten() {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(LocatorError::new("locator_progression_invalid"));
            }
        }
        if fragments.len() > 16 {
            return Err(LocatorError::new("locator_fragments_invalid"));
        }
        for fragment in &fragments {
            bounded(fragment, MAX_FRAGMENT, "locator_fragments_invalid")?;
        }
        Ok(Self {
            progression,
            position,
            total_progression,
            fragments,
        })
    }

    fn has_transport_neutral_position(&self) -> bool {
        self.progression.is_some() || self.position.is_some() || self.total_progression.is_some()
    }
    #[must_use]
    pub const fn progression(&self) -> Option<f64> {
        self.progression
    }
    #[must_use]
    pub const fn position(&self) -> Option<u32> {
        self.position
    }
    #[must_use]
    pub const fn total_progression(&self) -> Option<f64> {
        self.total_progression
    }
    #[must_use]
    pub fn fragments(&self) -> &[String] {
        &self.fragments
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocatorText {
    before: Option<String>,
    highlight: Option<String>,
    after: Option<String>,
}
impl LocatorText {
    /// Builds bounded optional text context.
    ///
    /// # Errors
    ///
    /// Returns an error when any supplied context string is empty or too long.
    pub fn new(
        before: Option<String>,
        highlight: Option<String>,
        after: Option<String>,
    ) -> Result<Self, LocatorError> {
        for value in [&before, &highlight, &after].into_iter().flatten() {
            bounded(value, MAX_TEXT, "locator_text_invalid")?;
        }
        Ok(Self {
            before,
            highlight,
            after,
        })
    }
    #[must_use]
    pub fn before(&self) -> Option<&str> {
        self.before.as_deref()
    }
    #[must_use]
    pub fn highlight(&self) -> Option<&str> {
        self.highlight.as_deref()
    }
    #[must_use]
    pub fn after(&self) -> Option<&str> {
        self.after.as_deref()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LocatorExtensionValue {
    Boolean(bool),
    Integer(i64),
    Number(f64),
    String(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocatorExtensions {
    version: u16,
    values: BTreeMap<String, LocatorExtensionValue>,
}
impl LocatorExtensions {
    /// Builds the explicitly versioned extension bag.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported versions, keys, values, or excessive entries.
    pub fn new(
        version: u16,
        values: BTreeMap<String, LocatorExtensionValue>,
    ) -> Result<Self, LocatorError> {
        if version != 1 || values.len() > MAX_EXTENSIONS {
            return Err(LocatorError::new("locator_extensions_invalid"));
        }
        for (key, value) in &values {
            bounded(key, MAX_EXTENSION_KEY, "locator_extensions_invalid")?;
            if !key.contains(':') {
                return Err(LocatorError::new("locator_extensions_invalid"));
            }
            match value {
                LocatorExtensionValue::String(text) => {
                    bounded(text, MAX_EXTENSION_TEXT, "locator_extensions_invalid")?;
                }
                LocatorExtensionValue::Number(number) if !number.is_finite() => {
                    return Err(LocatorError::new("locator_extensions_invalid"));
                }
                _ => {}
            }
        }
        Ok(Self { version, values })
    }
    #[must_use]
    pub fn empty_v1() -> Self {
        Self {
            version: 1,
            values: BTreeMap::new(),
        }
    }
    #[must_use]
    pub const fn version(&self) -> u16 {
        self.version
    }
    #[must_use]
    pub const fn values(&self) -> &BTreeMap<String, LocatorExtensionValue> {
        &self.values
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("invalid locator")]
pub struct LocatorError {
    code: &'static str,
}
impl LocatorError {
    const fn new(code: &'static str) -> Self {
        Self { code }
    }
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }
}

fn bounded(value: &str, max: usize, code: &'static str) -> Result<(), LocatorError> {
    if value.is_empty() || value.len() > max {
        Err(LocatorError::new(code))
    } else {
        Ok(())
    }
}
