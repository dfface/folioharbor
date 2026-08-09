# EPUB 2/3 Compatibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Import safe non-DRM EPUB 2.0/2.0.1 and EPUB 3.0–3.3 publications through the existing Worker pipeline, including common producer quirks.

**Architecture:** Keep archive bounds, path normalization, and catalog output unchanged. Make `package.rs` version-aware, add NCX parsing alongside the current EPUB 3 Navigation Document parser, and choose one safe navigation source or a deterministic spine-derived fallback. Map stable EPUB inspection codes to actionable Worker failures without exposing raw parser errors.

**Tech Stack:** Rust, quick-xml, zip, existing EPUB parser fixtures, SQLx-backed Worker tests.

## Global Constraints

- Support non-DRM EPUB 2.0, 2.0.1, EPUB 3.0–3.3; reject encrypted ZIP entries.
- Preserve all current ZIP, decompression, XML depth, path, and resource-size limits.
- Do not rewrite user-uploaded EPUB bytes.
- Accept only internal normalized targets and never resolve external references.
- Treat ambiguous navigation candidates and an unreadable/empty spine as failures.
- Record compatibility fallbacks as bounded warnings; do not use warnings to bypass safety validation.

---

### Task 1: Add an EPUB 2 NCX parser with security-equivalent target validation

**Files:**
- Create: `crates/epub/src/ncx.rs`
- Modify: `crates/epub/src/lib.rs`
- Test: `crates/epub/tests/valid_epub.rs`

**Interfaces:**
- Produces `ncx::parse(&BoundedArchive, &[u8], &EpubPath) -> Result<Vec<TocEntry>, EpubError>`.
- Every `TocEntry.href` is an `EpubPath` resolved against the NCX location and its archive entry exists after fragment stripping.

- [ ] **Step 1: Write the failing EPUB 2 NCX fixture test**

Add `parses_epub_two_ncx_entries_into_ordered_toc` to `valid_epub.rs`. Construct a bounded archive fixture with `book.opf`, `toc.ncx`, and two internal chapter entries. Assert labels and hrefs preserve nested `navPoint` document order, including `#fragment`.

- [ ] **Step 2: Run the focused test**

Run: `cargo test -p folioharbor-epub parses_epub_two_ncx_entries_into_ordered_toc`

Expected: FAIL because `ncx` is not implemented.

- [ ] **Step 3: Implement `ncx::parse`**

Parse the DAISY NCX namespace with `NsReader`. Accept a single `ncx/navMap`; collect each `navPoint`'s non-empty `navLabel/text` and required `content@src`; resolve with `EpubPath::resolve_from`; call `strip_fragment` and require `archive.contains`. Enforce `BoundedArchive::check_processing(depth)` in every XML event loop and return `InvalidNavigation` for malformed/missing/empty labels, missing targets, duplicate structural roots, or external/unsafe paths.

- [ ] **Step 4: Run focused parser tests**

Run: `cargo test -p folioharbor-epub valid_epub`

Expected: PASS.

- [ ] **Step 5: Add malformed NCX cases and verify rejection**

Add cases in `crates/epub/tests/malicious_epub.rs` for an NCX target outside the archive, an empty label, and missing `navMap`. Run: `cargo test -p folioharbor-epub malicious_epub`.

- [ ] **Step 6: Commit the NCX parser**

```bash
git add crates/epub/src/ncx.rs crates/epub/src/lib.rs crates/epub/tests/valid_epub.rs crates/epub/tests/malicious_epub.rs
git commit -m "feat: parse EPUB 2 NCX navigation"
```

### Task 2: Make package parsing version-aware and add safe fallbacks

**Files:**
- Modify: `crates/epub/src/package.rs`
- Modify: `crates/epub/src/lib.rs`
- Test: `crates/epub/tests/valid_epub.rs`
- Test: `crates/epub/tests/malicious_epub.rs`

**Interfaces:**
- `PackageDocument` retains OPF major version, `spine@toc`, EPUB 2 cover metadata, and `guide` cover href.
- `build_publication` returns the existing `ParsedPublication` plus bounded warnings.

- [ ] **Step 1: Write failing compatibility tests**

Add fixtures proving: a canonical EPUB 2 OPF plus `spine toc="ncx"` imports through NCX; a valid EPUB 3 package keeps nav precedence; an EPUB 2 package with one manifest NCX but no `spine@toc` imports with warning; a package without navigation derives its ordered TOC from readable spine items with warning; and two fallback candidates fail as `InvalidNavigation`.

- [ ] **Step 2: Run compatibility tests**

Run: `cargo test -p folioharbor-epub epub_two`

Expected: FAIL because OPF 2 and NCX are rejected by `PackageState`/`build_publication`.

- [ ] **Step 3: Parse OPF 2 and OPF 3 explicitly**

Replace the `version.starts_with("3.")` predicate with a private `PackageVersion::{Epub2,Epub3}` parsed only from `2.0`, `2.0.1`, and `3.x`. Preserve rejection of unknown major versions. Capture `spine@toc`; parse EPUB 2 `meta name="cover" content="manifest-id"`; capture `guide/reference type="cover" href` as a fallback only when the manifest cover is unavailable.

- [ ] **Step 4: Implement deterministic navigation selection**

For EPUB 3, prefer the unique readable `properties="nav"` item; for EPUB 2, prefer the manifest item identified by `spine@toc` and require `application/x-dtbncx+xml`. If the preferred source is absent, accept one unique readable alternative (NCX or nav) and append an explanatory warning. If neither is valid, generate `TocEntry` records from readable spine entries with their normalized href as the label and append a warning. Reject multiple alternatives, external targets, and empty spines.

- [ ] **Step 5: Implement media-type tolerance without expanding reader safety**

Allow EPUB 2 spine entries declared as `text/html` when their manifest resource is internal; normalize the catalog resource media type to `application/xhtml+xml` only after content is accepted as readable. Continue rejecting arbitrary media types and fallback loops.

- [ ] **Step 6: Run EPUB parser suite**

Run: `cargo test -p folioharbor-epub`

Expected: PASS.

- [ ] **Step 7: Commit version-aware parsing**

```bash
git add crates/epub/src/package.rs crates/epub/src/lib.rs crates/epub/tests/valid_epub.rs crates/epub/tests/malicious_epub.rs
git commit -m "feat: support EPUB 2 package navigation"
```

### Task 3: Preserve typed parser outcomes in Worker import failures

**Files:**
- Modify: `crates/epub/src/lib.rs`
- Modify: `apps/worker/src/handlers.rs`
- Test: `apps/worker/tests/import_recovery.rs`

**Interfaces:**
- `EpubPublicationParser::parse` maps `EncryptedContent` to a stable `PublicationParserError` outcome distinct from malformed packages.
- Worker records safe `error_code` values for encrypted EPUB, invalid navigation, and invalid package failures without raw XML/ZIP text.

- [ ] **Step 1: Write failing Worker import tests**

Add imports that use parser doubles returning encrypted, invalid-navigation, and malformed outcomes. Assert persisted `background_jobs.error_code` and `upload_sessions.error_code` equal the documented stable code, while summaries contain no source path or archive text.

- [ ] **Step 2: Run focused Worker tests**

Run: `cargo test -p folioharbor-worker import_recovery`

Expected: FAIL because all parser errors currently collapse to `Malformed`.

- [ ] **Step 3: Add safe parser-error mapping**

Map only `EncryptedContent` to `encrypted_epub_unsupported`; map navigation/package/content errors to `invalid_epub_navigation` or `invalid_epub`; retain unavailable/capacity/configuration mappings. Keep raw `EpubError` internal and produce fixed user-safe summaries.

- [ ] **Step 4: Verify Worker tests**

Run: `cargo test -p folioharbor-worker import_recovery`

Expected: PASS.

- [ ] **Step 5: Commit actionable import errors**

```bash
git add crates/epub/src/lib.rs apps/worker/src/handlers.rs apps/worker/tests/import_recovery.rs
git commit -m "feat: report EPUB import compatibility failures"
```

### Task 4: Verify the supplied Calibre EPUB end-to-end

**Files:**
- Create: `tests/fixtures/epub/calibre-epub2/README.md`
- Modify: `crates/epub/tests/valid_epub.rs`
- Modify: `docs/operations/configuration.md`

**Interfaces:**
- Documents non-DRM EPUB 2/3 compatibility and explicit DRM exclusion.
- Provides a non-secret, license-safe fixture acquisition or structural fixture instead of committing the supplied copyrighted publication.

- [ ] **Step 1: Write a fixture-backed test for the Calibre structure**

Create a minimal generated fixture matching the supplied archive's OPF 2 + NCX + HTML/XHTML declaration pattern. Assert it produces a non-empty spine and TOC. Do not commit the supplied book bytes.

- [ ] **Step 2: Run the focused fixture test**

Run: `cargo test -p folioharbor-epub calibre_epub_two`

Expected: PASS after Tasks 1 and 2.

- [ ] **Step 3: Update operations documentation**

Document accepted EPUB versions, common recoverable package variations, and clear non-support for DRM/encrypted packages. Do not promise support for arbitrary malformed archives.

- [ ] **Step 4: Run complete verification**

Run:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 5: Commit documentation and fixtures**

```bash
git add tests/fixtures/epub/calibre-epub2/README.md crates/epub/tests/valid_epub.rs docs/operations/configuration.md
git commit -m "docs: document EPUB compatibility"
```

### Task 5: Rebuild and exercise local staging acceptance

**Files:**
- No tracked source changes required.

**Interfaces:**
- Consumes the local staging image and existing ignored staging environment.
- Produces a successful upload/import of the supplied Calibre EPUB in local staging.

- [ ] **Step 1: Build the local image using the configured host proxy**

Run the Dockerfile build with `HTTP_PROXY`, `HTTPS_PROXY`, and `NODE_USE_ENV_PROXY=1` only for the local invocation; do not persist proxy credentials in source.

- [ ] **Step 2: Recreate API and Worker with the local image**

Run the local staging Compose topology with its ignored SMTP override and wait for API, Worker, and Web health checks.

- [ ] **Step 3: Upload the supplied file through the browser**

Use the existing staging administrator to upload the supplied Calibre EPUB. Confirm the import task succeeds and the library item opens with a non-empty table of contents.

- [ ] **Step 4: Commit no environment-specific artifacts**

Run `git status --short` and verify only intentional tracked changes remain. The ignored environment, SMTP secret, local certificate, local override, and uploaded content must not be staged.
