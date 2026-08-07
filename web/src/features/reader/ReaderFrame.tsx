import { forwardRef, useEffect, useState } from "react";

import type { ReadingFlow } from "./ReadingSettings";

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
    const objectUrl = URL.createObjectURL(blob);
    setSource(objectUrl);
    return () => {
      URL.revokeObjectURL(objectUrl);
    };
  }, [blob]);

  if (source === null) {
    return null;
  }

  const scale = fontScale / 100;
  return (
    <div style={{ maxWidth: "100%", overflow: flow === "continuous" ? "auto" : "hidden" }}>
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
          height: `${String(Math.round(70 / scale))}vh`,
          scrollBehavior: reducedMotion ? "auto" : "smooth",
          width: `${String(100 / scale)}%`,
          zoom: scale,
        }}
      />
    </div>
  );
});
