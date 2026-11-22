// 群：自动接入开关 + 群卡片（负责项目、记录文档、谁能触发、编辑 / 移除）+ 涉及的项目。
import { openUrl } from "@tauri-apps/plugin-opener";
import { Check, ChevronRight, FileText, FolderGit2, Hash, LogIn, Pencil, Plus, Trash2, X } from "lucide-react";
import { useState } from "react";
import {
  errText,
  removeGroup,
  saveBot,
  saveGroup,
  type BotView,
  type Group,
  type ProjectsView,
  type RunRecord,
  type Trigger,
} from "../../api";
import { isToday, relTime } from "../../format";
import { Switch, useToast } from "../common";
import { Checkbox, ChipMultiSelect, EmptyState, HelpTip, Modal } from "../ui";

/** 「谁能触发」的选项文字（概览、向导、群 tab 共用）。 */
export const TRIGGER_LABEL: Record<Trigger, string> = { owner: "仅机器人主人", all: "群内所有人" };
export const OWNER_HINT = "机器人主人 = 安装本客户端、登录 lark-cli 的人";
export const AUTO_ONBOARD_DESC = "机器人被拉进新群后，由你 @ 一次即自动登记";

/** 群的目录名默认取 chat_id 后 8 位（和自动接入保持一致）。 */
export const defaultSlug = (chatId: string) => `beetle-${chatId.slice(-8)}`;

function TriggerSeg({ value, onChange, label }: { value: Trigger; onChange: (t: Trigger) => void; label: string }) {
  return (
    <div className="seg" role="radiogroup" aria-label={label}>
      {(
        [
          ["owner", TRIGGER_LABEL.owner],
          ["all", TRIGGER_LABEL.all],
        ] as const
      ).map(([k, t]) => (
        <button key={k} type="button" role="radio" aria-checked={value === k} className={`tr ${value === k ? "on" : ""}`} onClick={() => onChange(k)}>
          {t}
        </button>
      ))}
    </div>
  );
}

function GroupCard({
  botId,
  group,
  isNew,
  runs,
  projectNames,
  onDone,
  onCancel,
  onGoProjects,
}: {
  botId: string;
  group: Group;
  isNew?: boolean;
  runs: RunRecord[];
  projectNames: string[];
  onDone: () => Promise<void> | void;
  onCancel?: () => void;
  onGoProjects?: () => void;
}) {
  const toast = useToast();
  const [editing, setEditing] = useState(!!isNew);
  const [g, setG] = useState<Group>(group);
  const [error, setError] = useState<string | null>(null);
  const mine = runs.filter((r) => r.chat_id === group.chat_id);
  const today = mine.filter((r) => isToday(r.started_at)).length;
  const last = mine[0];

  const save = async (next: Group) => {
    setError(null);
    try {
      await saveGroup(botId, { ...next, slug: next.slug.trim() || defaultSlug(next.chat_id), doc_url: next.doc_url?.trim() || null });
      setEditing(false);
      await onDone();
      return true;
    } catch (e) {
      setError(errText(e));
      return false;
    }
  };

  const remove = async () => {
    if (!window.confirm(`确定把群「${group.name}」从这个机器人移除吗？群工作目录和记录文档不会删除。`)) return;
    try {
      await removeGroup(botId, group.chat_id);
      toast(`已移除群「${group.name}」`, "warn");
      await onDone();
    } catch (e) {
      setError(errText(e));
    }
  };

  return (
    <div className="card" style={{ padding: 14 }} data-testid={`group-${group.chat_id || "new"}`}>
      <div className="flex items-center" style={{ gap: 10 }}>
        <div className="grid place-items-center" style={{ width: 30, height: 30, borderRadius: 8, background: "var(--subtle)" }}>
          <Hash style={{ width: 15, height: 15, color: "var(--muted)" }} />
        </div>
        <div className="min-w-0 flex-1">
          {editing ? (
            <div className="flex gap-2">
              <input className="inp" style={{ height: 28, maxWidth: 240 }} aria-label="群名称" placeholder="群名称" value={g.name} onChange={(e) => setG({ ...g, name: e.target.value })} />
              {isNew && (
                <input
                  className="inp mono"
                  style={{ height: 28, maxWidth: 260 }}
                  aria-label="群 chat_id"
                  placeholder="chat_id：oc_ 开头，可在群设置里复制"
                  value={g.chat_id}
                  onChange={(e) => setG({ ...g, chat_id: e.target.value.trim() })}
                />
              )}
            </div>
          ) : (
            <>
              <div style={{ fontWeight: 600 }}>{group.name}</div>
              <div className="mono" style={{ color: "var(--dim)", fontSize: 11 }}>
                {group.chat_id}
              </div>
            </>
          )}
        </div>
        {!isNew && (
          <div style={{ textAlign: "right", marginRight: 8 }}>
            <span className="mono" style={{ fontWeight: 600 }}>
              {today}
            </span>
            <span style={{ color: "var(--muted)", fontSize: 12 }}> 次今日 · 最后 {last ? relTime(last.started_at) : "—"}</span>
          </div>
        )}
        {editing ? (
          <>
            <button type="button" className="btn btn-pri btn-sm tr" onClick={() => void save(g)}>
              <Check />
              {isNew ? "添加群" : "保存"}
            </button>
            <button
              type="button"
              className="btn btn-ghost btn-sm tr"
              onClick={() => {
                setG(group);
                setEditing(false);
                onCancel?.();
              }}
            >
              <X />
              取消
            </button>
          </>
        ) : (
          <>
            <button type="button" className="btn btn-sm tr" onClick={() => setEditing(true)}>
              <Pencil />
              编辑
            </button>
            <button type="button" className="btn btn-ghost btn-sm tr" aria-label={`移除群 ${group.name}`} onClick={() => void remove()}>
              <Trash2 />
              移除
            </button>
          </>
        )}
      </div>
      {error && (
        <div className="alert-err" role="alert" style={{ marginTop: 10, fontSize: 12 }}>
          {error}
        </div>
      )}
      <div className="grid" style={{ gridTemplateColumns: "1.2fr 1.2fr 1fr", gap: 14, marginTop: 12, paddingTop: 12, borderTop: "1px solid var(--line)" }}>
        <div>
          <span className="lbl flex items-center gap-1.5">
            负责项目{editing ? "（可多选）" : ""}
            {editing && (
              <HelpTip text="机器人在这个群里查问题时用哪些项目的代码和库。勾上「全部项目」后，这里选的项目优先，问到别的项目时由 AI 自行定位" label="负责项目" />
            )}
          </span>
          {editing ? (
            <>
            <ChipMultiSelect
              ariaLabel="负责项目"
              mono
              options={projectNames}
              value={g.projects}
              onChange={(projects) => setG((x) => ({ ...x, projects }))}
              empty={
                <span className="flex items-center gap-2" style={{ color: "var(--dim)", fontSize: 12 }}>
                  还没有项目
                  {onGoProjects && (
                    <button type="button" className="lnk" onClick={onGoProjects}>
                      去添加项目
                    </button>
                  )}
                </span>
              }
            />
            <Checkbox
              className="mt-2"
              checked={!!g.all_projects}
              onChange={(v) => setG((x) => ({ ...x, all_projects: v }))}
              label="全部项目，AI 自行定位"
              description="问到没关联的项目也能查，上面选的项目优先"
            />
            </>
          ) : (
            <div className="flex flex-wrap gap-1.5">
              {group.all_projects && (
                <span className="chip c-blue" title="全部项目都可查，由 AI 按问题自行定位项目">
                  AI 定位全部项目
                </span>
              )}
              {group.projects.map((name) => (
                <span key={name} className="chip c-mut">
                  <FolderGit2 />
                  <span className="mono">{name}</span>
                </span>
              ))}
              {group.projects.length === 0 && !group.all_projects && <span style={{ color: "var(--dim)", fontSize: 12 }}>还没有关联项目</span>}
              {!isNew && (
                <button type="button" className="chip chip-add tr" aria-label={`给 ${group.name} 添加项目`} onClick={() => setEditing(true)}>
                  <Plus />
                  添加
                </button>
              )}
            </div>
          )}
        </div>
        <div className="min-w-0">
          <span className="lbl">记录文档</span>
          {editing ? (
            <input
              className="inp"
              style={{ height: 28 }}
              aria-label="记录文档链接"
              placeholder="飞书文档链接（留空则自动新建）"
              value={g.doc_url ?? ""}
              onChange={(e) => setG({ ...g, doc_url: e.target.value })}
            />
          ) : group.doc_url ? (
            <button type="button" className="lnk flex max-w-full items-center gap-1 truncate" onClick={() => void openUrl(group.doc_url as string)}>
              <FileText style={{ width: 13, height: 13 }} />
              <span className="truncate">{group.doc_url}</span>
            </button>
          ) : (
            <span style={{ color: "var(--dim)", fontSize: 12 }}>首次运行时自动新建</span>
          )}
        </div>
        <div>
          <span className="lbl flex items-center gap-1.5">
            谁能触发
            <HelpTip text={OWNER_HINT} label="仅机器人主人" />
          </span>
          <TriggerSeg
            label={`谁能触发 ${group.name}`}
            value={editing ? g.trigger : group.trigger}
            onChange={(t) => (editing ? setG({ ...g, trigger: t }) : void save({ ...group, trigger: t }))}
          />
        </div>
      </div>
    </div>
  );
}

/** 设置机器人负责的项目：默认选中各群已有的项目；保存后所选群的「负责项目」就是这里选中的（新勾的加进去，取消的移除）。 */
function AddProjectsDialog({
  botId,
  groups,
  projectNames,
  onGoProjects,
  onClose,
  onDone,
}: {
  botId: string;
  groups: Group[];
  projectNames: string[];
  onGoProjects: () => void;
  onClose: () => void;
  onDone: () => Promise<void>;
}) {
  const toast = useToast();
  // 默认选中：所有群已有项目的并集（按项目页的顺序）
  const [picked, setPicked] = useState<string[]>(() => projectNames.filter((n) => groups.some((g) => g.projects.includes(n))));
  const [targets, setTargets] = useState<string[]>(groups.map((g) => g.chat_id));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      for (const g of groups.filter((x) => targets.includes(x.chat_id))) {
        // 保留群里已有项目的原顺序，新加的排在后面
        const next = [...g.projects.filter((p) => picked.includes(p)), ...picked.filter((p) => !g.projects.includes(p))];
        if (next.join("\n") !== g.projects.join("\n")) await saveGroup(botId, { ...g, projects: next });
      }
      toast(picked.length ? `已保存：${targets.length} 个群负责 ${picked.join("、")}` : `已保存：${targets.length} 个群改用默认项目`);
      await onDone();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      ariaLabel="添加项目"
      icon={FolderGit2}
      title="机器人负责的项目"
      subtitle="已负责的项目默认选中：勾选新项目即可添加，取消勾选会从所选群里移除；机器人在这些群里查问题时会用到它们的代码和数据源"
      width={560}
      onClose={onClose}
      closeDisabled={busy}
      footer={
        <>
          <span className="flex-1" />
          <button type="button" className="btn tr" disabled={busy} onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn btn-pri tr" disabled={busy || targets.length === 0} onClick={() => void submit()}>
            <Check />
            {busy ? "保存中…" : "保存"}
          </button>
        </>
      }
    >
      <div className="grid" style={{ gap: 14 }}>
        <div>
          <span className="lbl">项目（可多选；都不选 = 用默认项目）</span>
          <ChipMultiSelect
            ariaLabel="要添加的项目"
            mono
            options={projectNames}
            value={picked}
            onChange={setPicked}
            empty={
              <span className="flex items-center gap-2" style={{ color: "var(--dim)", fontSize: 12 }}>
                还没有项目
                <button type="button" className="lnk" onClick={onGoProjects}>
                  去项目页添加
                </button>
              </span>
            }
          />
        </div>
        <div>
          <span className="lbl">加到哪些群</span>
          <div className="grid" style={{ gap: 6 }}>
            {groups.map((g) => (
              <Checkbox
                key={g.chat_id}
                variant="card"
                checked={targets.includes(g.chat_id)}
                onChange={() => setTargets((t) => (t.includes(g.chat_id) ? t.filter((x) => x !== g.chat_id) : [...t, g.chat_id]))}
                label={g.name}
                right={<span style={{ color: "var(--dim)", fontSize: 12 }}>{g.projects.length ? `已有 ${g.projects.join("、")}` : "用默认项目"}</span>}
              />
            ))}
          </div>
        </div>
        {error && (
          <div className="alert-err" role="alert">
            {error}
          </div>
        )}
      </div>
    </Modal>
  );
}

export default function GroupsTab({
  view,
  runs,
  projects,
  onChanged,
  onGoProjects,
}: {
  view: BotView;
  runs: RunRecord[];
  projects: ProjectsView | null;
  onChanged: () => Promise<void> | void;
  onGoProjects: () => void;
}) {
  const { bot } = view;
  const toast = useToast();
  const [adding, setAdding] = useState(false);
  const [addingProjects, setAddingProjects] = useState(false);
  const projectNames = Object.keys(projects?.projects ?? {});
  const involved = projectNames.filter((n) => bot.groups.some((g) => g.projects.includes(n)));

  const toggleAuto = async (on: boolean) => {
    try {
      await saveBot({ ...bot, auto_onboard: on });
      await onChanged();
      toast(on ? "已开启自动接入新群" : "已关闭自动接入新群");
    } catch (e) {
      toast(errText(e), "err");
    }
  };

  return (
    <div className="scroll flex-1" style={{ padding: "18px 24px" }}>
      {addingProjects && (
        <AddProjectsDialog
          botId={bot.id}
          groups={bot.groups}
          projectNames={projectNames}
          onGoProjects={onGoProjects}
          onClose={() => setAddingProjects(false)}
          onDone={async () => {
            setAddingProjects(false);
            await onChanged();
          }}
        />
      )}
      <div className="card flex items-center" style={{ padding: "12px 14px", gap: 12, marginBottom: 12 }}>
        <LogIn style={{ width: 18, height: 18, color: "var(--accent-text)" }} />
        <div className="flex-1">
          <div style={{ fontWeight: 600 }}>自动接入新群</div>
          <div style={{ color: "var(--muted)", fontSize: 12, marginTop: 2 }}>
            {AUTO_ONBOARD_DESC}（自动建群工作目录、使用默认项目、新建记录文档）
          </div>
        </div>
        <Switch on={bot.auto_onboard} label="自动接入新群" onChange={(on) => void toggleAuto(on)} />
      </div>

      <div className="grid" style={{ gap: 10 }}>
        {bot.groups.map((g) => (
          <GroupCard key={g.chat_id} botId={bot.id} group={g} runs={runs} projectNames={projectNames} onDone={onChanged} onGoProjects={onGoProjects} />
        ))}
        {adding ? (
          <GroupCard
            botId={bot.id}
            isNew
            group={{ chat_id: "", name: "", slug: "", projects: [], doc_url: null, trigger: "owner", all_projects: false }}
            runs={[]}
            projectNames={projectNames}
            onGoProjects={onGoProjects}
            onDone={async () => {
              setAdding(false);
              await onChanged();
            }}
            onCancel={() => setAdding(false)}
          />
        ) : (
          <button
            type="button"
            className="btn tr justify-center"
            style={{ borderStyle: "dashed", background: "transparent" }}
            onClick={() => setAdding(true)}
          >
            <Plus />
            手动添加群
          </button>
        )}
      </div>

      <div className="card" style={{ marginTop: 12 }}>
        <div className="card-h">
          <h3>
            <FolderGit2 />
            涉及的项目
          </h3>
          <div className="flex items-center gap-1">
            <button type="button" className="btn btn-sm tr" disabled={bot.groups.length === 0} title={bot.groups.length === 0 ? "先添加群" : undefined} onClick={() => setAddingProjects(true)}>
              <Plus />
              添加项目
            </button>
            <button type="button" className="btn btn-ghost btn-sm tr" onClick={onGoProjects}>
              项目管理 <ChevronRight />
            </button>
          </div>
        </div>
        {involved.length === 0 ? (
          <EmptyState compact icon={FolderGit2} title="群里还没有指定负责项目" desc="点右上角「添加项目」，或在群卡片的「负责项目」里添加；不选时用默认项目" />
        ) : (
          <div className="t-scroll">
          <table className="t">
            <thead>
              <tr>
                <th>项目</th>
                <th>仓库</th>
                <th>分支规则</th>
                <th>关联数据源</th>
                <th>所在群</th>
              </tr>
            </thead>
            <tbody>
              {involved.map((n) => {
                const pr = projects!.projects[n];
                return (
                  <tr key={n}>
                    <td className="mono nw" style={{ fontWeight: 600 }}>
                      {n}
                    </td>
                    <td className="mono brk" style={{ color: "var(--muted)", fontSize: 12 }}>
                      {pr.repo}
                    </td>
                    <td className="mono nw" style={{ color: "var(--muted)", fontSize: 12 }}>
                      {pr.branch_glob} 取最新
                    </td>
                    <td>
                      <div className="t-chips">
                        {pr.dbs.length ? (
                          pr.dbs.map((d) => (
                            <span key={d} className="chip c-blue mono">
                              {d}
                            </span>
                          ))
                        ) : (
                          <span style={{ color: "var(--dim)", fontSize: 12 }}>无</span>
                        )}
                      </div>
                    </td>
                    <td>
                      <div className="t-chips">
                        {bot.groups
                          .filter((g) => g.projects.includes(n))
                          .map((g) => (
                            <span key={g.chat_id} className="chip c-mut">
                              {g.name}
                            </span>
                          ))}
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          </div>
        )}
      </div>
    </div>
  );
}
