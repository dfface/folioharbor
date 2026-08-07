import type { ReadingFlow } from "./ReadingSettings";

interface PublicationLayout {
  flow: ReadingFlow;
  fontScale: number;
  reducedMotion: boolean;
}

function readBlob(blob: Blob): Promise<string> {
  if (typeof blob.text === "function") {
    return blob.text();
  }
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.addEventListener("load", () => {
      if (typeof reader.result === "string") {
        resolve(reader.result);
      } else {
        reject(new Error("publication_blob_read_failed"));
      }
    });
    reader.addEventListener("error", () => { reject(reader.error ?? new Error("publication_blob_read_failed")); });
    reader.readAsText(blob);
  });
}

function layoutCss({ flow, fontScale, reducedMotion }: PublicationLayout): string {
  const motion = reducedMotion ? "auto" : "smooth";
  const shared = `
* { box-sizing: border-box; }
html, body { margin: 0; padding: 0; }
body { font-size: ${String(fontScale)}%; line-height: 1.6; scroll-behavior: ${motion}; }
img, svg, video { max-inline-size: 100%; block-size: auto; }
`;
  if (flow === "continuous") {
    return `${shared}
html { min-block-size: 100%; overflow-x: hidden; overflow-y: auto; }
body { min-block-size: 100%; padding: 1.5rem; overflow: visible; }
`;
  }
  return `${shared}
html, body { block-size: 100%; overflow: hidden; }
body {
  block-size: calc(100vh - 2rem);
  padding: 0;
  margin: 1rem;
  column-fill: auto;
  column-gap: 2rem;
  column-width: calc(100vw - 2rem);
  overflow-x: auto;
  overflow-y: hidden;
  scroll-snap-type: x mandatory;
}
body > * { break-inside: avoid; scroll-snap-align: start; }
`;
}

export async function createLaidOutPublicationBlob(blob: Blob, layout: PublicationLayout): Promise<Blob> {
  const source = await readBlob(blob);
  const mediaType = blob.type.split(";", 1)[0]?.trim().toLowerCase();
  const parseType = mediaType === "text/html" ? "text/html" : "application/xhtml+xml";
  const publication = new DOMParser().parseFromString(source, parseType);
  if (publication.querySelector("parsererror") !== null) {
    throw new Error("publication_markup_invalid");
  }
  const namespace = publication.documentElement.namespaceURI ?? "http://www.w3.org/1999/xhtml";
  const style = publication.createElementNS(namespace, "style");
  style.setAttribute("data-folioharbor-reader-layout", layout.flow);
  style.textContent = layoutCss(layout);
  let head: Element | null = publication.querySelector("head");
  if (head === null) {
    head = publication.createElementNS(namespace, "head");
    publication.documentElement.insertBefore(head, publication.documentElement.firstChild);
  }
  head.append(style);
  return new Blob([new XMLSerializer().serializeToString(publication)], {
    type: mediaType ?? "application/xhtml+xml",
  });
}
