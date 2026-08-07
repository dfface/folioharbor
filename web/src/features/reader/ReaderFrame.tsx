import { forwardRef, useEffect, useState } from "react";

import type { ReadingFlow } from "./ReadingSettings";
import { createLaidOutPublicationBlob } from "./publicationLayout";

interface ReaderFrameProps {
  blob: Blob;
  flow: ReadingFlow;
  fontScale: number;
  reducedMotion: boolean;
  title: string;
}

export const ReaderFrame = forwardRef<HTMLIFrameElement, ReaderFrameProps>(function ReaderFrame(
  { blob, flow, fontScale, reducedMotion, title },
  ref,
) {
  const [source, setSource] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    let objectUrl: string | null = null;
    setSource(null);
    void createLaidOutPublicationBlob(blob, { flow, fontScale, reducedMotion }).then((laidOut) => {
      if (!active) {
        return;
      }
      objectUrl = URL.createObjectURL(laidOut);
      setSource(objectUrl);
    });
    return () => {
      active = false;
      if (objectUrl !== null) {
        URL.revokeObjectURL(objectUrl);
      }
    };
  }, [blob, flow, fontScale, reducedMotion]);

  if (source === null) {
    return null;
  }

  return (
    <div style={{ maxWidth: "100%", overflow: "hidden" }}>
      <iframe
        ref={ref}
        data-font-scale={fontScale}
        data-reading-flow={flow}
        data-reduced-motion={String(reducedMotion)}
        sandbox=""
        src={source}
        title={title}
        style={{
          border: 0,
          height: "70vh",
          width: "100%",
        }}
      />
    </div>
  );
});
