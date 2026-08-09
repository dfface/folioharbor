# EPUB 2/3 compatibility design

## Purpose

Import non-DRM EPUB 2.0, 2.0.1, and EPUB 3.0–3.3 publications into the
existing catalog and reader pipeline. The parser must accept common
producer quirks when a publication remains safe and unambiguous to read.

## Scope and boundary

The importer continues to reject encrypted or DRM-protected archives, damaged
ZIP data, path traversal, external references, over-limit archives, and a
package without any readable spine content. These are safety or availability
boundaries, not compatibility warnings.

Benign package deviations are recoverable. The importer records a bounded
compatibility-warning list in the parsed publication and continues importing
when it can derive one safe, deterministic result.

## Version-aware package model

`package.rs` determines the OPF major version at the root package element and
parses both versions into the existing `ParsedPublication` model. Catalog,
storage, API, and reader layers remain version-neutral.

EPUB 3 retains the current Navigation Document path: exactly one usable
manifest item with the `nav` property is preferred. EPUB 2 resolves the NCX
manifest item named by `spine@toc`. Both sources normalize into ordered
`TocEntry` values with internal package-relative targets.

## Navigation recovery order

1. EPUB 3: use a valid Navigation Document.
2. EPUB 2: use the NCX item referenced by `spine@toc`.
3. If the preferred source is absent, use the one unique valid alternative
   (EPUB 3 NCX or EPUB 2 Navigation Document) and record a warning.
4. If no valid navigation source exists, derive a linear table of contents
   from readable spine entries and record a warning.

An ambiguous alternative (multiple candidates), an empty spine, or a target
outside the archive remains a hard failure.

## Compatibility rules

- Accept OPF 2 and OPF 3 namespaces and their metadata conventions.
- Recognize EPUB 2 cover metadata (`meta name="cover"`) and `guide` fallback.
- Treat common XHTML/HTML media-type mismatches as warnings only when the
  referenced entry is an allowed internal reading resource.
- Preserve strict path normalization, archive size/decompression limits, XML
  depth limits, and resource type allowlists before any fallback is used.
- Report typed import failures: encrypted EPUB unsupported, missing readable
  navigation, invalid package, or invalid internal target. Do not expose raw
  archive/parser error strings to users.

## Testing and verification

Add focused parser fixtures for a canonical EPUB 2 NCX package, the supplied
Calibre EPUB 2 structure, an EPUB 3 package, missing-navigation spine
fallback, and each unsafe/ambiguous rejection path. Add an end-to-end Worker
import assertion proving a valid EPUB 2 reaches the ready catalog state.

Run the EPUB crate tests, Worker import tests, workspace formatting and
Clippy, then build the local staging image and re-upload the supplied Calibre
fixture. A successful import must expose ordered reading content and a
generated or parsed table of contents.
