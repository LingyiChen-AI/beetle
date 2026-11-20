// 设置页：外观（模式分段 + 主题色块）、通用（开机自启、关闭到托盘说明）、运行（代理、代码缓存目录、日志保留天数）、
// 配置导入 / 导出、关于。（数据源在独立的「数据源」页）
import { Check, Download, RefreshCw, Sparkles } from "lucide-react";
import { useEffect, useState } from "react";
import {
  errText,
  exportConfig,
  getAppVersion,
  getAutostart,
  getRuntimeSettings,
  setAutostart,
  setRuntimeSettings,
  type RuntimeSettingsView,
} from "../../api";
import { useAppearance } from "../../theme/appearance";
import { AccentSwatches, ModeSeg, PageHead, SettingRow, Switch, useToast } from "../common";
import ImportConfig from "../ImportConfig";
import { useUpdate } from "../UpdateBanner";
import { NumberInput } from "../ui";

/** 代理地址、代码缓存目录、日志保留天数。 */
function RuntimeSettingsCard() {
  const toast = useToast();
  const [view, setView] = useState<RuntimeSettingsView | null>(null);
  const [proxy, setProxy] = useState("");
  const [dir, setDir] = useState("");
  const [days, setDays] = useState("30");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    getRuntimeSettings()
      .then((v) => {
        setView(v);
        setProxy(v.proxy ?? "");
        setDir(v.code_cache_dir ?? "");
        setDays(String(v.log_retention_days));
      })
      .catch((e) => setLoadError(errText(e)));
  }, []);

  const save = async () => {
    setError(null);
    const n = Number(days);
    if (!Number.isInteger(n) || (view && (n < view.min_retention_days || n > view.max_retention_days))) {
      setError(`日志保留天数要在 ${view?.min_retention_days ?? 1}～${view?.max_retention_days ?? 3650} 之间的整数`);
      return;
    }
    setSaving(true);
    try {
      await setRuntimeSettings({ proxy: proxy.trim() || null, code_cache_dir: dir.trim() || null, log_retention_days: n });
      toast("运行设置已保存：代理和代码缓存目录从机器人下一次运行起生效，超过保留天数的记录已清理");
    } catch (e) {
      setError(errText(e));
    } finally {
      setSaving(false);
    }
  };

  if (loadError)
    return (
      <div className="card" style={{ padding: "14px 16px", color: "var(--muted)", fontSize: 12 }} data-testid="runtime-settings">
        读取运行设置失败：{loadError}
      </div>
    );
  return (
    <div className="card" data-testid="runtime-settings">
      <SettingRow title="代理地址" desc="Agent（Claude Code / Codex）访问外网用；留空不走代理。例如 http://127.0.0.1:7890">
        <input className="inp mono" style={{ width: 300 }} aria-label="代理地址" placeholder="不使用代理" value={proxy} onChange={(e) => setProxy(e.target.value)} />
      </SettingRow>
      <SettingRow title="代码缓存目录" desc="机器人同步项目代码的位置（每个分支一个子目录）；留空用默认目录">
        <input
          className="inp mono"
          style={{ width: 300 }}
          aria-label="代码缓存目录"
          placeholder={view?.default_code_cache_dir ?? "默认目录"}
          value={dir}
          onChange={(e) => setDir(e.target.value)}
        />
      </SettingRow>
      <SettingRow title="日志保留天数" desc="运行记录和原始日志保留多少天；保存时和每次启动客户端时清理更早的" last>
        <NumberInput
          width={120}
          min={view?.min_retention_days ?? 1}
          max={view?.max_retention_days ?? 3650}
          ariaLabel="日志保留天数"
          invalid={!!error && error.startsWith("日志保留天数")}
          value={days}
          onChange={setDays}
        />
      </SettingRow>
      {error && (
        <div className="alert-err" role="alert" style={{ margin: "0 16px 12px", fontSize: 12 }}>
          {error}
        </div>
      )}
      <div className="flex justify-end" style={{ padding: "0 16px 14px" }}>
        <button type="button" className="btn btn-pri btn-sm tr" disabled={!view || saving} onClick={() => void save()}>
          <Check />
          保存运行设置
        </button>
      </div>
    </div>
  );
}

/** 版本与更新：自动每 30 分钟检查一次 GitHub Release，也可手动检查；有新版本时可直接更新。 */
function UpdateRow({ version }: { version: string | null }) {
  const { status, busy, error, check, install } = useUpdate();
  const checked = status?.checked_at ? new Date(status.checked_at).toLocaleString() : null;
  const desc = (
    <>
      当前版本 <span className="mono">{version ?? status?.current ?? "—"}</span>
      {status?.available ? (
        <>
          ，发现新版本 <span className="mono">v{status.available}</span>
        </>
      ) : checked && !status?.error ? (
        "，已是最新版本"
      ) : null}
      <br />
      <span style={{ color: error || status?.error ? "var(--danger-fg)" : "var(--dim)" }}>
        {error ?? status?.error ?? (checked ? `上次检查 ${checked}，每 30 分钟自动检查` : "每 30 分钟自动检查一次")}
      </span>
    </>
  );
  return (
    <SettingRow title="版本与更新" desc={desc}>
      <div className="flex items-center gap-2">
        <button type="button" className="btn tr" disabled={busy !== null} onClick={() => void check()}>
          <RefreshCw />
          {busy === "check" ? "检查中…" : "检查更新"}
        </button>
        {status?.available && (
          <button type="button" className="btn btn-pri tr" disabled={busy !== null} onClick={() => void install()}>
            <Download />
            {busy === "install" ? "正在下载安装…" : "立即更新"}
          </button>
        )}
      </div>
    </SettingRow>
  );
}

export default function SettingsPage({ onOpenOnboarding }: { onOpenOnboarding?: () => void }) {
  const { appearance, resolved, error } = useAppearance();
  const toast = useToast();
  const [autostart, setAuto] = useState(false);
  const [exportPath, setExportPath] = useState("");
  const [exportError, setExportError] = useState<string | null>(null);
  const [importing, setImporting] = useState(false);
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    getAutostart().then(setAuto).catch(() => {});
    getAppVersion().then(setVersion).catch(() => {});
  }, []);

  const toggleAutostart = async (on: boolean) => {
    try {
      await setAutostart(on);
      setAuto(on);
    } catch (e) {
      toast(errText(e), "err");
    }
  };

  const doExport = async () => {
    setExportError(null);
    try {
      await exportConfig(exportPath.trim());
      toast(`已导出到 ${exportPath.trim()}`);
    } catch (e) {
      setExportError(errText(e));
    }
  };

  return (
    <>
      <PageHead title="设置" sub="客户端级配置，对所有机器人生效" />
      <div className="scroll flex-1" style={{ padding: "16px 24px" }}>
        <div className="grid" style={{ maxWidth: 820, gap: 12 }}>
          <div className="grp-t">外观</div>
          <div className="card">
            <SettingRow
              title="外观模式"
              desc={
                <>
                  深色适合长时间盯日志；跟随系统会随 Windows 深浅色自动切换
                  {appearance.mode === "system" && <span> · 当前系统为{resolved === "dark" ? "深色" : "浅色"}</span>}
                </>
              }
            >
              <ModeSeg />
            </SettingRow>
            <SettingRow title="主题色" desc="影响主按钮、选中态、开关、进度条和链接；成功 / 警告 / 错误的颜色不变" last>
              <AccentSwatches />
            </SettingRow>
          </div>
          {error && (
            <div className="alert-err" role="alert" style={{ fontSize: 12 }}>
              外观保存失败：{error}
            </div>
          )}

          <div className="grp-t" style={{ marginTop: 6 }}>
            通用
          </div>
          <div className="card">
            <SettingRow title="开机自启" desc="登录 Windows 后在托盘启动，并按各机器人的「随客户端启动」自动开机器人">
              <Switch on={autostart} label="开机自启" onChange={(on) => void toggleAutostart(on)} />
            </SettingRow>
            <SettingRow title="关闭窗口时最小化到托盘" desc="点右上角关闭不退出，机器人继续收消息；从托盘菜单「退出」才会结束" last>
              <span className="chip c-mut">始终开启</span>
            </SettingRow>
          </div>

          <div className="grp-t" style={{ marginTop: 6 }}>
            运行
          </div>
          <RuntimeSettingsCard />

          <div className="grp-t" style={{ marginTop: 6 }}>
            配置
          </div>
          <div className="card">
            <SettingRow title="导出配置" desc="给同事复用项目、数据源、技能设置、流程模板；不包含 App Secret、数据库密码等密钥，仓库地址里的账号 / 令牌也会去掉">
              <div className="flex gap-2">
                <input
                  className="inp mono"
                  style={{ width: 300 }}
                  aria-label="导出路径"
                  placeholder="保存路径，如 D:/beetle-config.json"
                  value={exportPath}
                  onChange={(e) => setExportPath(e.target.value)}
                />
                <button type="button" className="btn tr" disabled={!exportPath.trim()} onClick={() => void doExport()}>
                  <Download />
                  导出
                </button>
              </div>
            </SettingRow>
            {exportError && (
              <div className="alert-err" role="alert" style={{ margin: "0 16px 12px", fontSize: 12 }}>
                {exportError}
              </div>
            )}
            <SettingRow title="导入同事配置" desc="先预览，再勾选要导入的项目、数据源、模板和技能设置" last={!importing}>
              <button type="button" className="btn tr" onClick={() => setImporting((v) => !v)}>
                {importing ? "收起" : "选择文件导入"}
              </button>
            </SettingRow>
            {importing && (
              <div style={{ padding: "0 16px 14px" }}>
                <ImportConfig />
              </div>
            )}
          </div>

          <div className="grp-t" style={{ marginTop: 6 }}>
            关于
          </div>
          <div className="card">
            <UpdateRow version={version} />
            <SettingRow title="首次启动引导" desc="重新走一遍：环境检查、从旧版导入、创建机器人" last>
              <button type="button" className="btn tr" disabled={!onOpenOnboarding} onClick={onOpenOnboarding}>
                <Sparkles />
                重新打开
              </button>
            </SettingRow>
          </div>
        </div>
      </div>
    </>
  );
}
