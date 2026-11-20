// 自动更新：发现新版本时在右下角弹出卡片（可「立即更新」或「稍后」，更新时显示下载进度），以及设置页「关于」里用的 useUpdate。
// 后端启动后检查一次、之后每 30 分钟检查一次，发现新版本推 update-available 事件；这里只负责展示和触发安装。
import { openUrl } from "@tauri-apps/plugin-opener";
import { ArrowUpCircle, Download, ExternalLink, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { checkUpdate, errText, installUpdate, onUpdateAvailable, onUpdateProgress, updateStatus, type UpdateStatus } from "../api";

/** GitHub 上这个版本的发布页（更新说明）。 */
export const releaseUrl = (version: string) => `https://github.com/LingyiChen-AI/beetle/releases/tag/v${version}`;

export function useUpdate() {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [busy, setBusy] = useState<"check" | "install" | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** 下载进度 0–100；总大小未知时为 null */
  const [progress, setProgress] = useState<number | null>(null);

  useEffect(() => {
    let alive = true;
    updateStatus()
      .then((s) => alive && setStatus(s))
      .catch(() => {});
    const un = onUpdateAvailable((s) => alive && setStatus(s));
    const unP = onUpdateProgress((p) => alive && setProgress(p.total ? Math.min(100, Math.round((p.downloaded / p.total) * 100)) : null));
    return () => {
      alive = false;
      void un.then((f) => f());
      void unP.then((f) => f());
    };
  }, []);

  const check = useCallback(async () => {
    setError(null);
    setBusy("check");
    try {
      setStatus(await checkUpdate());
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(null);
    }
  }, []);

  const install = useCallback(async () => {
    setError(null);
    setProgress(null);
    setBusy("install");
    try {
      await installUpdate(); // 成功后客户端会重启
    } catch (e) {
      setError(errText(e));
      setBusy(null);
    }
  }, []);

  return { status, busy, error, progress, check, install };
}

export default function UpdateBanner() {
  const { status, busy, error, progress, install } = useUpdate();
  const [dismissed, setDismissed] = useState<string | null>(null);
  const version = status?.available;
  if (!version || dismissed === version) return null;
  const installing = busy === "install";
  return (
    <div className="update-card" role="status" aria-label="发现新版本" data-testid="update-banner">
      <div className="flex items-start" style={{ gap: 12 }}>
        <div className="update-card-icon">
          <ArrowUpCircle />
        </div>
        <div className="min-w-0 flex-1">
          <div style={{ fontWeight: 600 }}>
            发现新版本 <span className="mono">v{version}</span>
          </div>
          <div style={{ color: "var(--muted)", fontSize: 12, marginTop: 2 }}>
            当前 <span className="mono">v{status?.current}</span>
            {!installing && (
              <>
                {" · "}
                <button type="button" className="lnk" onClick={() => void openUrl(releaseUrl(version))}>
                  更新说明
                  <ExternalLink style={{ width: 11, height: 11, marginLeft: 2, verticalAlign: -1 }} />
                </button>
              </>
            )}
          </div>
        </div>
        {!installing && (
          <button type="button" className="update-card-close tr" aria-label="稍后更新" onClick={() => setDismissed(version)}>
            <X />
          </button>
        )}
      </div>

      {installing && (
        <div style={{ marginTop: 12 }}>
          <div className="update-progress">
            <div className={progress === null ? "update-progress-bar indeterminate" : "update-progress-bar"} style={progress === null ? undefined : { width: `${progress}%` }} />
          </div>
          <div style={{ color: "var(--muted)", fontSize: 12, marginTop: 6 }}>
            {progress === null ? "正在下载更新…" : progress < 100 ? `正在下载 ${progress}%` : "下载完成，正在安装，稍后自动重启…"}
          </div>
        </div>
      )}
      {error && (
        <div style={{ color: "var(--danger-fg)", fontSize: 12, marginTop: 10, lineHeight: 1.5 }} role="alert">
          {error}
        </div>
      )}
      {!installing && (
        <div className="flex justify-end" style={{ gap: 8, marginTop: 14 }}>
          <button type="button" className="btn btn-ghost btn-sm tr" onClick={() => setDismissed(version)}>
            稍后
          </button>
          <button type="button" className="btn btn-pri btn-sm tr" onClick={() => void install()}>
            <Download />
            {error ? "重试" : "立即更新"}
          </button>
        </div>
      )}
    </div>
  );
}
