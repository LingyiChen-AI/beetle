// 运行日志里代码审查结果的预览：照飞书消息卡片的样子画（蓝色标题头 + 正文），和群里收到的一致。
import { cardLines, cardTitle, severityLabel, type ReviewResult } from "../review/card";

const HEADER_BLUE = "#3370ff";
const SEVERITY_COLOR = { high: "#f54a45", medium: "#ff8800", low: "var(--muted)" } as const;

export default function ReviewCardPreview({ result }: { result: ReviewResult }) {
  return (
    <div
      data-testid="review-card"
      style={{ width: 520, maxWidth: "100%", borderRadius: 8, overflow: "hidden", border: "1px solid var(--line)", background: "var(--panel, transparent)" }}
    >
      <div data-testid="review-card-title" style={{ background: HEADER_BLUE, color: "#fff", fontWeight: 600, padding: "10px 14px" }}>
        {cardTitle(result)}
      </div>
      <div style={{ padding: "10px 14px 12px", display: "grid", gap: 4, lineHeight: 1.6, wordBreak: "break-word" }}>
        {cardLines(result).map((l, i) => {
          switch (l.kind) {
            case "title":
              return <div key={i} style={{ fontWeight: 600 }}>{l.text}</div>;
            case "meta":
              return <div key={i} style={{ color: "var(--muted)", fontSize: 12 }}>{l.text}</div>;
            case "divider":
              return <div key={i} style={{ borderTop: "1px solid var(--line)", margin: "6px 0" }} />;
            case "heading":
              return <div key={i} style={{ fontWeight: 600 }}>{l.text}</div>;
            case "finding":
              return (
                <div key={i} data-testid="review-finding" style={{ paddingLeft: 16, textIndent: -16 }}>
                  {l.index}. <b style={{ color: SEVERITY_COLOR[l.severity ?? "low"] }}>[{severityLabel(l.severity ?? "low")}]</b> {l.text}
                </div>
              );
            case "resolved":
              return (
                <div key={i} data-testid="review-finding" style={{ color: "var(--muted)" }}>
                  [{severityLabel(l.severity ?? "low")}] {l.text}
                </div>
              );
            case "note":
              return <div key={i} style={{ color: "var(--muted)", fontSize: 12 }}>{l.text}</div>;
            case "link":
              return (
                <div key={i} style={{ marginTop: 6 }}>
                  <span className="mono" style={{ display: "inline-block", fontSize: 12, color: "#fff", background: HEADER_BLUE, borderRadius: 6, padding: "3px 10px" }} title={l.text}>
                    查看 PR
                  </span>
                </div>
              );
            default:
              return <div key={i}>{l.text}</div>;
          }
        })}
      </div>
    </div>
  );
}
