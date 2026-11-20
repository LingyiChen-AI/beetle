// 自绘标题栏（tauri.conf.json decorations:false）。空白处 data-tauri-drag-region 可拖动 / 双击最大化；
// 右侧：外观快捷浮层、最小化、最大化、关闭（关闭 = 隐藏到托盘，后端 tray.rs 拦截 CloseRequested）。
import { ChevronRight, Minus, Moon, Settings, Square, Sun, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { windowClose, windowMinimize, windowToggleMaximize } from "../api";
import { useAppearance } from "../theme/appearance";
import BrandLogo from "./BrandLogo";
import { AccentSwatches, ModeSeg } from "./common";

export default function TitleBar({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { resolved } = useAppearance();
  const [pop, setPop] = useState(false);
  const btnRef = useRef<HTMLButtonElement>(null);
  const [right, setRight] = useState(146);

  useEffect(() => {
    if (!pop) return;
    const r = btnRef.current?.getBoundingClientRect();
    if (r && r.right > 0) setRight(Math.max(8, Math.round(window.innerWidth - r.right)));
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setPop(false);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [pop]);

  const ThemeIcon = resolved === "dark" ? Moon : Sun;
  return (
    <>
      <div className="titlebar" data-tauri-drag-region>
        <div className="brand" data-tauri-drag-region>
          <BrandLogo size={18} />
          Beetle
        </div>
        <div className="flex-1 self-stretch" data-tauri-drag-region />
        <button
          ref={btnRef}
          type="button"
          className={`tb-act tb-icon tr ${pop ? "on" : ""}`}
          aria-label="外观：切换深浅色与主题色"
          aria-haspopup="dialog"
          aria-expanded={pop}
          title="外观"
          onClick={() => setPop((v) => !v)}
        >
          <ThemeIcon style={{ width: 14, height: 14 }} />
        </button>
        <button type="button" className="wc tr" aria-label="最小化" onClick={() => void windowMinimize()}>
          <Minus style={{ width: 15, height: 15 }} />
        </button>
        <button type="button" className="wc tr" aria-label="最大化" onClick={() => void windowToggleMaximize()}>
          <Square style={{ width: 12, height: 12 }} />
        </button>
        <button type="button" className="wc close tr" aria-label="关闭" onClick={() => void windowClose()}>
          <X style={{ width: 16, height: 16 }} />
        </button>
      </div>
      {pop && (
        <>
          <div className="absolute inset-0" style={{ zIndex: 89 }} onClick={() => setPop(false)} />
          <div className="theme-pop" role="dialog" aria-label="外观" style={{ right }}>
            <div className="tp-h">外观模式</div>
            <ModeSeg />
            <div className="tp-h" style={{ marginTop: 14 }}>
              主题色
            </div>
            <AccentSwatches />
            <div className="tray-sep" style={{ margin: "12px 0 6px" }} />
            <button
              type="button"
              className="tray-it tr"
              onClick={() => {
                setPop(false);
                onOpenSettings();
              }}
            >
              <Settings />
              <span className="flex-1">更多外观设置</span>
              <ChevronRight />
            </button>
          </div>
        </>
      )}
    </>
  );
}
