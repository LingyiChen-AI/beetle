// 二维码：把文本（如飞书授权链接）画成 SVG。白底黑码（深色主题下也保持白底，手机才扫得出来），四周留静区。
import qrcode from "qrcode-generator";
import { useMemo } from "react";

export default function QrCode({
  text,
  size = 168,
  label,
}: {
  text: string;
  size?: number;
  label?: string;
}) {
  const { path, count } = useMemo(() => {
    const qr = qrcode(0, "M"); // 0 = 按内容自动选版本
    qr.addData(text, "Byte");
    qr.make();
    const n = qr.getModuleCount();
    let d = "";
    for (let r = 0; r < n; r++)
      for (let c = 0; c < n; c++)
        if (qr.isDark(r, c)) d += `M${c} ${r}h1v1h-1z`;
    return { path: d, count: n };
  }, [text]);
  const quiet = 2; // 静区（模块数）
  const box = count + quiet * 2;
  return (
    <svg
      role="img"
      aria-label={label ?? "二维码"}
      data-testid="qr-code"
      width={size}
      height={size}
      viewBox={`${-quiet} ${-quiet} ${box} ${box}`}
      shapeRendering="crispEdges"
      style={{ background: "var(--qr-bg)", borderRadius: 8, display: "block" }}
    >
      <path d={path} fill="var(--qr-fg)" />
    </svg>
  );
}
