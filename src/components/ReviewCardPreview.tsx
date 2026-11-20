// 运行日志里代码审查结果的预览：照飞书消息卡片的样子画（蓝色标题头 + 正文），和群里收到的一致。
import { cardLines, cardTitle, severityLabel, type ReviewResult } from "../review/card";

const HEADER_BLUE = "#3370ff";

export default function ReviewCardPreview({ result }: { result: ReviewResult }) {
  return (
    <div
      data-testid="review-card"
      style={{ width: 520, maxWidth: "100%", borderRadius: 8, overflow: "hidden", border: "1px solid var(--line)", background: "var(--panel, transparent)" }}
    >
      <div data-testid="review-card-title" style={{ background: HEADER_BLUE, color: "#fff", fontWeight: 600, padding: "10px 14px" }}>
        {cardTitle(result)}
      </div>
      <div style={{ padding: "10px 14px", display: "grid", gap: 4, lineHeight: 1.6, wordBreak: "break-word" }}>
        {cardLines(result).map((l, i) => {
          if (l.kind === "heading") return <div key={i} style={{ fontWeight: 600, marginTop: 2 }}>{l.text}</div>;
          if (l.kind === "finding")
            return (
              <div key={i} data-testid="review-finding" style={{ paddingLeft: 12, textIndent: -12 }}>
                • <b>[{severityLabel(l.severity ?? "low")}]</b> {l.text}
              </div>
            );
          if (l.kind === "link")
            return (
              <div key={i} className="mono" style={{ fontSize: 12, color: HEADER_BLUE }}>
                {l.text}
              </div>
            );
          return (
            <div key={i}>
              {l.label && <b>{l.label}</b>}
              {l.text}
            </div>
          );
        })}
      </div>
    </div>
  );
}
