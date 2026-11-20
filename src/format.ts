// 时间 / 耗时格式化。
const p2 = (n: number) => String(n).padStart(2, "0");

/** HH:MM */
export const hm = (ms: number) => {
  const d = new Date(ms);
  return `${p2(d.getHours())}:${p2(d.getMinutes())}`;
};
/** HH:MM:SS */
export const hms = (ms: number) => {
  const d = new Date(ms);
  return `${p2(d.getHours())}:${p2(d.getMinutes())}:${p2(d.getSeconds())}`;
};
/** MM-DD HH:MM */
export const mdhm = (ms: number) => {
  const d = new Date(ms);
  return `${p2(d.getMonth() + 1)}-${p2(d.getDate())} ${hm(ms)}`;
};

/** 6分12秒 */
export function fmtDur(ms: number | null | undefined): string {
  if (ms == null || ms < 0) return "—";
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}分${p2(s % 60)}秒`;
}

/** 步骤耗时：0.6s / 21.7s / 4m12s */
export function fmtStepDur(ms: number | null | undefined): string {
  if (ms == null) return "";
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}m${p2(s % 60)}s`;
}

export function startOfDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

export const isToday = (ms: number, now = Date.now()) => ms >= startOfDay(now);

/** 列表里「最后回复」的写法：今天 HH:MM / 昨天 HH:MM / MM-DD HH:MM */
export function relTime(ms: number, now = Date.now()): string {
  const today = startOfDay(now);
  if (ms >= today) return hm(ms);
  if (ms >= today - 86_400_000) return `昨天 ${hm(ms)}`;
  return mdhm(ms);
}
