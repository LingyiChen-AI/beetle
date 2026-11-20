// 技能中心：本产品自己做的技能，安装到本机 Claude Code / Codex，在自己的会话里直接用（不经过飞书群）。
// 安装时在弹窗里勾选装到哪里；有新版本显示「更新」，已装的按位置「卸载」。点卡片打开详情弹窗，可以浏览技能里的文件。
// 查库用的数据源在独立的「数据源」页配置。
import { Check, ChevronRight, Download, FolderOpen, Puzzle, RefreshCw, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { catalogInstall, catalogList, catalogRefreshConfig, errText, SKILL_TARGETS, type CatalogEntry, type EnvItem, type SkillTarget } from "../../api";
import { PageHead, useToast } from "../common";
import SkillDetail from "../skills/SkillDetail";
import { detected, missing, REQUIRE_LABEL, skillIcon, targetLabel, useSkillActions } from "../skills/shared";
import { Checkbox, EmptyState, LoadingState, Modal } from "../ui";

function InstallDialog({ entry, env, onClose, onDone }: { entry: CatalogEntry; env: EnvItem[]; onClose: () => void; onDone: () => void }) {
  const toast = useToast();
  const [picked, setPicked] = useState<SkillTarget[]>(() => {
    const found = SKILL_TARGETS.filter((t) => detected(env, t.id)).map((t) => t.id);
    return found.length > 0 ? found : ["claude"];
  });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const toggle = (t: SkillTarget) => setPicked((p) => (p.includes(t) ? p.filter((x) => x !== t) : [...p, t]));

  const submit = async () => {
    setError(null);
    setBusy(true);
    try {
      // 保持 Claude Code 在前的固定顺序
      const targets = SKILL_TARGETS.map((t) => t.id).filter((t) => picked.includes(t));
      await catalogInstall(entry.name, targets);
      toast(`已安装「${entry.display_name}」到 ${targets.map(targetLabel).join("、")}`);
      onDone();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      ariaLabel={`安装 ${entry.display_name}`}
      icon={Download}
      title={
        <>
          安装「{entry.display_name}」<span className="mono" style={{ color: "var(--dim)", fontSize: 12, fontWeight: 400 }}>v{entry.version}</span>
        </>
      }
      subtitle="选择安装到哪里，装好后在对应的会话里直接说需求即可使用"
      width={480}
      onClose={onClose}
      closeDisabled={busy}
      footer={
        <>
          <button type="button" className="btn tr" disabled={busy} onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn btn-pri tr" disabled={busy || picked.length === 0} onClick={() => void submit()}>
            <Check />
            {busy ? "安装中…" : "确认安装"}
          </button>
        </>
      }
    >
      <div className="grid" style={{ gap: 8 }}>
        {SKILL_TARGETS.map((t) => {
          const installed = entry.installed[t.id];
          const hint = installed
            ? entry.update_available[t.id]
              ? `已安装 v${installed}，将更新到 v${entry.version}`
              : `已安装 v${installed}，将重新安装`
            : detected(env, t.id)
              ? "已检测到 CLI"
              : "未检测到 CLI";
          return (
            <Checkbox
              key={t.id}
              variant="card"
              checked={picked.includes(t.id)}
              disabled={busy}
              onChange={() => toggle(t.id)}
              label={t.label}
              right={<span style={{ color: installed || detected(env, t.id) ? "var(--muted)" : "var(--dim)", fontSize: 12, marginTop: 1 }}>{hint}</span>}
            />
          );
        })}
      </div>
      {error && (
        <div className="alert-err" role="alert" style={{ marginTop: 12 }}>
          {error}
        </div>
      )}
    </Modal>
  );
}

function SkillCard({
  entry,
  env,
  onInstall,
  onOpen,
  onChanged,
}: {
  entry: CatalogEntry;
  env: EnvItem[];
  onInstall: () => void;
  onOpen: () => void;
  onChanged: () => Promise<void>;
}) {
  const { error, busy, update, uninstall } = useSkillActions(entry, onChanged);
  const Icon = skillIcon(entry.name);
  const installedTargets = SKILL_TARGETS.filter((t) => entry.installed[t.id]);

  return (
    <div className="card skill-card tr" data-testid={`catalog-${entry.name}`}>
      <button type="button" className="skill-card-main" aria-label={`查看 ${entry.display_name} 详情`} onClick={onOpen}>
        <div className="tile">
          <Icon style={{ width: 18, height: 18, color: "var(--accent-text)" }} />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span style={{ fontWeight: 600 }}>{entry.display_name}</span>
            <span className="chip c-mut mono" style={{ height: 18, fontSize: 11 }}>
              v{entry.version}
            </span>
          </div>
          <div className="mono" style={{ color: "var(--dim)", fontSize: 11 }}>
            {entry.name}
          </div>
          <div style={{ color: "var(--muted)", fontSize: 12, marginTop: 4, lineHeight: 1.6 }}>{entry.description}</div>
          {entry.requires.length > 0 && (
            <div className="flex flex-wrap items-center gap-1" style={{ marginTop: 8, fontSize: 12 }}>
              <span style={{ color: "var(--dim)", marginRight: 2 }}>依赖</span>
              {entry.requires.map((r) => {
                const bad = missing(env, r);
                return (
                  <span
                    key={r}
                    className={`chip ${bad ? "c-err" : "c-mut"}`}
                    style={{ height: 18, fontSize: 11 }}
                    data-testid={`req-${entry.name}-${r}`}
                    title={bad ? "环境检查未通过，请到「环境」页处理" : undefined}
                  >
                    {REQUIRE_LABEL[r] ?? r}
                    {bad ? "（缺少）" : ""}
                  </span>
                );
              })}
            </div>
          )}
        </div>
        <span className="skill-card-more">
          <FolderOpen />
          查看文件
          <ChevronRight />
        </span>
      </button>

      <div style={{ margin: "0 16px", padding: "10px 0 14px", borderTop: "1px solid var(--line)", fontSize: 12 }}>
        {installedTargets.length === 0 && <div style={{ color: "var(--muted)" }}>未安装</div>}
        {installedTargets.map((t) => (
          <div key={t.id} className="flex items-center gap-2" style={{ marginBottom: 6 }}>
            <span className="chip c-ok">
              已装到 {t.label} v{entry.installed[t.id]}
            </span>
            {entry.update_available[t.id] && <span className="chip c-warn">有新版本 v{entry.version}</span>}
            <span className="flex-1" />
            {entry.update_available[t.id] && (
              <button type="button" className="btn btn-sm tr" disabled={busy} aria-label={`更新 ${t.label}`} onClick={() => void update(t.id)}>
                <RefreshCw />
                更新
              </button>
            )}
            <button type="button" className="btn btn-ghost btn-sm tr" disabled={busy} aria-label={`从 ${t.label} 卸载`} onClick={() => uninstall(t.id)}>
              <Trash2 />
              卸载
            </button>
          </div>
        ))}
        {error && (
          <div className="alert-err" role="alert" style={{ margin: "6px 0" }}>
            {error}
          </div>
        )}
        <div className="flex justify-end" style={{ marginTop: 6 }}>
          <button type="button" className="btn btn-pri btn-sm tr" disabled={busy} aria-label={`安装 ${entry.display_name}`} onClick={onInstall}>
            <Download />
            安装
          </button>
        </div>
      </div>
    </div>
  );
}

export default function SkillsPage({ env }: { env: EnvItem[] }) {
  const toast = useToast();
  const [entries, setEntries] = useState<CatalogEntry[] | null>(null);
  const [installing, setInstalling] = useState<CatalogEntry | null>(null);
  /** 详情弹窗里的技能名（列表刷新后用最新的条目渲染） */
  // 支持 #page=skills&skill=<name> 直接打开详情（便于从别处跳转和截图核对）
  const [viewing, setViewing] = useState<string | null>(() =>
    new URLSearchParams(window.location.hash.replace(/^#/, "")).get("skill"),
  );
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setEntries(await catalogList());
    } catch (e) {
      setError(errText(e));
    }
  }, []);
  useEffect(() => {
    void reload();
  }, [reload]);

  const syncConfig = async () => {
    setError(null);
    try {
      const n = await catalogRefreshConfig();
      toast(n > 0 ? `已把最新配置同步到 ${n} 处已安装的技能` : "没有需要同步配置的已安装技能");
    } catch (e) {
      setError(errText(e));
    }
  };

  return (
    <>
      <PageHead
        title="技能中心"
        sub="本产品自己做的技能，安装到本机 Claude Code / Codex，在自己的会话里直接使用"
        right={
          <button
            type="button"
            className="btn tr"
            title="改了项目或数据源后，点这里让已安装的技能用上新配置"
            onClick={() => void syncConfig()}
          >
            <RefreshCw />
            同步配置
          </button>
        }
      />
      <div className="scroll flex-1" style={{ padding: "16px 24px" }}>
        {error && (
          <div className="alert-err" role="alert" style={{ marginBottom: 12 }}>
            {error}
          </div>
        )}
        {entries === null && !error && <LoadingState text="正在读取技能中心…" />}
        {entries?.length === 0 && (
          <div className="card">
            <EmptyState icon={Puzzle} title="技能中心里还没有技能" desc="新版本客户端会随包带上技能，升级后再来看看" />
          </div>
        )}
        <div className="grid grid-cols-2" style={{ gap: 12 }}>
          {entries?.map((e) => (
            <SkillCard key={e.name} entry={e} env={env} onInstall={() => setInstalling(e)} onOpen={() => setViewing(e.name)} onChanged={reload} />
          ))}
        </div>
      </div>
      {viewing && entries?.some((e) => e.name === viewing) && (
        <SkillDetail
          entry={entries.find((e) => e.name === viewing)!}
          env={env}
          onClose={() => setViewing(null)}
          onInstall={() => setInstalling(entries.find((e) => e.name === viewing) ?? null)}
          onChanged={reload}
        />
      )}
      {installing && (
        <InstallDialog
          entry={installing}
          env={env}
          onClose={() => setInstalling(null)}
          onDone={() => {
            setInstalling(null);
            void reload();
          }}
        />
      )}
    </>
  );
}
