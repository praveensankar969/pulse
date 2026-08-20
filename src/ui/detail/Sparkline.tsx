import {
  bucket24h,
  SPARKLINE_HEIGHT,
  SPARKLINE_WIDTH,
  sparklinePath,
} from "../../lib/format";
import type { CompactSample } from "../../lib/types";

type Props = {
  samples24h: CompactSample[];
  now: number;
};

export function Sparkline({ samples24h, now }: Props) {
  const path = sparklinePath(samples24h);
  const strip = bucket24h(samples24h, now);

  return (
    <section className="spark-block" aria-label="History">
      <p className="spark-label">Last 24 checks</p>
      <svg
        className="spark-line"
        viewBox={`0 0 ${SPARKLINE_WIDTH} ${SPARKLINE_HEIGHT}`}
        preserveAspectRatio="none"
        aria-hidden="true"
      >
        {path ? (
          <path
            d={path}
            fill="none"
            stroke="currentColor"
            strokeWidth="1.6"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        ) : null}
      </svg>
      <p className="spark-label">Last 24 hours</p>
      <div className="spark strip" aria-hidden="true">
        {strip.map((point, index) => (
          <i key={`h-${index}`} className={point} />
        ))}
      </div>
    </section>
  );
}
