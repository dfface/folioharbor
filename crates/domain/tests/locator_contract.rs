#![allow(clippy::expect_used)]

use std::collections::BTreeMap;

use folioharbor_domain::reader::{
    LocatorExtensionValue, LocatorExtensions, LocatorLocations, LocatorText, ReadiumLocator,
};

#[test]
fn locator_accepts_transport_neutral_readium_position() {
    let locator = ReadiumLocator::new(
        "OPS/chapter.xhtml".to_owned(),
        Some("application/xhtml+xml".to_owned()),
        LocatorLocations::new(Some(0.25), Some(12), None, vec!["epubcfi(/6/4)".to_owned()])
            .expect("valid locations"),
        Some(LocatorText::new(None, Some("hello".to_owned()), None).expect("valid text")),
        LocatorExtensions::new(
            1,
            BTreeMap::from([(
                "folioharbor:page".to_owned(),
                LocatorExtensionValue::Integer(12),
            )]),
        )
        .expect("valid extensions"),
    )
    .expect("valid locator");

    assert_eq!(locator.href(), "OPS/chapter.xhtml");
    assert_eq!(locator.locations().position(), Some(12));
    assert_eq!(locator.extensions().version(), 1);
}

#[test]
fn locator_rejects_dom_fragment_as_the_only_position() {
    let error = ReadiumLocator::new(
        "OPS/chapter.xhtml".to_owned(),
        None,
        LocatorLocations::new(None, None, None, vec!["element-id".to_owned()])
            .expect("bounded fragment"),
        None,
        LocatorExtensions::empty_v1(),
    )
    .expect_err("a DOM identity cannot be the sole position");

    assert_eq!(error.code(), "locator_position_required");
}

#[test]
fn locator_rejects_unbounded_or_unknown_extension_data() {
    assert!(LocatorLocations::new(Some(1.01), None, None, Vec::new()).is_err());
    assert!(LocatorExtensions::new(2, BTreeMap::new()).is_err());
    assert!(
        LocatorExtensions::new(
            1,
            (0..17)
                .map(|index| (format!("x:{index}"), LocatorExtensionValue::Boolean(true)))
                .collect(),
        )
        .is_err()
    );
}
