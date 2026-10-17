#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""
一条命令完成一次发版：改版本 → 带签名构建 → 生成更新清单 → 提交打标签 → 推送 → 创建 Release 并上传。

前提：
    · 环境变量 GH_TOKEN 已设置（GitHub PAT，需要 repo 权限）
      Windows: setx GH_TOKEN "ghp_xxx" 后重开终端；临时用：export GH_TOKEN=...
    · TAURI_SIGNING_PRIVATE_KEY 指向私钥文件
    · 远端已配置：git remote add origin https://github.com/<owner>/<repo>.git

常用：
    python scripts/release.py --bump --notes "修复卸载卡住"          # 版本 +0.1 并发版
    python scripts/release.py --notes "只构建，不推送" --no-push     # 本地演练
    python scripts/release.py --bump --notes "..." --dry-run        # 只打印计划
"""
import argparse
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CONF = os.path.join(ROOT, 'src-tauri', 'tauri.conf.json')
CARGO = os.path.join(ROOT, 'src-tauri', 'Cargo.toml')
PKG = os.path.join(ROOT, 'package.json')
RELEASE_DIR = os.path.join(ROOT, 'release')


def sh(cmd, check=True, capture=True, env=None):
    r = subprocess.run(cmd, cwd=ROOT, shell=isinstance(cmd, str),
                       capture_output=capture, text=True,
                       encoding='utf-8', errors='replace',
                       env=env or os.environ.copy())
    if check and r.returncode != 0:
        print(f'[失败] {cmd}')
        print((r.stdout or '')[-1500:])
        print((r.stderr or '')[-1500:])
        sys.exit(r.returncode)
    return (r.stdout or '') + (r.stderr or '')


def read_version():
    with open(CONF, encoding='utf-8') as f:
        return json.load(f)['version']


def bump_version(v):
    """1.1.0 -> 1.2.0（用户约定：每次 +0.1）"""
    parts = v.split('.')
    major, minor = int(parts[0]), int(parts[1])
    return f'{major}.{minor + 1}.0'


def append_changelog(ver, notes):
    """
    把本次说明插到 CHANGELOG.md 最前面。

    ⚠️ 必须在**构建之前**执行：CHANGELOG 会被 `?raw` 内联进前端 bundle，
    构建后再改就进不了这个版本的安装包，设置页里会缺本次记录。
    """
    path = os.path.join(ROOT, 'CHANGELOG.md')
    if not os.path.isfile(path) or not notes.strip():
        return
    with open(path, encoding='utf-8') as f:
        s = f.read()
    if re.search(rf'(?m)^##\s+v?{re.escape(ver)}\b', s):
        print(f'  CHANGELOG 已有 {ver}，跳过追加')
        return
    today = datetime.now().strftime('%Y-%m-%d')
    # 用户传的说明里可能自带 `## 修复…` 这类标题，降一级作为分组
    body = re.sub(r'(?m)^##\s+', '### ', notes.strip())
    entry = f'## {ver} · {today}\n\n{body}\n\n'
    # 插到第一个 `## ` 之前（文件顶部是 `# 更新日志` 与说明段）
    idx = s.find('\n## ')
    s = s.rstrip() + '\n\n' + entry if idx < 0 else s[:idx + 1] + entry + s[idx + 1:]
    with open(path, 'w', encoding='utf-8', newline='') as f:
        f.write(s)
    print(f'  CHANGELOG 已加入 {ver}')


def write_version(new):
    """四处版本号必须同步：漏掉任何一处都会让「程序内显示的版本」和安装包不一致"""
    for path, pat, rep in [
        (CONF, r'("version"\s*:\s*")[^"]+(")', rf'\g<1>{new}\g<2>'),
        (CARGO, r'(?m)^(version\s*=\s*")[^"]+(")', rf'\g<1>{new}\g<2>'),
        (PKG, r'("version"\s*:\s*")[^"]+(")', rf'\g<1>{new}\g<2>'),
    ]:
        with open(path, encoding='utf-8') as f:
            s = f.read()
        s2 = re.sub(pat, rep, s, count=1)
        with open(path, 'w', encoding='utf-8', newline='') as f:
            f.write(s2)

    # 前端 mock 与侧边栏文案
    for path, pat, rep in [
        (os.path.join(ROOT, 'src', 'services', 'api.ts'), r"(appVersion:\s*')[^']+(')", rf'\g<1>{new}\g<2>'),
        (os.path.join(ROOT, 'src', 'App.tsx'), r'Disk Manager v[\d.]+', f'Disk Manager v{new.rsplit(".", 1)[0]}'),
    ]:
        if not os.path.isfile(path):
            continue
        with open(path, encoding='utf-8') as f:
            s = f.read()
        with open(path, 'w', encoding='utf-8', newline='') as f:
            f.write(re.sub(pat, rep, s, count=1))


def repo_from_remote():
    out = sh('git remote get-url origin', check=False)
    m = re.search(r'github\.com[:/]([\w.-]+)/([\w.-]+?)(?:\.git)?\s*$', out.strip())
    return f'{m.group(1)}/{m.group(2)}' if m else None


def push_with_token(token, repo):
    """
    推送主分支。

    本机环境的两个坑（换机器时可能不存在，但保留这段逻辑不会有副作用）：
    1. 全局 gitconfig 有 `url.'https://ghproxy.net/https://github.com/'.insteadOf`，
       ghproxy 是只读加速代理，push 会报 "could not read Username"。
       → 把 token 写进 URL，前缀不再匹配 `https://github.com/`，自然绕过重写。
    2. 直连 github.com 时 schannel 报 CRYPT_E_NO_REVOCATION_CHECK（代理 MITM 证书无法查吊销），
       → GIT_SSL_NO_VERIFY=true。仅影响这一次推送，不写入任何配置。
    """
    url = f'https://x-access-token:{token}@github.com/{repo}.git'
    env = os.environ.copy()
    env['GIT_SSL_NO_VERIFY'] = 'true'
    env['GIT_TERMINAL_PROMPT'] = '0'
    out = sh(['git', 'push', url, 'HEAD:main', '--follow-tags'], check=False, env=env)
    return out


def api(method, path, payload=None, token=None):
    url = path if path.startswith('http') else 'https://api.github.com' + path
    headers = {
        'Authorization': f'Bearer {token}',
        'Accept': 'application/vnd.github+json',
        'User-Agent': 'disk-manager-release',
    }
    data = None
    if payload is not None:
        headers['Content-Type'] = 'application/json'
        data = json.dumps(payload).encode('utf-8')
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=300) as r:
            body = r.read().decode('utf-8', 'replace')
            return json.loads(body) if body.strip() else {}
    except urllib.error.HTTPError as e:
        detail = e.read().decode('utf-8', 'replace')[:600]
        raise RuntimeError(f'{method} {path} -> {e.code}: {detail}') from None


def upload_asset(upload_base, name, path, token):
    """
    上传 Release 资产。

    ⚠️ 这里用 curl 而不是 urllib：本机走本地代理时，urllib 对 uploads.github.com 的
    CONNECT 隧道会返回 502，而 curl 走同样的代理正常。实测 latest.json / .sig / 3.7MB
    安装包三者 curl 均为 201。
    """
    cmd = ['curl', '-sS', '--ssl-no-revoke', '-X', 'POST',
           '-H', f'Authorization: Bearer {token}',
           '-H', 'Content-Type: application/octet-stream',
           '-H', 'User-Agent: disk-manager-release',
           '--data-binary', f'@{path}',
           f'{upload_base}?name={name}',
           '-w', 'HTTP:%{http_code}', '-o', os.devnull, '-m', '900']
    out = sh(cmd, check=False)
    m = re.search(r'HTTP:(\d{3})', out)
    code = m.group(1) if m else '000'
    if code not in ('200', '201'):
        raise RuntimeError(f'上传 {name} 失败，HTTP {code}')
    return code


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--bump', action='store_true', help='版本号 +0.1')
    ap.add_argument('--notes', default='', help='更新说明')
    ap.add_argument('--tag', default=None, help='默认 v<版本>')
    ap.add_argument('--no-push', action='store_true', help='只本地提交，不推送、不建 Release')
    ap.add_argument('--dry-run', action='store_true', help='只打印计划，不做任何修改')
    args = ap.parse_args()

    cur = read_version()
    ver = bump_version(cur) if args.bump else cur
    tag = args.tag or f'v{ver}'
    repo = repo_from_remote()

    print(f'[计划] 当前 {cur} -> 版本 {ver}   tag {tag}')
    print(f'[计划] 远端 {repo or "（未配置 origin）"}')
    if args.notes:
        print(f'[计划] 说明 {args.notes[:120]}')
    if args.dry_run:
        return

    if args.bump:
        write_version(ver)
        print(f'[1/6] 版本号已改为 {ver}')

    # 必须在构建之前：CHANGELOG 会被内联进前端包
    print('[1.5/6] 写入更新日志…')
    append_changelog(ver, args.notes)

    # 构建（必须带签名，否则没有 .sig，更新会因签名缺失被拒）
    if not os.environ.get('TAURI_SIGNING_PRIVATE_KEY'):
        sys.exit('缺少 TAURI_SIGNING_PRIVATE_KEY —— 没有它构建不出签名，自动更新会用不了')
    print('[2/6] 带签名构建…')
    sh('npx tauri build --bundles nsis')

    print('[3/6] 生成更新清单…')
    if not repo:
        sys.exit('未配置 git remote origin，无法生成下载地址')
    sh([sys.executable, os.path.join(ROOT, 'scripts', 'make_latest_json.py'),
        '--repo', repo, '--tag', tag, '--notes', args.notes or f'版本 {ver}'])

    print('[4/6] 提交并打标签…')
    sh('git add -A')
    sh(['git', 'commit', '-m', f'release: v{ver}\n\n{args.notes or "版本更新"}'], check=False)
    sh(['git', 'tag', '-f', tag], check=False)

    if args.no_push:
        print('[5/6] 已跳过推送（--no-push）')
        print(f'[完成] 本地已准备 v{ver}，产物在 release/')
        return

    token = os.environ.get('GH_TOKEN') or os.environ.get('GITHUB_TOKEN')
    if not token:
        sys.exit('缺少 GH_TOKEN，无法创建 Release。可先 git push，再手动上传 release/ 下的文件')

    print('[5/6] 推送代码与标签…')
    push_out = push_with_token(token, repo)
    print('  ' + (push_out.strip().splitlines()[-1] if push_out.strip() else '已推送'))

    print('[6/6] 创建 Release 并上传…')
    try:
        rel = api('POST', f'/repos/{repo}/releases', {
            'tag_name': tag,
            'name': tag,
            'target_commitish': 'main',
            'body': args.notes or f'版本 {ver}',
            'draft': False,
            'prerelease': False,
        }, token=token)
    except RuntimeError as e:
        if '422' in str(e):
            # 已存在同 tag 的 Release：复用并覆盖资产
            rel = api('GET', f'/repos/{repo}/releases/tags/{tag}', token=token)
            print('  该 tag 已有 Release，改为覆盖资产')
        else:
            raise

    upload_base = rel['upload_url'].split('{')[0]
    rel = api('GET', f'/repos/{repo}/releases/tags/{tag}', token=token)
    # 只上传与**本次版本**匹配的包：release/ 里可能残留旧版本文件，
    # 一并上传会让新 Release 混入旧安装包（v1.2.0 实际踩过）
    want = [f for f in os.listdir(RELEASE_DIR)
            if f.endswith(('.exe', '.sig')) and f'_{ver}_' in f] + ['latest.json']
    for name in want:
        path = os.path.join(RELEASE_DIR, name)
        if not os.path.isfile(path):
            continue
        # 同名资产已存在时先删，否则上传会 422
        for a in rel.get('assets', []):
            if a['name'] == name:
                api('DELETE', f'/repos/{repo}/releases/assets/{a["id"]}', token=token)
        upload_asset(upload_base, name, path, token)
        print(f'  已上传 {name}')

    print(f'\n[完成] v{ver} 已发布：https://github.com/{repo}/releases/tag/{tag}')
    print('旧版本启动后会自动检查到新版本。')


if __name__ == '__main__':
    main()
