# Calibre-like EPUB 2 structural fixture

The regression test `parses_calibre_epub_two_opf_ncx_and_html_structure` builds a
small, deterministic archive in `crates/epub/tests/valid_epub.rs`. It is an
original structural analogue of the supplied EPUB, not a copy of that
publication.

It preserves the compatibility-relevant package shape only:

- an EPUB 2 OPF at `OEBPS/content.opf`;
- an NCX table of contents selected with `spine@toc`;
- both HTML (`text/html`) and XHTML (`application/xhtml+xml`) spine resources;
- relative OPF and NCX paths rooted below `OEBPS/`.

All titles, identifiers, chapter labels, and content are project-authored
placeholders. The supplied book must not be copied into this repository or CI
artifacts. To exercise a local installation with a licensed copy, upload it
through the normal import workflow; do not replace this fixture.
