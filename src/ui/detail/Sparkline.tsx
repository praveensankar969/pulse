import { bucket24h, padSparkline, type SparkPoint } from "../../lib/format";
import type { CompactSample } from "../../lib/types";

type Props = {
  sparkline24: SparkPoint[];
  samples24h: CompactSample[];
  now: number;
};

export function Sparkline({ sparkline24, samples24h, now }: Props) {
  const last24 = padSparkline(sparkline24);
  const strip = bucket24h(samples24h, now);

  return (
    <section className="spark-block" aria-label="History">
      <p className="spark-label">Last 24 checks</p>
      <div className="spark" aria-hidden="true">
        {last24.map((point, index) => (
          <i key={`c-${index}`} className={point} />
        ))}
      </div>
      <p className="spark-label">Last 24 hours</p>
      <div className="spark strip" aria-hidden="true">
        {strip.map((point, index) => (
          <i key={`h-${index}`} className={point} />
        ))}
      </div>
    </section>
  );
}
