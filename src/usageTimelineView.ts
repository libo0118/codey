import type { LogAnalytics } from "./usageAnalysis";

type TimelineStats = Pick<LogAnalytics, "trend" | "fromUnixMs" | "toUnixMs" | "bucketMs">;

/** 仅负责时间图的窗口与选择，不改变服务端统计粒度或数据。 */
export function usageTimelineView(stats: TimelineStats) {
  const { fromUnixMs: from, toUnixMs: to, bucketMs } = stats;
  if (![from, to, bucketMs].every(Number.isFinite) || to <= from || bucketMs <= 0) return null;
  const origin = Math.floor(from / bucketMs) * bucketMs;
  const spansYears = new Date(from).getUTCFullYear() !== new Date(to).getUTCFullYear();
  const count = Math.ceil((to - origin) / bucketMs);
  const source = new Map(stats.trend.filter((bucket) => bucket.timestampUnixMs < to && bucket.timestampUnixMs + bucketMs > from).map((bucket) => [bucket.timestampUnixMs, bucket]));
  const recorded = [...source.values()].filter((bucket) => bucket.total > 0).sort((a, b) => a.timestampUnixMs - b.timestampUnixMs);
  const known = recorded.filter((bucket) => bucket.totalTokensSum != null);
  const peak = known.length ? Math.max(...known.map((bucket) => bucket.totalTokensSum!)) : null;
  const peakBuckets = known.filter((bucket) => bucket.totalTokensSum === peak);
  const indexOf = (timestamp: number) => Math.max(0, Math.min(count - 1, Math.floor((timestamp - origin) / bucketMs)));
  const latestIndex = recorded.length ? indexOf(recorded[recorded.length - 1].timestampUnixMs) : count - 1;
  const focusLength = Math.min(count, Math.max(8, Math.min(24, recorded.length ? latestIndex - indexOf(recorded[0].timestampUnixMs) + 1 : 24)));
  const bucketAt = (index: number) => {
    const timestamp = origin + Math.max(0, Math.min(count - 1, index)) * bucketMs;
    const item = source.get(timestamp);
    return { timestamp, start: Math.max(from, timestamp), end: Math.min(to, timestamp + bucketMs), total: item?.total ?? 0, tokens: item?.totalTokensSum ?? null };
  };
  const windowAt = (start: number, size: number) => {
    const length = Math.max(1, Math.min(count, Math.floor(size)));
    const offset = Math.max(0, Math.min(count - length, Math.floor(start)));
    return { offset, length, buckets: Array.from({ length }, (_, index) => ({ ...bucketAt(offset + index), index: offset + index })) };
  };
  return { count, origin, spansYears, peak, peakIndex: peakBuckets.length ? indexOf(peakBuckets[0].timestampUnixMs) : null, peakCount: peakBuckets.length, latestIndex, focusLength, bucketAt, windowAt };
}
