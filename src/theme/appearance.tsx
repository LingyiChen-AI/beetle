// 外观（深色 / 浅色 / 跟随系统 + 6 主题色）。读写后端 get_appearance / set_appearance（存 AppConfig.appearance），不用 localStorage。
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { getAppearance, setAppearance as saveAppearance, type Accent, type Appearance, type AppearanceMode } from "../api";

export const MODES: { id: AppearanceMode; label: string }[] = [
  { id: "dark", label: "深色" },
  { id: "light", label: "浅色" },
  { id: "system", label: "跟随系统" },
];
export const ACCENTS: { id: Accent; label: string }[] = [
  { id: "vermilion", label: "朱红" },
  { id: "green", label: "绿" },
  { id: "blue", label: "蓝" },
  { id: "indigo", label: "靛" },
  { id: "purple", label: "紫" },
  { id: "orange", label: "橙" },
  { id: "teal", label: "青" },
];
export const DEFAULT_APPEARANCE: Appearance = { mode: "dark", accent: "vermilion" };

const isMode = (v: unknown): v is AppearanceMode => MODES.some((m) => m.id === v);
const isAccent = (v: unknown): v is Accent => ACCENTS.some((a) => a.id === v);

function systemDark(): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function"
    ? window.matchMedia("(prefers-color-scheme: dark)").matches
    : true;
}

export function resolveTheme(mode: AppearanceMode, sysDark: boolean): "dark" | "light" {
  return mode === "system" ? (sysDark ? "dark" : "light") : mode;
}

let themingTimer: ReturnType<typeof setTimeout> | undefined;
function apply(ap: Appearance, sysDark: boolean, animate: boolean) {
  const r = document.documentElement;
  if (animate) {
    r.classList.add("theming");
    clearTimeout(themingTimer);
    themingTimer = setTimeout(() => r.classList.remove("theming"), 260);
  }
  r.dataset.theme = resolveTheme(ap.mode, sysDark);
  r.dataset.accent = ap.accent;
  r.dataset.mode = ap.mode;
}

interface Ctx {
  appearance: Appearance;
  resolved: "dark" | "light";
  setAppearance: (patch: Partial<Appearance>) => void;
  error: string | null;
}

const AppearanceContext = createContext<Ctx | null>(null);

export function AppearanceProvider({ children }: { children: ReactNode }) {
  const [ap, setAp] = useState<Appearance>(DEFAULT_APPEARANCE);
  const [sysDark, setSysDark] = useState(systemDark);
  const [error, setError] = useState<string | null>(null);

  // 启动时读后端保存的外观；读不到（后端还没这条命令 / 出错）就用默认深色朱红
  useEffect(() => {
    let alive = true;
    getAppearance()
      .then((v) => {
        if (!alive || !v) return;
        setAp({ mode: isMode(v.mode) ? v.mode : DEFAULT_APPEARANCE.mode, accent: isAccent(v.accent) ? v.accent : DEFAULT_APPEARANCE.accent });
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  // 跟随系统：监听 Windows 深浅色变化
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const on = (e: MediaQueryListEvent) => setSysDark(e.matches);
    mq.addEventListener?.("change", on);
    return () => mq.removeEventListener?.("change", on);
  }, []);

  const [animate, setAnimate] = useState(false);
  useEffect(() => {
    apply(ap, sysDark, animate);
  }, [ap, sysDark, animate]);

  const apRef = useRef(ap);
  apRef.current = ap;
  const setAppearance = useCallback((patch: Partial<Appearance>) => {
    const next = { ...apRef.current, ...patch };
    apRef.current = next;
    setAnimate(true);
    setAp(next);
    setError(null);
    saveAppearance(next).catch((e) => setError(typeof e === "string" ? e : String(e)));
  }, []);

  const value = useMemo<Ctx>(
    () => ({ appearance: ap, resolved: resolveTheme(ap.mode, sysDark), setAppearance, error }),
    [ap, sysDark, setAppearance, error],
  );
  return <AppearanceContext.Provider value={value}>{children}</AppearanceContext.Provider>;
}

export function useAppearance(): Ctx {
  const c = useContext(AppearanceContext);
  if (!c) throw new Error("useAppearance 必须在 AppearanceProvider 内使用");
  return c;
}
