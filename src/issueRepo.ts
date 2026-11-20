// 建 issue 的平台与仓库推断：与后端 src-tauri/src/issue.rs、技能脚本 _shared/bp_issue.py 同一套规则（只用于界面显示和默认勾选）。
import type { Project, RecordOpts } from "./api";

export type IssuePlatform = "gitee" | "github";

export const PLATFORM_LABEL: Record<IssuePlatform, string> = { gitee: "Gitee", github: "GitHub" };
export const DEFAULT_TOKEN_ENV: Record<IssuePlatform, string> = { gitee: "GITEE_TOKEN", github: "GITHUB_TOKEN" };

export function platformOfHost(host: string): IssuePlatform | null {
  const h = host.trim().toLowerCase().replace(/^www\./, "");
  return h === "gitee.com" ? "gitee" : h === "github.com" ? "github" : null;
}

/** 仓库地址 → { host, owner, repo }。支持 git@host:o/r(.git)、ssh://、https://[user:pass@]host/o/r(.git)、host/o/r。 */
export function parseRepo(url: string): { host: string; owner: string; repo: string } | null {
  const s = url.trim();
  if (!s) return null;
  let host: string;
  let path: string;
  const i = s.indexOf("://");
  if (i >= 0) {
    const rest = s.slice(i + 3);
    const j = rest.indexOf("/");
    const auth = j >= 0 ? rest.slice(0, j) : rest;
    path = j >= 0 ? rest.slice(j) : "";
    const hostport = auth.slice(auth.lastIndexOf("@") + 1);
    host = hostport.split(":")[0].toLowerCase();
  } else {
    const c = s.indexOf(":");
    const prefix = c >= 0 ? s.slice(0, c) : "";
    if (c >= 0 && !prefix.includes("/") && !/\s/.test(prefix)) {
      host = prefix.slice(prefix.lastIndexOf("@") + 1).toLowerCase();
      path = s.slice(c + 1);
    } else {
      const k = s.indexOf("/");
      host = (k >= 0 ? s.slice(0, k) : s).toLowerCase();
      path = k >= 0 ? s.slice(k + 1) : "";
    }
  }
  const parts = path.split("/").filter(Boolean);
  if (!host || parts.length < 2) return null;
  const repo = parts[1].endsWith(".git") ? parts[1].slice(0, -4) : parts[1];
  if (!repo) return null;
  return { host, owner: parts[0], repo };
}

export function platformOf(repoUrl: string): IssuePlatform | null {
  const p = parseRepo(repoUrl);
  return p ? platformOfHost(p.host) : null;
}

/** 没配置过时的默认记录方式：gitee / github 仓库建 issue；写飞书文档默认开。 */
export function defaultRecord(repoUrl: string): RecordOpts {
  return { issue: platformOf(repoUrl) !== null, feishu_doc: true };
}

/** 项目的建 issue 目标（平台、owner/repo、token 变量名）；推不出平台返回 null。 */
export function issueTarget(p: Project): { platform: IssuePlatform; repo: string; tokenEnv: string } | null {
  const base = parseRepo(p.repo);
  let platform = base ? platformOfHost(base.host) : null;
  let full = base ? `${base.owner}/${base.repo}` : null;
  const override = (p.issue_repo ?? "").trim();
  if (override) {
    const parts = override.split("/").filter(Boolean);
    if (parts.length === 2 && !override.includes(":") && !override.includes("@")) {
      full = `${parts[0]}/${parts[1].replace(/\.git$/, "")}`;
    } else {
      const o = parseRepo(override);
      if (!o) return null;
      platform = platformOfHost(o.host) ?? platform;
      full = `${o.owner}/${o.repo}`;
    }
  }
  if (!platform || !full) return null;
  const tokenEnv = (p.token_env ?? "").trim() || DEFAULT_TOKEN_ENV[platform];
  return { platform, repo: full, tokenEnv };
}
