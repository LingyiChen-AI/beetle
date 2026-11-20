// 浮层公共逻辑：贴着触发元素定位（fixed，挂到 body，不被滚动容器裁掉；下方放不下就翻到上方），点外部关闭。
import { useCallback, useEffect, useLayoutEffect, useState, type CSSProperties, type RefObject } from "react";

export interface AnchorPos {
  top: number;
  left: number;
  width: number;
  placement: "bottom" | "top";
  maxHeight: number;
}

/** 打开时计算浮层位置，窗口缩放 / 任意容器滚动时跟着更新；关闭时返回 null。 */
export function useAnchoredPosition(anchor: RefObject<HTMLElement | null>, open: boolean, preferredHeight = 280, gap = 4): AnchorPos | null {
  const [pos, setPos] = useState<AnchorPos | null>(null);
  const update = useCallback(() => {
    const el = anchor.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const vh = window.innerHeight || 768;
    const vw = window.innerWidth || 1024;
    const below = vh - r.bottom - gap - 8;
    const above = r.top - gap - 8;
    const placement = below < Math.min(preferredHeight, 160) && above > below ? "top" : "bottom";
    const maxHeight = Math.max(120, Math.min(preferredHeight, placement === "bottom" ? below : above));
    const left = Math.max(8, Math.min(r.left, vw - Math.max(r.width, 160) - 8));
    setPos({ top: placement === "bottom" ? r.bottom + gap : r.top - gap, left, width: r.width, placement, maxHeight });
  }, [anchor, gap, preferredHeight]);

  useLayoutEffect(() => {
    if (!open) {
      setPos(null);
      return;
    }
    update();
    window.addEventListener("resize", update);
    window.addEventListener("scroll", update, true);
    return () => {
      window.removeEventListener("resize", update);
      window.removeEventListener("scroll", update, true);
    };
  }, [open, update]);
  return pos;
}

/** 打开期间在这些元素之外按下鼠标就关闭。 */
export function useOutsideClose(refs: RefObject<HTMLElement | null>[], open: boolean, onClose: () => void) {
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node | null;
      if (t && refs.some((r) => r.current?.contains(t))) return;
      onClose();
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
    // refs 是稳定的 ref 对象
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, onClose]);
}

/** 浮层的 fixed 定位样式。 */
export function popStyle(pos: AnchorPos, minWidth = true): CSSProperties {
  return {
    top: pos.top,
    left: pos.left,
    minWidth: minWidth ? pos.width : undefined,
    maxHeight: pos.maxHeight,
    transform: pos.placement === "top" ? "translateY(-100%)" : undefined,
  };
}
