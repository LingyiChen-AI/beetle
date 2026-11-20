// 人员选择：边输边搜（停顿 350ms 自动查），候选列在输入框下方；选中后显示成可清除的标签。
// 飞书项目处理人、飞书任务负责人共用，搜索函数由调用方给。
import { LoaderCircle, Search, User, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { errText } from "../../api";

export interface PersonHit {
  id: string;
  name: string;
  /** 部门 / 邮箱，候选里灰字显示 */
  sub?: string;
}

export function PersonPicker({
  id,
  value,
  placeholder = "输入姓名搜索",
  search,
  onChange,
}: {
  id: string;
  value: { id: string; name: string } | null;
  placeholder?: string;
  search: (q: string) => Promise<PersonHit[]>;
  onChange: (v: { id: string; name: string } | null) => void;
}) {
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<PersonHit[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const box = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const text = q.trim();
    if (!text) {
      setHits(null);
      return;
    }
    let live = true;
    const t = setTimeout(() => {
      setBusy(true);
      setErr(null);
      search(text)
        .then((r) => live && setHits(r))
        .catch((e) => live && setErr(errText(e)))
        .finally(() => live && setBusy(false));
    }, 350);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [q, search]);

  // 点外面收起候选
  useEffect(() => {
    if (hits === null) return;
    const off = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) setHits(null);
    };
    document.addEventListener("mousedown", off);
    return () => document.removeEventListener("mousedown", off);
  }, [hits]);

  if (value)
    return (
      <div className="inp flex items-center" style={{ gap: 6 }} data-testid={`${id}-picked`}>
        <User style={{ width: 14, height: 14, color: "var(--muted)", flex: "none" }} />
        <span className="min-w-0 flex-1 truncate">{value.name}</span>
        <button type="button" className="lnk" aria-label={`清除 ${value.name}`} onClick={() => onChange(null)} style={{ display: "inline-flex" }}>
          <X style={{ width: 14, height: 14 }} />
        </button>
      </div>
    );

  return (
    <div ref={box} className="relative">
      <div className="relative">
        <Search style={{ position: "absolute", left: 10, top: "50%", transform: "translateY(-50%)", width: 14, height: 14, color: "var(--dim)" }} />
        <input
          id={id}
          className="inp"
          style={{ paddingLeft: 30 }}
          placeholder={placeholder}
          value={q}
          autoComplete="off"
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.preventDefault();
            if (e.key === "Escape") setHits(null);
          }}
        />
        {busy && <LoaderCircle className="spin" style={{ position: "absolute", right: 10, top: "50%", marginTop: -7, width: 14, height: 14, color: "var(--dim)" }} />}
      </div>
      {(hits || err) && (
        <div
          role="listbox"
          aria-label="候选人员"
          className="card"
          style={{ position: "absolute", left: 0, right: 0, top: "calc(100% + 4px)", zIndex: 30, padding: 4, maxHeight: 220, overflowY: "auto", boxShadow: "var(--shadow-pop, 0 8px 24px rgba(0,0,0,.25))" }}
        >
          {err && <div style={{ padding: "6px 8px", fontSize: 12, color: "var(--danger-fg)" }}>{err}</div>}
          {hits?.length === 0 && <div style={{ padding: "6px 8px", fontSize: 12, color: "var(--muted)" }}>没有找到「{q.trim()}」</div>}
          {hits?.map((h) => (
            <button
              key={h.id}
              type="button"
              role="option"
              aria-selected={false}
              className="tr flex w-full items-center"
              style={{ gap: 8, padding: "6px 8px", borderRadius: 6, textAlign: "left", background: "transparent", border: 0, color: "var(--fg)", cursor: "pointer" }}
              onMouseEnter={(e) => (e.currentTarget.style.background = "var(--hover, rgba(127,127,127,.12))")}
              onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}
              onClick={() => {
                onChange({ id: h.id, name: h.name });
                setQ("");
                setHits(null);
              }}
            >
              <span>{h.name}</span>
              {h.sub && <span className="min-w-0 flex-1 truncate" style={{ fontSize: 12, color: "var(--muted)" }}>{h.sub}</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
