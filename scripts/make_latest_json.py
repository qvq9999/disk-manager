#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""
生成 Tauri 自动更新所需的 latest.json。

用法（在 disk-manager 目录下）：
    python scripts/make_latest_json.py --repo <用户名>/<仓库名> --notes "更新说明"

前置条件：
    · 已用 TAURI_SIGNING_PRIVATE_KEY 完成 `npx tauri build --bundles nsis`
      （否则 bundle 目录里不会有 .sig 文件）

产物：
    release/latest.json  —— 与安装包一起上传到 GitHub Release 即可
    release/ 目录下同时会给出上传清单

为什么需要这个文件：
    应用内的 updater 请求的是「最新版本的清单」，它靠清单里的 signature
    用编译进程序的公钥校验安装包。没有清单或签名不符，更新一律被拒绝 ——
    这是防止有人伪造更新源投毒的关键。
"""
import argparse
import hashlib
import json
import os
import re
import shutil
import sys
from datetime import datetime, timezone

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NSIS_DIR = os.path.join(ROOT, 'src-tauri', 'target', 'release', 'bundle', 'nsis')
OUT_DIR = os.path.join(ROOT, 'release')


def find_artifacts():
    """返回 (setup.exe 路径, sig 路径)"""
    if not os.path.isdir(NSIS_DIR):
        sys.exit(f'找不到打包目录：{NSIS_DIR}\n请先执行 npx tauri build --bundles nsis')
    exes = [f for f in os.listdir(NSIS_DIR) if f.endswith('-setup.exe')]
    if not exes:
        sys.exit('打包目录里没有 *-setup.exe，请先构建')
    # 取最新的一个
    exes.sort(key=lambda f: os.path.getmtime(os.path.join(NSIS_DIR, f)), reverse=True)
    exe = os.path.join(NSIS_DIR, exes[0])
    sig = exe + '.sig'
    if not os.path.isfile(sig):
        sys.exit(
            '找不到签名文件：' + sig + '\n'
            '说明构建时没有设置 TAURI_SIGNING_PRIVATE_KEY，或 bundle.createUpdaterArtifacts 未开启。\n'
            '示例：\n'
            '  export TAURI_SIGNING_PRIVATE_KEY=/path/to/disk-manager.key\n'
            '  npx tauri build --bundles nsis'
        )
    return exe, sig


def read_version():
    """从 tauri.conf.json 读版本，保证与安装包一致"""
    conf = os.path.join(ROOT, 'src-tauri', 'tauri.conf.json')
    with open(conf, encoding='utf-8') as f:
        return json.load(f)['version']


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--repo', required=True, help='GitHub 仓库，形如 octocat/disk-manager')
    ap.add_argument('--notes', default='', help='更新说明（支持换行）')
    ap.add_argument('--tag', default=None, help='Release tag，默认 v<版本号>')
    ap.add_argument('--draft-ok', action='store_true', help='仅生成文件，不检查 URL 可达性')
    args = ap.parse_args()

    if not re.fullmatch(r'[\w.-]+/[\w.-]+', args.repo):
        sys.exit(f'--repo 格式不对：{args.repo}（应为 用户名/仓库名）')

    exe, sig = find_artifacts()
    version = read_version()
    tag = args.tag or f'v{version}'
    exe_name = os.path.basename(exe)

    with open(sig, encoding='utf-8') as f:
        signature = f.read().strip()
    if not signature:
        sys.exit('签名文件是空的，构建可能没真正签名')

    url = f'https://github.com/{args.repo}/releases/download/{tag}/{exe_name}'
    sha256 = hashlib.sha256(open(exe, 'rb').read()).hexdigest()

    latest = {
        'version': version,
        'notes': args.notes or f'存储管理 Disk Manager {version}',
        'pub_date': datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ'),
        'platforms': {
            'windows-x86_64': {
                'signature': signature,
                'url': url,
            }
        },
    }

    os.makedirs(OUT_DIR, exist_ok=True)
    # 先清掉上一次发版留下的安装包。
    # 否则 release/ 会越攒越多，而 release.py 是按「release/ 下所有 .exe/.sig」上传的，
    # 结果新版本的 Release 里混进旧版本安装包（v1.2.0 实际踩过）。
    for f in os.listdir(OUT_DIR):
        if f.endswith(('.exe', '.sig')):
            os.remove(os.path.join(OUT_DIR, f))

    out_json = os.path.join(OUT_DIR, 'latest.json')
    with open(out_json, 'w', encoding='utf-8') as f:
        json.dump(latest, f, ensure_ascii=False, indent=2)

    # 把安装包与签名复制到 release/，方便一次性拖拽上传
    shutil.copy2(exe, os.path.join(OUT_DIR, exe_name))
    shutil.copy2(sig, os.path.join(OUT_DIR, exe_name + '.sig'))

    size_mb = os.path.getsize(exe) / 1024 / 1024
    print('=' * 60)
    print(f'版本      {version}   (tag {tag})')
    print(f'安装包    {exe_name}   {size_mb:.2f} MiB')
    print(f'SHA256    {sha256}')
    print(f'清单      {out_json}')
    print('=' * 60)
    print('\n接下来：')
    print(f'  1. 在 GitHub 创建 Release，tag 填 {tag}')
    print(f'  2. 上传 release/ 下的 3 个文件：')
    print(f'       {exe_name}')
    print(f'       {exe_name}.sig')
    print(f'       latest.json')
    print(f'  3. 确认 tauri.conf.json 的 endpoints 指向：')
    print(f'       https://github.com/{args.repo}/releases/latest/download/latest.json')
    print(f'     以及 src-tauri/src/commands/update.rs 的 ENDPOINT 常量同步改成上面的地址')
    print(f'  4. 发布后，旧版本启动时会自动检查到 {version}')


if __name__ == '__main__':
    main()
