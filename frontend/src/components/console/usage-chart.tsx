import { useState, type KeyboardEvent, type PointerEvent } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";

import { chartSegments, type UsagePoint } from "./usage-chart-data";

// Building a formatter per call costs tens of microseconds; charts label hundreds of samples.
const clock = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
const dated = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });

/**
 * One polyline per unbroken run, plus a single hover or arrow-key readout.
 * A long range has hundreds of samples per chart and the page has many charts,
 * so samples are not individual DOM elements.
 */
export function UsageChart({ title, points, now, format, color = "var(--chart-1)", windowMs = 600000 }: {
  title: string; points: UsagePoint[]; now: number; format: (value: number) => string; color?: string; windowMs?: number;
}) {
  const [active, setActive] = useState<number | null>(null);
  const start = now - windowMs;
  const visible = points.filter(point => point.at >= start && point.at <= now);
  const measured = visible.filter(point => point.value !== null);
  const maximum = Math.max(1, ...measured.map(point => point.value!)) * 1.1;
  const segments = chartSegments(visible, start, now, maximum, Math.max(30000, Math.ceil(windowMs / 3600000) * 10000 * 3));
  const samples = segments.flat();
  const time = (at: number) => (windowMs >= 86400000 ? dated : clock).format(at);
  const selected = active === null ? undefined : samples[Math.min(active, samples.length - 1)];
  const latest = measured.at(-1);
  const readout = selected ?? samples.at(-1);
  const describe = (sample: typeof readout) => sample ? `${time(sample.point.at)}: ${format(sample.point.value!)}${sample.point.note ? ` · ${sample.point.note}` : ""}` : "";

  function pick(event: PointerEvent<SVGSVGElement>) {
    if (!samples.length) return;
    const box = event.currentTarget.getBoundingClientRect();
    const x = ((event.clientX - box.left) / Math.max(1, box.width)) * 600;
    let nearest = 0;
    samples.forEach((sample, index) => { if (Math.abs(sample.x - x) < Math.abs(samples[nearest].x - x)) nearest = index; });
    setActive(nearest);
  }
  function step(event: KeyboardEvent<SVGSVGElement>) {
    const last = samples.length - 1;
    const current = active ?? last;
    const next = { ArrowLeft: current - 1, ArrowRight: current + 1, Home: 0, End: last }[event.key];
    if (next === undefined || last < 0) return;
    event.preventDefault();
    setActive(Math.max(0, Math.min(last, next)));
  }

  return <Card>
    <CardHeader><CardTitle>{title}</CardTitle></CardHeader>
    <CardContent>
      {!latest ? <div className="text-muted-foreground grid h-48 place-items-center text-sm">Waiting for measurements…</div> : <>
        <svg viewBox="0 0 600 208" className="w-full touch-pan-y outline-none focus-visible:ring-ring/50 focus-visible:rounded-md focus-visible:ring-3" role="img" tabIndex={0}
          aria-label={`${title} over the selected time range. Latest measurement: ${format(latest.value!)}. Use the arrow keys to read samples.`}
          onPointerMove={pick} onPointerDown={pick} onPointerLeave={() => setActive(null)} onKeyDown={step} onBlur={() => setActive(null)}>
          <title>{title}</title>
          <desc>Missing measurements and sampling pauses appear as gaps.</desc>
          {[0, 0.5, 1].map(fraction => <g key={fraction}>
            <line x1="64" x2="576" y1={164 - fraction * 144} y2={164 - fraction * 144} stroke="var(--border)" />
            <text x="58" y={168 - fraction * 144} textAnchor="end" fill="var(--muted-foreground)" fontSize="10">{format(maximum * fraction)}</text>
          </g>)}
          {segments.map((segment, index) => segment.length === 1
            // An isolated sample has no line to sit on.
            ? <circle key={index} cx={segment[0].x} cy={segment[0].y} r="2.5" fill={color} />
            : <polyline key={index} points={segment.map(point => `${point.x},${point.y}`).join(" ")} fill="none" stroke={color} strokeWidth="2" strokeLinejoin="round" />)}
          {selected && <g pointerEvents="none">
            <line x1={selected.x} x2={selected.x} y1="20" y2="164" stroke="var(--muted-foreground)" strokeDasharray="3 3" />
            <circle cx={selected.x} cy={selected.y} r="4" fill={color} stroke="var(--card)" strokeWidth="2" />
          </g>}
          {[start, start + windowMs / 2, now].map((at, index) => <text key={index} x={64 + index * 256} y="192" textAnchor={index === 0 ? "start" : index === 2 ? "end" : "middle"} fill="var(--muted-foreground)" fontSize="11">{time(at)}</text>)}
        </svg>
        <p className="text-muted-foreground min-h-4 truncate text-xs tabular-nums" aria-live={selected ? "polite" : "off"}>{selected ? describe(selected) : `Latest ${describe(readout)}`}</p>
      </>}
    </CardContent>
  </Card>;
}
