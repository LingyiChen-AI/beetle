// 向导 / 首次引导顶部的步骤条（照原型 .steps）。
import { Check } from "lucide-react";
import { Fragment } from "react";

export default function Steps({ cur, names }: { cur: number; names: string[] }) {
  return (
    <div className="flex items-center" style={{ gap: 8 }} aria-label="步骤">
      {names.map((n, i) => {
        const k = i + 1;
        const state = k < cur ? "done" : k === cur ? "on" : "";
        return (
          <Fragment key={n}>
            {i > 0 && <div style={{ flex: 1, height: 1, background: "var(--border)", minWidth: 20 }} />}
            <div
              className="flex items-center"
              aria-current={k === cur ? "step" : undefined}
              style={{ gap: 8, whiteSpace: "nowrap", fontSize: 12, color: k === cur ? "var(--fg)" : "var(--muted)" }}
            >
              <span
                className="grid place-items-center"
                style={{
                  width: 22,
                  height: 22,
                  borderRadius: "50%",
                  fontWeight: 600,
                  fontSize: 11,
                  border: "1px solid var(--border)",
                  ...(state === "on"
                    ? { background: "var(--accent)", borderColor: "var(--accent)", color: "var(--on-accent)" }
                    : state === "done"
                      ? { background: "var(--accent-soft)", borderColor: "color-mix(in srgb, var(--accent) 50%, transparent)", color: "var(--accent-text)" }
                      : {}),
                }}
              >
                {k < cur ? <Check style={{ width: 12, height: 12 }} /> : k}
              </span>
              {n}
            </div>
          </Fragment>
        );
      })}
    </div>
  );
}
