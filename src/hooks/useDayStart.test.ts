import { act, renderHook } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { startOfDay } from "../format";
import { useDayStart } from "./useDayStart";

afterEach(() => vi.useRealTimers());

test("跨过零点后定时器触发，返回新一天的零点", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 10, 22, 23, 59, 0));
  const { result } = renderHook(() => useDayStart());
  expect(result.current).toBe(startOfDay(new Date(2026, 10, 22, 12).getTime()));
  act(() => {
    vi.advanceTimersByTime(2 * 60_000);
  });
  expect(result.current).toBe(startOfDay(new Date(2026, 10, 23, 12).getTime()));
});

test("睡眠唤醒（定时器没按时到）时，窗口获得焦点也会更新", () => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date(2026, 10, 22, 15, 0, 0));
  const { result } = renderHook(() => useDayStart());
  vi.setSystemTime(new Date(2026, 10, 23, 9, 55, 0));
  act(() => {
    window.dispatchEvent(new Event("focus"));
  });
  expect(result.current).toBe(startOfDay(new Date(2026, 10, 23, 12).getTime()));
});
