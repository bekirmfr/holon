// A flow's runs hour by hour as bars (overview.ts's `bars`): the oldest hour on the left, red
// where a run failed. Read out as its words (`hoursSaid`), not its bars. Narrower than `width`
// where there is less room (styles.css), its bars narrow and its height stays.

import { bars, hoursSaid } from "../overview";

export function Spark({ runs, failed, width = 96, height = 22 }: { runs: number[]; failed: number[]; width?: number; height?: number }) {
  const bw = width / Math.max(1, runs.length);
  return (
    <svg className="hl-spark" width={width} height={height} viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" role="img" aria-label={hoursSaid(runs, failed)}>
      {bars(runs, failed).map((b, i) => {
        const h = b.height * (height - 2);
        return <rect key={i} className={b.bad ? "hl-spark__bad" : undefined} x={i * bw + 0.5} y={height - h} width={Math.max(0.5, bw - 1.5)} height={h} rx={1} />;
      })}
    </svg>
  );
}
