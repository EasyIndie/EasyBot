#!/usr/bin/env python3
"""GHCR 保留策略 reconcile — EasyIndie/easybot 容器镜像包。

维持的不变量（任一时刻成立）：
  GHCR 仅含
    1. 当前 `main`（/`latest`）滚动镜像树，
    2. 全部正式 release 镜像树（带 `N.N[.N]` 数字 tag 的 index），
    3. 上述树的子 manifest（平台镜像）与 attestation。
  其余（被取代的 sha7 dev 树、孤儿 untagged、孤儿 attestation）一律删除。

幂等；默认 dry-run 只打印计划，`--apply` 才执行。

用法：
  python3 scripts/ghcr-reconcile.py            # dry-run
  python3 scripts/ghcr-reconcile.py --apply    # 执行删除
  python3 scripts/ghcr-reconcile.py --keep-releases 6   # 只保留最近 6 个 release 树（可选收紧）

鉴权：删除 org 级包需 read:packages + delete:packages 的 token
  （repo 的 GITHUB_TOKEN 无权删 org 级包）。取 token 顺序：
  env GHCR_TOKEN > env GH_TOKEN > `gh auth token`。读取公共包走匿名即可。

图安全：凡被“保留 index”引用的子 manifest（平台/attestation）绝不删除，
  即使它同时被一棵将被删除的树引用；删除顺序子先于父。
  保留 index 的 manifest 若读取失败 → fail-closed 中止（宁可少删不误删）。
"""
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed

ORG = os.environ.get("GHCR_ORG", "EasyIndie")
PKG = os.environ.get("GHCR_PKG", "easybot")
API_PKG = f"orgs/{ORG}/packages/container/{PKG}"
API_VER = f"{API_PKG}/versions"

APPLY = "--apply" in sys.argv
KEEP_RELEASES = None
for i, a in enumerate(sys.argv):
    if a == "--keep-releases":
        KEEP_RELEASES = int(sys.argv[i + 1])

ACCEPT = ("application/vnd.oci.image.index.v1+json, "
          "application/vnd.docker.distribution.manifest.list.v2+json, "
          "application/vnd.oci.image.manifest.v1+json, "
          "application/vnd.docker.distribution.manifest.v2+json")
SEM = re.compile(r"^\d+\.\d+(\.\d+)?$")      # 数字版本 tag：0.0 / 0.0.40 / 0.1 ...
AUTO = lambda t: t.startswith("sha256-")


def gh_token():
    for name in ("GHCR_TOKEN", "GH_TOKEN"):
        if os.environ.get(name):
            return os.environ[name]
    p = subprocess.run(["gh", "auth", "token"], capture_output=True, text=True)
    return p.stdout.strip() if p.returncode == 0 else ""


def api_json(url):
    tok = gh_token()
    req = urllib.request.Request(url, headers={"Authorization": "Bearer " + tok,
                                               "Accept": "application/vnd.github+json",
                                               "User-Agent": "ghcr-reconcile"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


def api_delete(path):
    tok = gh_token()
    req = urllib.request.Request("https://api.github.com/" + path, method="DELETE",
                                 headers={"Authorization": "Bearer " + tok,
                                          "Accept": "application/vnd.github+json",
                                          "User-Agent": "ghcr-reconcile"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return r.status


def registry_token():
    url = f"https://ghcr.io/token?scope=repository:{ORG.lower()}/{PKG}:pull&service=ghcr.io"
    headers = {}
    if gh_token():
        headers["Authorization"] = "Bearer " + gh_token()
    req = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.load(r)["token"]
    except Exception:
        return ""


def children_of(digest, rtoken):
    """index 的子 manifest digest 列表（单 manifest / 404 → 空；网络错 → None 表示失败）。"""
    req = urllib.request.Request(f"https://ghcr.io/v2/{ORG.lower()}/{PKG}/manifests/{digest}",
                                 headers={"Authorization": "Bearer " + rtoken, "Accept": ACCEPT})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            m = json.load(r)
        return [c["digest"] for c in m.get("manifests", [])] if "manifests" in m else []
    except urllib.error.HTTPError as e:
        if e.code in (404, 400, 401, 403):
            return []                      # 不存在/不可读：按无子处理
        return None
    except Exception:
        return None


def fetch_versions():
    out, page = {}, 1
    while True:
        b = api_json(f"https://api.github.com/{API_VER}?per_page=100&page={page}")
        if not b:
            break
        for v in b:
            out[v["id"]] = {"id": v["id"], "digest": v["name"],
                            "tags": v["metadata"].get("container", {}).get("tags", []),
                            "created": v["created_at"][:10]}
        if len(b) < 100:
            break
        page += 1
    return out


def delete_one(item):
    vid, tries = item, 0
    while True:
        tries += 1
        try:
            api_delete(f"{API_VER}/{vid}")
            return ("ok", vid, "")
        except urllib.error.HTTPError as e:
            if e.code == 404:
                return ("gone", vid, "")
            if e.code in (403, 429) or e.code >= 500:
                if tries >= 6:
                    return ("fail", vid, f"HTTP {e.code}")
                time.sleep(min(2 ** tries, 20))
                continue
            return ("fail", vid, f"HTTP {e.code}")
        except Exception as e:
            if tries >= 6:
                return ("fail", vid, str(e)[:120])
            time.sleep(min(2 ** tries, 20))


def main():
    vers = fetch_versions()
    n = len(vers)

    auto_tagged = {}      # 只带 sha256- 自动 tag（attestation / reference-type）
    indexes = {}          # 带真实 tag 的 index
    for v in vers.values():
        if not v["tags"]:
            continue                         # 无 tag 版本：作为保留 index 的子或孤儿处理
        if any(not AUTO(t) for t in v["tags"]):
            indexes[v["digest"]] = v
        else:
            auto_tagged[v["digest"]] = v

    def owner(tag):
        return next((d for d, v in indexes.items() if tag in v["tags"]), None)

    main_owner = owner("main") or owner("latest")

    # release 树 = 带数字版本 tag 的 index；releases 保留列表（升序，取 max 版本避免 0.0/0.0.40 组合排序错乱）
    rel = sorted((d for d, v in indexes.items() if any(SEM.match(t) for t in v["tags"])),
                 key=lambda d: max((tuple(int(x) for x in t.split("."))
                                    for t in indexes[d]["tags"] if SEM.match(t))))
    if KEEP_RELEASES is not None and len(rel) > KEEP_RELEASES:
        keep_rel = set(rel[-KEEP_RELEASES:])
    else:
        keep_rel = set(rel)

    keep_indexes = set(keep_rel)
    if main_owner:
        keep_indexes.add(main_owner)

    # 收集保留 index 的子（图安全：读取失败 → fail-closed）
    rtoken = registry_token()
    if not rtoken and gh_token():
        print("WARN: 无法获取 registry 读 token；尝试继续", file=sys.stderr)
    children = {}
    for d in keep_indexes:
        c = children_of(d, rtoken)
        if c is None:
            print(f"FATAL: 保留 index {d} 的 manifest 读取失败——中止，宁可少删不误删", file=sys.stderr)
            sys.exit(2)
        children[d] = c

    keep_digests = set(keep_indexes)
    for cset in children.values():
        keep_digests |= set(cset)
    # 保留：attestation（只带 sha256- 自动 tag）其 subject 是被保留 index
    for d, v in auto_tagged.items():
        if any(("sha256:" + t[len("sha256-"):]) in keep_indexes for t in v["tags"]):
            keep_digests.add(d)

    # 删除集 = 全集 - 保留集
    delete = [v for v in vers.values() if v["digest"] not in keep_digests]
    # 安全断言：保留 index 的子绝不进删除集
    for d, cset in children.items():
        bad = [c for c in cset if c not in keep_digests]
        assert not bad, f"保留 index {d[:12]} 将失去子 {[c[:12] for c in bad]}"

    print(f"registry total: {n}")
    print(f"  keep: {len(keep_digests)} versions "
          f"(indexes {len(keep_indexes)}: main={main_owner[:12] if main_owner else '-'} "
          f"+ releases {sorted(indexes[d]['tags'] for d in keep_rel)} ... )")
    print(f"  delete candidates: {len(delete)}")
    if not APPLY:
        sample = ", ".join(sorted({t for v in delete[:6] for t in v['tags']} or ["<untagged>"]))
        print("DRY-RUN: 传入 --apply 才执行。示例待删 tags:", sample[:160] or "<untagged>")
        sys.exit(0)

    # 子先于父：wave1 无 tag/ref（子、孤儿），wave2 带真实 tag（index）
    wave1 = [v["id"] for v in delete if not v["tags"] or (v["tags"] and all(AUTO(t) for t in v["tags"]))]
    wave2 = [v["id"] for v in delete if v["tags"] and any(not AUTO(t) for t in v["tags"])]
    stats = {"ok": 0, "gone": 0, "fail": 0}
    failed = []
    for label, wave in (("children/orphans", wave1), ("indexes", wave2)):
        if not wave:
            continue
        with ThreadPoolExecutor(max_workers=8) as ex:
            futs = {ex.submit(delete_one, vid): vid for vid in wave}
            for k, fut in enumerate(as_completed(futs), 1):
                st, vid, msg = fut.result()
                stats[st] += 1
                if st == "fail":
                    failed.append((vid, msg))
                if k % 100 == 0 or k == len(wave):
                    print(f"  [{label}] {k}/{len(wave)}  ok={stats['ok']} gone={stats['gone']} "
                          f"fail={stats['fail']}", flush=True)
    print(f"DONE ok={stats['ok']} gone={stats['gone']} fail={stats['fail']}")
    if failed:
        print("FAILED ids:", failed[:20], file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
