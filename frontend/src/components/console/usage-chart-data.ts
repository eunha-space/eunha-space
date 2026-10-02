export type UsagePoint = { at: number; value: number | null; note?: string };

/** Keep missing samples and pauses as gaps instead of inventing measurements. */
export function chartSegments(points: UsagePoint[], start: number, end: number, maximum: number, gapMs = 30000) {
  const segments: { x: number; y: number; point: UsagePoint }[][] = [];
  let current: (typeof segments)[number] = [];
  let previous = 0;
  for (const point of points) {
    if (point.value === null || (previous && point.at - previous > gapMs)) {
      if (current.length) segments.push(current);
      current = [];
    }
    if (point.value !== null) {
      current.push({ x: 64 + ((point.at - start) / Math.max(1, end - start)) * 512, y: 164 - (point.value / maximum) * 144, point });
    }
    previous = point.at;
  }
  if (current.length) segments.push(current);
  return segments;
}

