import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";

import { chartSegments, type UsagePoint } from "./usage-chart-data";

export function UsageChart({ title, points, now, format, color = "var(--chart-1)", windowMs = 600000 }: {
  title: string; points: UsagePoint[]; now: number; format: (value: number) => string; color?: string; windowMs?: number;
}) {
  const start = now - windowMs;
  const visible = points.filter(point => point.at >= start && point.at <= now);
  const measured = visible.filter(point => point.value !== null);
  const maximum = Math.max(1, ...measured.map(point => point.value!)) * 1.1;
  const segments = chartSegments(visible, start, now, maximum, Math.max(30000, Math.ceil(windowMs / 3600000) * 10000 * 3));
  const time = (at: number) => new Date(at).toLocaleString(undefined, { ...(windowMs >= 86400000 ? { month: "short", day: "numeric" } as const : {}), hour: "2-digit", minute: "2-digit" });
  return <Card>
    <CardHeader><CardTitle>{title}</CardTitle></CardHeader>
    <CardContent>
      {measured.length === 0 ? <div className="text-muted-foreground grid h-48 place-items-center text-sm">Waiting for measurements…</div> :
        <svg viewBox="0 0 600 208" className="w-full" role="img" aria-label={`${title} over the selected time range. Latest measurement: ${format(measured.at(-1)!.value!)}.`}>
          <title>{title}</title>
          <desc>Missing measurements and sampling pauses appear as gaps. Hover or focus a sample to read its value.</desc>
          {[0, 0.5, 1].map(fraction => <g key={fraction}>
            <line x1="64" x2="576" y1={164 - fraction * 144} y2={164 - fraction * 144} stroke="var(--border)" />
            <text x="58" y={168 - fraction * 144} textAnchor="end" fill="var(--muted-foreground)" fontSize="10">{format(maximum * fraction)}</text>
          </g>)}
          {segments.map((segment, index) => <g key={index}>
            <polyline points={segment.map(point => `${point.x},${point.y}`).join(" ")} fill="none" stroke={color} strokeWidth="2" />
            {segment.map(({ x, y, point }) => <circle key={point.at} cx={x} cy={y} r="3" fill={color} tabIndex={0} aria-label={`${time(point.at)}: ${format(point.value!)}${point.note ? ` · ${point.note}` : ""}`}><title>{time(point.at)}: {format(point.value!)}{point.note ? ` · ${point.note}` : ""}</title></circle>)}
          </g>)}
          {[start, start + windowMs / 2, now].map((at, index) => <text key={index} x={64 + index * 256} y="192" textAnchor={index === 0 ? "start" : index === 2 ? "end" : "middle"} fill="var(--muted-foreground)" fontSize="11">{time(at)}</text>)}
        </svg>}
    </CardContent>
  </Card>;
}
