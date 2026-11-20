// 项目表单里「建飞书任务」的配置：任务清单（lark-cli 列出可见清单）、负责人（通讯录边输边搜）、截止天数，都可不填。
import { useCallback, useEffect, useState } from "react";
import { errText, larkTasklists, larkUsers, type FeishuTaskTarget, type Tasklist } from "../../api";
import { Select } from "../ui";
import { SubCard } from "./MeegleFields";
import { PersonPicker, type PersonHit } from "./PersonPicker";

export const emptyTask = (): FeishuTaskTarget => ({ tasklist_guid: "", tasklist_name: "", assignee_open_id: "", assignee_name: "", due_days: 0 });

const NONE = "__none__";

export function TaskFields({ value, k, onChange }: { value: FeishuTaskTarget | null | undefined; k: string; onChange: (t: FeishuTaskTarget) => void }) {
  const t = value ?? emptyTask();
  const [lists, setLists] = useState<Tasklist[] | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    larkTasklists()
      .then((v) => live && setLists(v))
      .catch((e) => live && setErr(errText(e)));
    return () => {
      live = false;
    };
  }, []);

  const searchUsers = useCallback(
    async (q: string): Promise<PersonHit[]> => (await larkUsers(q)).map((u) => ({ id: u.open_id, name: u.name, sub: u.department })),
    [],
  );

  return (
    <SubCard title="飞书任务" testId={`task-${k}`} extra={<span style={{ fontSize: 12, color: "var(--dim)" }}>以你的飞书身份创建，都可不填</span>}>
      <div className="grid" style={{ gridTemplateColumns: "1fr 1fr 110px", gap: 12 }}>
        <div>
          <label className="lbl" htmlFor={`tk-list-${k}`}>
            任务清单
          </label>
          <Select
            id={`tk-list-${k}`}
            testId={`tk-list-${k}`}
            placeholder={lists === null ? "加载中…" : "不放进清单"}
            value={t.tasklist_guid || NONE}
            options={[{ value: NONE, label: "不放进清单" }, ...(lists ?? []).map((l) => ({ value: l.guid, label: l.name || l.guid }))]}
            onChange={(g) => onChange({ ...t, tasklist_guid: g === NONE ? "" : g, tasklist_name: g === NONE ? "" : (lists?.find((l) => l.guid === g)?.name ?? "") })}
          />
        </div>
        <div>
          <label className="lbl" htmlFor={`tk-user-${k}`}>
            负责人<span style={{ color: "var(--dim)", fontWeight: 400 }}>（不填 = 你自己）</span>
          </label>
          <PersonPicker
            id={`tk-user-${k}`}
            placeholder="输入姓名、拼音或邮箱搜索"
            value={t.assignee_open_id ? { id: t.assignee_open_id, name: t.assignee_name } : null}
            search={searchUsers}
            onChange={(v) => onChange({ ...t, assignee_open_id: v?.id ?? "", assignee_name: v?.name ?? "" })}
          />
        </div>
        <div>
          <label className="lbl" htmlFor={`tk-due-${k}`}>
            截止
          </label>
          <div className="relative">
            <input
              id={`tk-due-${k}`}
              className="inp mono"
              style={{ paddingRight: 34 }}
              type="number"
              min={0}
              max={365}
              placeholder="不设"
              value={t.due_days || ""}
              onChange={(e) => onChange({ ...t, due_days: Math.max(0, Math.min(365, Number(e.target.value) || 0)) })}
            />
            <span style={{ position: "absolute", right: 10, top: "50%", transform: "translateY(-50%)", fontSize: 12, color: "var(--dim)", pointerEvents: "none" }}>天后</span>
          </div>
        </div>
      </div>
      {err && (
        <div className="alert-err" style={{ fontSize: 12, padding: "6px 10px" }} data-testid={`task-error-${k}`}>
          {err}
        </div>
      )}
    </SubCard>
  );
}
