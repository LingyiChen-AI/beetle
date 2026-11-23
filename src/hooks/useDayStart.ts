// 「今天」的零点时间戳；跨过零点时组件会重新渲染。
// 客户端常驻托盘、几天不关：「今日运行」这些数只在有新运行时才刷新的话，过了零点还停在昨天。
import { useEffect, useState } from "react";
import { startOfDay } from "../format";

export function useDayStart(): number {
  const [day, setDay] = useState(() => startOfDay(Date.now()));
  useEffect(() => {
    const check = () => setDay(startOfDay(Date.now()));
    // 睡眠唤醒后定时器会晚到，窗口重新可见 / 获得焦点时也核对一次
    const next = new Date(day);
    next.setDate(next.getDate() + 1);
    const t = window.setTimeout(check, Math.max(1000, next.getTime() - Date.now() + 1000));
    document.addEventListener("visibilitychange", check);
    window.addEventListener("focus", check);
    return () => {
      window.clearTimeout(t);
      document.removeEventListener("visibilitychange", check);
      window.removeEventListener("focus", check);
    };
  }, [day]);
  return day;
}
